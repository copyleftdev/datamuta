//! Bounded PostgreSQL flat mapping adapter with explicit lab and verified profiles.
#![forbid(unsafe_code)]
use datamuta_kernel::{self as kernel, MigrationUnit, Target};
use datamuta_plan::{CompiledMapping, DataType, Field, MappedUnit, Record, Scalar, Schema};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt, ops::Deref, time::Duration};
use tokio_postgres::{Client, types::ToSql};
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug)]
pub struct Error(String);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for Error {}
impl From<kernel::Error> for Error {
    fn from(e: kernel::Error) -> Self {
        err(e)
    }
}
fn err(e: impl fmt::Display) -> Error {
    Error(e.to_string())
}
fn invalid(s: &str) -> Error {
    Error(s.into())
}
fn require(ok: bool, s: &str) -> Result<()> {
    if ok { Ok(()) } else { Err(invalid(s)) }
}
mod admission;
mod connection;
pub use connection::{Endpoint, Session, Verified, connect};
fn implementation() -> kernel::Fingerprint {
    kernel::Fingerprint::of(
        concat!(
            include_str!("lib.rs"),
            include_str!("connection.rs"),
            include_str!("admission.rs"),
            include_str!("../Cargo.toml"),
            include_str!("../Cargo.lock"),
            include_str!("../../datamuta-kernel/src/execute.rs"),
            include_str!("../../datamuta-kernel/src/plan.rs"),
            include_str!("../../datamuta-plan/src/schema.rs"),
            include_str!("../../datamuta-plan/src/mapping.rs")
        )
        .as_bytes(),
    )
}
pub fn q(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
pub fn relation(s: &Schema) -> String {
    format!("{}.{}", q(&s.namespace), q(&s.relation))
}
/// Native catalog evidence, not sample-based inference. Only the stated plain-table
/// subset is admitted; unsupported semantics must not disappear from descriptors.
pub async fn describe(c: &Client, namespace: &str, name: &str) -> Result<Schema> {
    let rows = c
        .query(
            r#"SELECT c.oid::bigint,
        c.relkind <> 'r' OR c.relpersistence <> 'p' OR c.relrowsecurity OR c.relispartition
        OR EXISTS(SELECT 1 FROM pg_inherits WHERE inhrelid=c.oid OR inhparent=c.oid)
        OR EXISTS(SELECT 1 FROM pg_trigger WHERE tgrelid=c.oid AND NOT tgisinternal)
        OR EXISTS(SELECT 1 FROM pg_rewrite WHERE ev_class=c.oid)
        OR EXISTS(SELECT 1 FROM pg_constraint WHERE conrelid=c.oid AND
            (contype <> 'p' OR condeferrable OR NOT convalidated))
        OR EXISTS(SELECT 1 FROM pg_index WHERE indrelid=c.oid AND indisunique AND NOT indisprimary)
        FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
        WHERE n.nspname=$1 AND c.relname=$2"#,
            &[&namespace, &name],
        )
        .await
        .map_err(err)?;
    require(rows.len() == 1, "relation missing or ambiguous")?;
    require(
        !rows[0].try_get::<_, bool>(1).map_err(err)?,
        "unsupported relation semantics",
    )?;
    let oid: i64 = rows[0].try_get(0).map_err(err)?;
    let columns=c.query("SELECT attname,atttypid::int4,attnotnull,attnum,atthasdef OR attidentity<>'' OR attgenerated<>'' FROM pg_attribute WHERE attrelid=$1::bigint::oid AND attnum>0 AND NOT attisdropped ORDER BY attnum LIMIT 65", &[&oid]).await.map_err(err)?;
    require(
        !columns.is_empty() && columns.len() <= 64,
        "schema field bound",
    )?;
    let mut fields = Vec::new();
    let mut names = BTreeMap::new();
    for col in columns {
        require(
            !col.try_get::<_, bool>(4).map_err(err)?,
            "defaults, identity and generated columns are unsupported",
        )?;
        let name: String = col.try_get(0).map_err(err)?;
        let data_type = match col.try_get::<_, i32>(1).map_err(err)? {
            16 => DataType::Bool,
            20 => DataType::I64,
            23 => DataType::I32,
            25 => DataType::Utf8,
            _ => return Err(invalid("unsupported native column type")),
        };
        names.insert(col.try_get::<_, i16>(3).map_err(err)?, name.clone());
        fields.push(Field {
            name,
            data_type,
            nullable: !col.try_get::<_, bool>(2).map_err(err)?,
        });
    }
    let keys = c
        .query(
            "SELECT conkey FROM pg_constraint WHERE conrelid=$1::bigint::oid AND contype='p'",
            &[&oid],
        )
        .await
        .map_err(err)?;
    require(keys.len() == 1, "declared primary key required")?;
    let primary_key = keys[0]
        .try_get::<_, Vec<i16>>(0)
        .map_err(err)?
        .iter()
        .map(|k| {
            names
                .get(k)
                .cloned()
                .ok_or_else(|| invalid("unknown native key field"))
        })
        .collect::<Result<_>>()?;
    let schema = Schema {
        version: 1,
        namespace: namespace.into(),
        relation: name.into(),
        fields,
        primary_key,
    };
    schema.validate().map_err(err)?;
    Ok(schema)
}
pub async fn read(c: &Client, s: &Schema) -> Result<Vec<Record>> {
    s.validate().map_err(err)?;
    let sql = format!(
        "SELECT {} FROM {} ORDER BY {} LIMIT 4097",
        s.fields
            .iter()
            .map(|f| q(&f.name))
            .collect::<Vec<_>>()
            .join(","),
        relation(s),
        s.primary_key
            .iter()
            .map(|k| q(k))
            .collect::<Vec<_>>()
            .join(",")
    );
    let rows = c.query(&sql, &[]).await.map_err(err)?;
    require(rows.len() <= 4096, "source record bound")?;
    rows.iter()
        .map(|row| {
            s.fields
                .iter()
                .enumerate()
                .map(|(n, f)| {
                    let value = match f.data_type {
                        DataType::Bool => row
                            .try_get::<_, Option<bool>>(n)
                            .map_err(err)?
                            .map(Scalar::Bool),
                        DataType::I32 => row
                            .try_get::<_, Option<i32>>(n)
                            .map_err(err)?
                            .map(Scalar::I32),
                        DataType::I64 => row
                            .try_get::<_, Option<i64>>(n)
                            .map_err(err)?
                            .map(Scalar::I64),
                        DataType::Utf8 => row
                            .try_get::<_, Option<String>>(n)
                            .map_err(err)?
                            .map(Scalar::Utf8),
                    };
                    Ok((f.name.clone(), value.unwrap_or(Scalar::Null)))
                })
                .collect()
        })
        .collect()
}

pub struct Destination {
    client: Session,
    mapping: CompiledMapping,
    descriptor: kernel::TargetDescriptor,
}
impl Destination {
    pub async fn new(endpoint: &Endpoint, mapping: CompiledMapping) -> Result<Self> {
        endpoint.validate()?;
        require(
            mapping.source().namespace == endpoint.namespace
                && mapping.target().namespace == endpoint.namespace,
            "mapping outside admitted namespace",
        )?;
        require(
            !["receipts", "progress"].contains(&mapping.source().relation.as_str())
                && !["receipts", "progress"].contains(&mapping.target().relation.as_str()),
            "mapping overlaps agent metadata",
        )?;
        let descriptor = kernel::TargetDescriptor {
            adapter_id: "postgres/compiled-flat-v2".into(),
            adapter_version: 1,
            implementation_sha256: implementation(),
            receipt_domain: endpoint.receipt_domain()?,
            publication: kernel::Publication::AtomicTransaction,
            durable_receipt_lookup: true,
            contiguous_checkpoints: true,
            max_batch_units: 64,
            max_unit_bytes: 65_536,
            contracts: vec![mapping.contract().map_err(err)?],
        };
        let client = connect(endpoint).await?;
        admission::mapping(&client, &mapping).await?;
        Ok(Self {
            client,
            mapping,
            descriptor,
        })
    }
}
impl Target<MappedUnit> for Destination {
    type Error = Error;
    fn descriptor(&self) -> kernel::TargetDescriptor {
        self.descriptor.clone()
    }
    async fn frontier(&mut self) -> Result<u64> {
        let n: i64 = self
            .client
            .query_one("SELECT frontier FROM progress WHERE id=1", &[])
            .await
            .map_err(err)?
            .try_get(0)
            .map_err(err)?;
        u64::try_from(n).map_err(err)
    }
    async fn begin(&mut self) -> Result<()> {
        self.client
            .batch_execute(&format!(
                "BEGIN; LOCK TABLE {}, {}.receipts, {}.progress IN ROW EXCLUSIVE MODE",
                relation(self.mapping.target()),
                q(&self.mapping.target().namespace),
                q(&self.mapping.target().namespace)
            ))
            .await
            .map_err(err)?;
        admission::metadata(&self.client, &self.mapping.target().namespace).await?;
        // Hold a DDL-conflicting lock from descriptor check through commit.
        let observed = describe(
            &self.client,
            &self.mapping.target().namespace,
            &self.mapping.target().relation,
        )
        .await?;
        self.mapping
            .validate_observed(self.mapping.source(), &observed)
            .map_err(err)
    }
    async fn receipt(&mut self, id: i64) -> Result<Option<String>> {
        self.client
            .query_opt("SELECT hash FROM receipts WHERE id=$1", &[&id])
            .await
            .map_err(err)?
            .map(|r| r.try_get(0).map_err(err))
            .transpose()
    }
    async fn insert(&mut self, unit: &MappedUnit, hash: &str) -> Result<()> {
        require(
            *unit.mapping() == self.descriptor.contracts[0].sha256,
            "unit belongs to another mapping",
        )?;
        let fields = &self.mapping.target().fields;
        let mut values: Vec<Box<dyn ToSql + Sync + Send>> = Vec::new();
        for field in fields {
            let v = unit
                .fields()
                .get(&field.name)
                .ok_or_else(|| invalid("missing compiled field"))?;
            let value: Box<dyn ToSql + Sync + Send> = match v {
                Scalar::Bool(v) => Box::new(*v),
                Scalar::I32(v) => Box::new(*v),
                Scalar::I64(v) => Box::new(*v),
                Scalar::Utf8(v) => Box::new(v.clone()),
                Scalar::Null => match field.data_type {
                    DataType::Bool => Box::new(None::<bool>),
                    DataType::I32 => Box::new(None::<i32>),
                    DataType::I64 => Box::new(None::<i64>),
                    DataType::Utf8 => Box::new(None::<String>),
                },
            };
            values.push(value);
        }
        let params: Vec<&(dyn ToSql + Sync)> = values
            .iter()
            .map(|v| v.as_ref() as &(dyn ToSql + Sync))
            .collect();
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
        self.client.execute(&sql, &params).await.map_err(err)?;
        self.client
            .execute(
                "INSERT INTO receipts VALUES($1,$2)",
                &[&(unit.ordinal()? as i64), &hash],
            )
            .await
            .map_err(err)?;
        Ok(())
    }
    async fn commit(&mut self) -> Result<()> {
        self.client.batch_execute("COMMIT").await.map_err(err)
    }
    async fn checkpoint(&mut self, end: u64) -> Result<()> {
        let affected = self
            .client
            .execute(
                "UPDATE progress SET frontier=GREATEST(frontier,$1) WHERE id=1",
                &[&(end as i64)],
            )
            .await
            .map_err(err)?;
        require(affected == 1, "checkpoint singleton missing")
    }
}
