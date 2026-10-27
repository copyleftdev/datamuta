//! PostgreSQL 17 experimental capture/publish profile. One bounded partition is
//! in flight. The source transaction is needed only until the spool is sealed.
use super::*;
use crate::publisher::cert;
pub use crate::publisher::{Boundary, Cursor, Report, audit, run, status};
use datamuta_plan::{DataType, MappedUnit, Record, Scalar};
use datamuta_postgres::{self as pg, Session, q, relation};
use spool::{Builder, Meta};
use tokio_postgres::{Client, Row, types::ToSql};
const FETCH: u64 = 64;
const RAW_BYTES: u64 = 16384;
fn decode(row: &Row, schema: &Schema) -> Result<Record> {
    require(
        row.try_get::<_, bool>(0)?,
        "source row exceeds server-side wire budget",
    )?;
    schema
        .fields
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let n = i + 1;
            let v = match f.data_type {
                DataType::Bool => row.try_get::<_, Option<bool>>(n)?.map(Scalar::Bool),
                DataType::I32 => row.try_get::<_, Option<i32>>(n)?.map(Scalar::I32),
                DataType::I64 => row.try_get::<_, Option<i64>>(n)?.map(Scalar::I64),
                DataType::Utf8 => row.try_get::<_, Option<String>>(n)?.map(Scalar::Utf8),
            };
            Ok((f.name.clone(), v.unwrap_or(Scalar::Null)))
        })
        .collect()
}
/// Stops on any error. An interrupted capture cannot be appended or resumed:
/// its source transaction/snapshot no longer exists. Capture into a fresh path.
pub async fn capture(
    config: Config,
    path: &Path,
    mut observe: impl FnMut(u64) -> Result<()>,
) -> Result<Plan> {
    require(
        config.source_engine == SourceEngine::Postgres,
        "PostgreSQL source engine required",
    )?;
    let m = config.compile()?;
    // Destination metadata is observed on its own connection. Publication rechecks
    // it under native locks; no cross-database transaction is assumed.
    crate::publisher::admit_target(&config).await?;
    let target_schema = m.target().clone();
    let c = pg::connect(&config.source_endpoint).await?;
    c.batch_execute(&format!(
        "BEGIN ISOLATION LEVEL REPEATABLE READ; LOCK TABLE {} IN SHARE MODE",
        relation(m.source())
    ))
    .await?;
    m.validate_observed(
        &pg::describe(&c, &m.source().namespace, &m.source().relation).await?,
        &target_schema,
    )?;
    let snapshot: String = c
        .query_one("SELECT pg_current_snapshot()::text", &[])
        .await?
        .try_get(0)?;
    let source = canonical_hash(&(
        "postgres/cursor-snapshot/v2",
        config.source_endpoint.receipt_domain()?,
        m.source(),
        snapshot,
    ))?;
    let mut writer = Builder::create(
        path,
        config.limits.clone(),
        source,
        m.source().primary_key.clone(),
    )?;
    let sizes = m
        .source()
        .fields
        .iter()
        .map(|f| {
            if f.data_type == DataType::Utf8 {
                format!("COALESCE(octet_length({})::bigint,0)", q(&f.name))
            } else {
                "8::bigint".into()
            }
        })
        .collect::<Vec<_>>()
        .join("+");
    let guard = format!("({sizes})<={RAW_BYTES}");
    let fields = m
        .source()
        .fields
        .iter()
        .map(|f| format!("CASE WHEN {guard} THEN {} ELSE NULL END", q(&f.name)))
        .collect::<Vec<_>>()
        .join(",");
    let keys = m
        .source()
        .primary_key
        .iter()
        .map(|k| q(k))
        .collect::<Vec<_>>()
        .join(",");
    // CASE gates the transmitted field values; no oversized text value is sent
    // merely so the client can reject it. Server memory/work are separate bounds.
    c.batch_execute(&format!("DECLARE datamuta_capture NO SCROLL CURSOR WITHOUT HOLD FOR SELECT {guard},{fields} FROM {} ORDER BY {keys}",relation(m.source()))).await?;
    let mut count = 0;
    loop {
        let rows = c
            .query(&format!("FETCH FORWARD {FETCH} FROM datamuta_capture"), &[])
            .await?;
        if rows.is_empty() {
            break;
        }
        let decoded = rows
            .iter()
            .map(|r| decode(r, m.source()))
            .collect::<Result<Vec<_>>>()?;
        // Validate both input and output byte limits before sealing any record.
        let _ = m.prepare(&decoded)?;
        for record in decoded {
            writer.push(&record)?;
            count += 1;
            observe(count)?;
        }
    }
    let manifest = writer.seal()?;
    c.batch_execute("COMMIT").await?;
    Plan::new(config, manifest)
}
pub struct Destination {
    c: Session,
    mapping: CompiledMapping,
    plan: Fingerprint,
    partitions: u64,
}
async fn metadata(c: &Client, namespace: &str) -> Result<()> {
    for (name, fields) in [
        (
            "stream_cursor",
            vec![
                ("id", DataType::I32),
                ("plan", DataType::Utf8),
                ("next", DataType::I64),
                ("certificate", DataType::Utf8),
            ],
        ),
        (
            "stream_receipts",
            vec![("id", DataType::I64), ("hash", DataType::Utf8)],
        ),
    ] {
        let observed = pg::describe(c, namespace, name).await?;
        require(
            observed.primary_key == ["id"]
                && observed.fields.len() == fields.len()
                && observed
                    .fields
                    .iter()
                    .zip(fields)
                    .all(|(f, (n, t))| f.name == n && f.data_type == t && !f.nullable),
            "stream metadata schema mismatch",
        )?;
    }
    Ok(())
}
impl Destination {
    pub async fn open(plan: &Plan) -> Result<Self> {
        let mapping = plan.validate()?;
        let c = pg::connect(&plan.config.target_endpoint).await?;
        let result = Self {
            c,
            mapping,
            plan: plan.fingerprint()?,
            partitions: plan.snapshot.partitions,
        };
        result.admit().await?;
        Ok(result)
    }
    async fn admit(&self) -> Result<()> {
        let m = &self.mapping;
        metadata(&self.c, &m.target().namespace).await?;
        m.validate_observed(
            m.source(),
            &pg::describe(&self.c, &m.target().namespace, &m.target().relation).await?,
        )?;
        for table in [&m.target().relation, "stream_cursor", "stream_receipts"] {
            let qualified = format!("{}.{}", q(&m.target().namespace), q(table));
            for permission in ["SELECT", "INSERT", "UPDATE"] {
                let allowed: bool = self
                    .c
                    .query_one(
                        "SELECT has_table_privilege($1,$2)",
                        &[&qualified, &permission],
                    )
                    .await?
                    .try_get(0)?;
                require(allowed, "stream table privilege missing")?;
            }
        }
        Ok(())
    }
    pub(crate) async fn cursor(&self, lock: bool) -> Result<Cursor> {
        let sql = if lock {
            "SELECT id,plan,next,certificate FROM stream_cursor ORDER BY id LIMIT 2 FOR UPDATE"
        } else {
            "SELECT id,plan,next,certificate FROM stream_cursor ORDER BY id LIMIT 2"
        };
        let rows = self.c.query(sql, &[]).await?;
        require(
            rows.len() == 1 && rows[0].try_get::<_, i32>(0)? == 1,
            "stream cursor singleton missing",
        )?;
        let next = rows[0].try_get::<_, i64>(2)?;
        require(
            next >= 0 && next as u64 <= self.partitions,
            "stream frontier bounds",
        )?;
        let cursor = Cursor {
            next: next as u64,
            plan: rows[0].try_get(1)?,
            certificate: rows[0].try_get(3)?,
        };
        if cursor.next == 0 {
            require(
                cursor.plan.is_empty() && cursor.certificate.is_empty(),
                "initial cursor not empty",
            )?;
        } else {
            require(
                cursor.plan == self.plan.as_str(),
                "cursor belongs to another approved plan",
            )?;
        }
        Ok(cursor)
    }
    pub(crate) async fn receipt(&self, i: u64) -> Result<Option<String>> {
        Ok(self
            .c
            .query_opt(
                "SELECT hash FROM stream_receipts WHERE id=$1",
                &[&(i as i64)],
            )
            .await?
            .map(|r| r.try_get(0))
            .transpose()?)
    }
    /// On error the owning caller must discard this session. A cursor-row lock
    /// serializes native publishers, including competing processes/journals.
    pub(crate) async fn apply(
        &mut self,
        meta: &Meta,
        records: &[Record],
        observe: &mut impl FnMut(Boundary, u64) -> Result<()>,
    ) -> Result<bool> {
        let units = self.mapping.prepare(records)?;
        require(units.len() as u64 == meta.rows, "partition rows differ")?;
        let ns = q(&self.mapping.target().namespace);
        self.c.batch_execute(&format!("BEGIN; LOCK TABLE {}, {ns}.stream_cursor, {ns}.stream_receipts IN ROW EXCLUSIVE MODE",relation(self.mapping.target()))).await?;
        self.admit().await?;
        let cursor = self.cursor(true).await?;
        let expected = cert(&self.plan, meta)?;
        if cursor.next > meta.index {
            require(
                self.receipt(meta.index).await?.as_deref() == Some(expected.as_str()),
                "replayed partition receipt conflict",
            )?;
            self.c.batch_execute("COMMIT").await?;
            return Ok(false);
        }
        require(cursor.next == meta.index, "noncontiguous partition attempt")?;
        require(
            self.receipt(meta.index).await?.is_none(),
            "receipt exists ahead of atomic cursor",
        )?;
        let fields = &self.mapping.target().fields;
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            relation(self.mapping.target()),
            fields
                .iter()
                .map(|f| q(&f.name))
                .collect::<Vec<_>>()
                .join(","),
            (1..=fields.len())
                .map(|i| format!("${i}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        let stmt = self.c.prepare(&sql).await?;
        for unit in &units {
            let params = parameters(unit, fields)?;
            let refs: Vec<&(dyn ToSql + Sync)> = params
                .iter()
                .map(|p| p.as_ref() as &(dyn ToSql + Sync))
                .collect();
            self.c.execute(&stmt, &refs).await?;
        }
        self.c
            .execute(
                "INSERT INTO stream_receipts(id,hash) VALUES($1,$2)",
                &[&(meta.index as i64), &expected.as_str()],
            )
            .await?;
        require(
            self.c
                .execute(
                    "UPDATE stream_cursor SET plan=$1,next=$2,certificate=$3 WHERE id=1",
                    &[
                        &self.plan.as_str(),
                        &((meta.index + 1) as i64),
                        &expected.as_str(),
                    ],
                )
                .await?
                == 1,
            "atomic cursor disappeared",
        )?;
        observe(Boundary::BeforeCommit, meta.index)?;
        self.c.batch_execute("COMMIT").await?;
        observe(Boundary::AfterCommit, meta.index)?;
        Ok(true)
    }
}
fn parameters(
    unit: &MappedUnit,
    fields: &[datamuta_plan::Field],
) -> Result<Vec<Box<dyn ToSql + Sync + Send>>> {
    fields
        .iter()
        .map(|f| {
            let value: Box<dyn ToSql + Sync + Send> = match unit
                .fields()
                .get(&f.name)
                .ok_or_else(|| err("mapped field missing"))?
            {
                Scalar::Bool(v) => Box::new(*v),
                Scalar::I32(v) => Box::new(*v),
                Scalar::I64(v) => Box::new(*v),
                Scalar::Utf8(v) => Box::new(v.clone()),
                Scalar::Null => match f.data_type {
                    DataType::Bool => Box::new(None::<bool>),
                    DataType::I32 => Box::new(None::<i32>),
                    DataType::I64 => Box::new(None::<i64>),
                    DataType::Utf8 => Box::new(None::<String>),
                },
            };
            Ok(value)
        })
        .collect()
}
