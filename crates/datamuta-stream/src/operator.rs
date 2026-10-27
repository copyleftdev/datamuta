//! Operator inspection and point-in-time destination verification.
use datamuta_plan::{DataType, Record, Scalar};
use datamuta_postgres as pg;
use datamuta_stream::{self as stream, Plan, Result, TargetEngine, publisher, spool};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;

fn check(ok: bool, message: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(std::io::Error::other(message).into())
    }
}
pub async fn configure(
    source_endpoint: &Path,
    target_endpoint: &Path,
    source_table: &str,
    target_table: &str,
    mapping: &Path,
    spill_bytes: u64,
) -> Result<stream::Config> {
    let source_endpoint: pg::Endpoint = stream::load(source_endpoint)?;
    let target_endpoint: pg::Endpoint = stream::load(target_endpoint)?;
    let source = pg::connect(&source_endpoint).await?;
    let target = pg::connect(&target_endpoint).await?;
    let config = stream::Config {
        version: 2,
        source: pg::describe(&source, &source_endpoint.namespace, source_table).await?,
        target: pg::describe(&target, &target_endpoint.namespace, target_table).await?,
        source_endpoint,
        target_endpoint,
        source_engine: Default::default(),
        target_engine: Default::default(),
        candidate: stream::load(mapping)?,
        limits: spool::Limits {
            spill_bytes,
            ..Default::default()
        },
    };
    config.compile()?;
    Ok(config)
}
pub fn inspect(plan: &Plan, path: &Path) -> Result<Value> {
    plan.validate()?;
    let _reader = spool::Reader::open(path, &plan.snapshot)?;
    Ok(
        json!({"status":"inspected","plan_sha256":plan.fingerprint()?,"source_engine":plan.config.source_engine,"source_endpoint":plan.config.source_endpoint,"target_engine":plan.config.target_engine,"target_endpoint":plan.config.target_endpoint,"source_schema":plan.config.source,"target_schema":plan.config.target,"mapping":plan.config.candidate,"snapshot":plan.snapshot,"verification":"plan and manifest only; no database connection or payload scan","authorization":"not granted"}),
    )
}
/// Locks the target and receipt tables against writes for the duration of this
/// explicit audit. Values are compared to transformed authenticated spool records.
pub async fn verify(plan: &Plan, path: &Path) -> Result<Value> {
    let mapping = plan.validate()?;
    check(
        plan.config.target_engine == TargetEngine::Postgres,
        "data verification currently supports PostgreSQL destinations only",
    )?;
    let mut reader = spool::Reader::open(path, &plan.snapshot)?;
    let target = &plan.config.target;
    let c = pg::connect(&plan.config.target_endpoint).await?;
    let ns = pg::q(&target.namespace);
    c.batch_execute(&format!("BEGIN ISOLATION LEVEL REPEATABLE READ; LOCK TABLE {}, {ns}.stream_cursor, {ns}.stream_receipts IN SHARE MODE",pg::relation(target))).await?;
    mapping.validate_observed(
        mapping.source(),
        &pg::describe(&c, &target.namespace, &target.relation).await?,
    )?;
    // These tables remain locked while the separate receipt auditor reads them.
    let receipts = publisher::audit(plan, path).await?;
    check(
        receipts.end_partition == plan.snapshot.partitions,
        "data verification requires a fully published snapshot",
    )?;
    let snapshot: String = c
        .query_one("SELECT pg_current_snapshot()::text", &[])
        .await?
        .try_get(0)?;
    let sizes = target
        .fields
        .iter()
        .map(|f| {
            if f.data_type == DataType::Utf8 {
                format!("COALESCE(octet_length({})::bigint,0)", pg::q(&f.name))
            } else {
                "8::bigint".into()
            }
        })
        .collect::<Vec<_>>()
        .join("+");
    let guard = format!("({sizes})<=65536");
    let fields = target
        .fields
        .iter()
        .map(|f| format!("CASE WHEN {guard} THEN {} ELSE NULL END", pg::q(&f.name)))
        .collect::<Vec<_>>()
        .join(",");
    let keys = target
        .primary_key
        .iter()
        .map(|k| pg::q(k))
        .collect::<Vec<_>>()
        .join(",");
    c.batch_execute(&format!("DECLARE datamuta_verify NO SCROLL CURSOR WITHOUT HOLD FOR SELECT {guard},{fields} FROM {} ORDER BY {keys}",pg::relation(target))).await?;
    let mut rows = 0u64;
    let mut digest = Sha256::new();
    for i in 0..plan.snapshot.partitions {
        let meta = reader.meta(i)?;
        check(meta.first == rows, "verification spool ordinal gap")?;
        let expected = mapping.prepare(&reader.records(&meta)?)?;
        // Keep wire buffers bounded even for corrupted large destination values.
        for chunk in expected.chunks(16) {
            let actual = c
                .query(
                    &format!("FETCH FORWARD {} FROM datamuta_verify", chunk.len()),
                    &[],
                )
                .await?;
            check(actual.len() == chunk.len(), "destination is missing rows")?;
            for (native, unit) in actual.iter().zip(chunk) {
                check(
                    native.try_get::<_, bool>(0)?,
                    "destination row exceeds verification wire budget",
                )?;
                let mut record = Record::new();
                for (n, field) in target.fields.iter().enumerate() {
                    let value = match field.data_type {
                        DataType::Bool => {
                            native.try_get::<_, Option<bool>>(n + 1)?.map(Scalar::Bool)
                        }
                        DataType::I32 => native.try_get::<_, Option<i32>>(n + 1)?.map(Scalar::I32),
                        DataType::I64 => native.try_get::<_, Option<i64>>(n + 1)?.map(Scalar::I64),
                        DataType::Utf8 => native
                            .try_get::<_, Option<String>>(n + 1)?
                            .map(Scalar::Utf8),
                    };
                    record.insert(field.name.clone(), value.unwrap_or(Scalar::Null));
                }
                check(
                    &record == unit.fields(),
                    "destination value or identity mismatch",
                )?;
                let bytes = serde_json::to_vec(&record)?;
                digest.update((bytes.len() as u64).to_le_bytes());
                digest.update(bytes);
                rows += 1;
            }
        }
    }
    check(
        c.query("FETCH FORWARD 1 FROM datamuta_verify", &[])
            .await?
            .is_empty(),
        "destination has extra rows",
    )?;
    check(rows == plan.snapshot.rows, "verification row count differs")?;
    c.batch_execute("COMMIT").await?;
    let data_sha256: String = digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(
        json!({"status":"verified","scope":"PostgreSQL destination values and receipt history matched the approved transformed snapshot at the recorded native snapshot; not ongoing consistency or business-semantic approval","plan_sha256":plan.fingerprint()?,"snapshot_root":plan.snapshot.root,"target_snapshot":snapshot,"rows_verified":rows,"fields_verified":rows*target.fields.len() as u64,"data_sha256":data_sha256,"receipt_audit":receipts,"spool":reader.metrics,"unix_seconds":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(std::io::Error::other)?.as_secs()}),
    )
}
