//! Operator inspection and point-in-time destination verification.
use datamuta_plan::{DataType, Evidence, Record, Scalar, Schema};
use datamuta_postgres as pg;
use datamuta_stream::{self as stream, Plan, Result, TargetEngine, publisher, spool, suggest};
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
    let (source_endpoint, target_endpoint, source, target) =
        describe_pair(source_endpoint, target_endpoint, source_table, target_table).await?;
    let config = stream::Config {
        version: 2,
        source,
        target,
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
/// Discover both PostgreSQL relations; shared by `configure` and `suggest-mapping`.
pub async fn describe_pair(
    source_endpoint: &Path,
    target_endpoint: &Path,
    source_table: &str,
    target_table: &str,
) -> Result<(pg::Endpoint, pg::Endpoint, Schema, Schema)> {
    let source_endpoint: pg::Endpoint = stream::load(source_endpoint)?;
    let target_endpoint: pg::Endpoint = stream::load(target_endpoint)?;
    let source = pg::connect(&source_endpoint).await?;
    let target = pg::connect(&target_endpoint).await?;
    let s = pg::describe(&source, &source_endpoint.namespace, source_table).await?;
    let t = pg::describe(&target, &target_endpoint.namespace, target_table).await?;
    Ok((source_endpoint, target_endpoint, s, t))
}
/// Prepare a JEV mapping request, or the single structural candidate when no choice exists.
pub async fn suggest_prepare(
    endpoints: (&Path, &Path),
    tables: (&str, &str),
    evidence: &Path,
    request_out: &Path,
    mapping_out: &Path,
) -> Result<Value> {
    let (_, _, source, target) =
        describe_pair(endpoints.0, endpoints.1, tables.0, tables.1).await?;
    let evidence: Evidence = stream::load(evidence)?;
    let prepared = suggest::prepare(&source, &target, &evidence)?;
    let mut out = serde_json::to_value(&prepared)?;
    match &prepared {
        suggest::Prepared::UniqueCandidate { mapping } => {
            stream::save(mapping_out, mapping)?;
            out["mapping_file"] = json!(mapping_out);
            out["note"] = json!("only structural option; review field meanings before configure");
        }
        suggest::Prepared::RequestPrepared { payload, .. } => {
            suggest::save_bytes(request_out, payload)?;
            out["request_file"] = json!(request_out);
            out["note"] =
                json!("send the request file to JEV unchanged, then run suggest-mapping admit");
        }
    }
    out["authorization"] = json!("not granted");
    Ok(out)
}
/// Admit a saved JEV response; writes a draft mapping only when the frozen policy accepts.
pub async fn suggest_admit(
    endpoints: (&Path, &Path),
    tables: (&str, &str),
    evidence: &Path,
    request: &Path,
    response: &Path,
    mapping_out: &Path,
    provenance_out: &Path,
) -> Result<Value> {
    for p in [mapping_out, provenance_out] {
        check(
            p.symlink_metadata().is_err(),
            "suggestion output already exists",
        )?;
    }
    let (_, _, source, target) =
        describe_pair(endpoints.0, endpoints.1, tables.0, tables.1).await?;
    let evidence: Evidence = stream::load(evidence)?;
    let request = suggest::read_bounded(request, suggest::MAX_REQUEST_BYTES)?;
    let response = suggest::read_bounded(response, suggest::MAX_RESPONSE_BYTES)?;
    let admitted = suggest::admit(&source, &target, &evidence, &request, &response)?;
    let mut out = serde_json::to_value(&admitted)?;
    if let suggest::Admitted::Accepted {
        mapping,
        provenance,
        ..
    } = &admitted
    {
        stream::save(mapping_out, mapping)?;
        stream::save(provenance_out, provenance)?;
        out["mapping_file"] = json!(mapping_out);
        out["provenance_file"] = json!(provenance_out);
        out["note"] = json!("draft only; run configure, inspect, then approve the exact plan hash");
    }
    out["authorization"] = json!("not granted");
    Ok(out)
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
