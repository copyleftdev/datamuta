//! Loopback-only MySQL 8.4 laboratory publisher. InnoDB effects and recovery
//! metadata commit together; no RSA authentication fallback or implicit retries.
use crate::{
    publisher::{Boundary, Cursor, cert, check_cursor},
    *,
};
use datamuta_plan::{DataType, Field, Record, Scalar};
use spool::Meta;
use sqlx::{
    Connection, MySqlConnection, Row,
    mysql::{MySqlConnectOptions, MySqlSslMode},
};
// Dynamic SQL contains only validated, quoted identifiers and fixed syntax.
// All record values and receipt values use bind parameters.
fn q(s: &str) -> String {
    format!("`{}`", s.replace('`', "``"))
}
fn table(s: &Schema) -> String {
    format!("{}.{}", q(&s.namespace), q(&s.relation))
}
pub async fn connect(endpoint: &Endpoint) -> Result<MySqlConnection> {
    endpoint.validate()?;
    require(
        endpoint.lab_only && endpoint.verified.is_none(),
        "MySQL requires loopback lab profile",
    )?;
    // Lab certificates are ephemeral/self-signed. Require encryption to avoid
    // RSA authentication, but do not claim server identity verification.
    let options = MySqlConnectOptions::new()
        .host("127.0.0.1")
        .port(endpoint.port)
        .username("root")
        .password("")
        .ssl_mode(MySqlSslMode::Required)
        .charset("utf8mb4")
        .collation("utf8mb4_bin");
    let mut c = MySqlConnection::connect_with(&options).await?;
    let row = sqlx::query("SELECT VERSION(),CAST(@@innodb_flush_log_at_trx_commit AS SIGNED),CAST(@@read_only AS SIGNED)")
        .fetch_one(&mut c)
        .await?;
    require(
        row.try_get::<String, _>(0)?.starts_with("8.4.")
            && row.try_get::<i64, _>(1)? == 1
            && row.try_get::<i64, _>(2)? == 0,
        "MySQL version/durability/read-only policy",
    )?;
    sqlx::raw_sql("SET SESSION sql_mode='STRICT_ALL_TABLES,NO_ENGINE_SUBSTITUTION', innodb_lock_wait_timeout=5").execute(&mut c).await?;
    Ok(c)
}
async fn describe(c: &mut MySqlConnection, expected: &Schema) -> Result<()> {
    expected.validate()?;
    let ns = &expected.namespace;
    let name = &expected.relation;
    let tables=sqlx::query("SELECT ENGINE,TABLE_TYPE,TABLE_COLLATION,CREATE_OPTIONS FROM information_schema.TABLES WHERE TABLE_SCHEMA=? AND TABLE_NAME=?").bind(ns).bind(name).fetch_all(&mut *c).await?;
    require(tables.len() == 1, "MySQL target table missing")?;
    let t = &tables[0];
    require(
        t.try_get::<String, _>(0)? == "InnoDB"
            && t.try_get::<String, _>(1)? == "BASE TABLE"
            && t.try_get::<String, _>(2)? == "utf8mb4_bin"
            && t.try_get::<String, _>(3)?.is_empty(),
        "MySQL table engine/options/collation outside admitted profile",
    )?;
    let rows=sqlx::query("SELECT COLUMN_NAME,COLUMN_TYPE,IS_NULLABLE,COLUMN_DEFAULT,EXTRA,COLLATION_NAME FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=? AND TABLE_NAME=? ORDER BY ORDINAL_POSITION LIMIT 65").bind(ns).bind(name).fetch_all(&mut *c).await?;
    require(
        rows.len() == expected.fields.len(),
        "MySQL schema field count differs",
    )?;
    let mut fields = Vec::new();
    for r in rows {
        let ty: String = r.try_get(1)?;
        let data_type = match ty.as_str() {
            "tinyint(1)" => DataType::Bool,
            "int" => DataType::I32,
            "bigint" => DataType::I64,
            "longtext" => DataType::Utf8,
            _ => return Err(err("unsupported MySQL column type")),
        };
        require(
            r.try_get::<Option<String>, _>(3)?.is_none() && r.try_get::<String, _>(4)?.is_empty(),
            "MySQL defaults/generated/auto columns refused",
        )?;
        if data_type == DataType::Utf8 {
            require(
                r.try_get::<Option<String>, _>(5)?.as_deref() == Some("utf8mb4_bin"),
                "MySQL text collation mismatch",
            )?;
        }
        fields.push(Field {
            name: r.try_get(0)?,
            data_type,
            nullable: r.try_get::<String, _>(2)? == "YES",
        });
    }
    let indexes=sqlx::query("SELECT INDEX_NAME,COLUMN_NAME,SUB_PART,IS_VISIBLE FROM information_schema.STATISTICS WHERE TABLE_SCHEMA=? AND TABLE_NAME=? ORDER BY SEQ_IN_INDEX LIMIT 65").bind(ns).bind(name).fetch_all(&mut *c).await?;
    let mut keys = Vec::new();
    for r in indexes {
        require(
            r.try_get::<String, _>(0)? == "PRIMARY"
                && r.try_get::<Option<i64>, _>(2)?.is_none()
                && r.try_get::<String, _>(3)? == "YES",
            "MySQL additional/partial/invisible indexes refused",
        )?;
        keys.push(r.try_get::<String, _>(1)?);
    }
    let observed = Schema {
        version: 1,
        namespace: ns.clone(),
        relation: name.clone(),
        fields,
        primary_key: keys,
    };
    require(
        observed.fingerprint()? == expected.fingerprint()?,
        "MySQL observed schema differs from compiled mapping",
    )?;
    let forbidden:i64=sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA=? AND EVENT_OBJECT_TABLE=?) + (SELECT COUNT(*) FROM information_schema.TABLE_CONSTRAINTS WHERE TABLE_SCHEMA=? AND TABLE_NAME=? AND CONSTRAINT_TYPE <> 'PRIMARY KEY')").bind(ns).bind(name).bind(ns).bind(name).fetch_one(&mut *c).await?;
    require(
        forbidden == 0,
        "MySQL triggers or additional constraints refused",
    )
}
fn metadata(namespace: &str) -> [Schema; 2] {
    let make = |relation: &str, fields: Vec<(&str, DataType)>| Schema {
        version: 1,
        namespace: namespace.into(),
        relation: relation.into(),
        primary_key: vec!["id".into()],
        fields: fields
            .into_iter()
            .map(|(name, data_type)| Field {
                name: name.into(),
                data_type,
                nullable: false,
            })
            .collect(),
    };
    [
        make(
            "stream_cursor",
            vec![
                ("id", DataType::I32),
                ("plan", DataType::Utf8),
                ("next", DataType::I64),
                ("certificate", DataType::Utf8),
            ],
        ),
        make(
            "stream_receipts",
            vec![("id", DataType::I64), ("hash", DataType::Utf8)],
        ),
    ]
}
async fn admit(c: &mut MySqlConnection, s: &Schema) -> Result<()> {
    describe(c, s).await?;
    for m in metadata(&s.namespace) {
        describe(c, &m).await?;
    }
    Ok(())
}
pub(crate) async fn admit_config(config: &Config) -> Result<()> {
    let mut c = connect(&config.target_endpoint).await?;
    admit(&mut c, &config.target).await
}
pub struct Destination {
    c: MySqlConnection,
    mapping: CompiledMapping,
    plan: Fingerprint,
    partitions: u64,
}
impl Destination {
    pub async fn open(p: &Plan) -> Result<Self> {
        let mapping = p.validate()?;
        let mut c = connect(&p.config.target_endpoint).await?;
        admit(&mut c, mapping.target()).await?;
        Ok(Self {
            c,
            mapping,
            plan: p.fingerprint()?,
            partitions: p.snapshot.partitions,
        })
    }
    fn metadata_table(&self, name: &str) -> String {
        format!("{}.{}", q(&self.mapping.target().namespace), q(name))
    }
    pub(crate) async fn cursor(&mut self, lock: bool) -> Result<Cursor> {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT id,plan,next,certificate FROM {} ORDER BY id LIMIT 2 {}",
            self.metadata_table("stream_cursor"),
            if lock { "FOR UPDATE" } else { "" }
        )))
        .fetch_all(&mut self.c)
        .await?;
        require(
            rows.len() == 1 && rows[0].try_get::<i32, _>(0)? == 1,
            "stream cursor singleton missing",
        )?;
        let r = &rows[0];
        let next: i64 = r.try_get(2)?;
        require(next >= 0, "stream frontier bounds")?;
        check_cursor(
            Cursor {
                next: next as u64,
                plan: r.try_get(1)?,
                certificate: r.try_get(3)?,
            },
            &self.plan,
            self.partitions,
        )
    }
    pub(crate) async fn receipt(&mut self, i: u64) -> Result<Option<String>> {
        Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT hash FROM {} WHERE id=?",
            self.metadata_table("stream_receipts")
        )))
        .bind(i as i64)
        .fetch_optional(&mut self.c)
        .await?)
    }
    pub(crate) async fn apply(
        &mut self,
        m: &Meta,
        records: &[Record],
        observe: &mut impl FnMut(Boundary, u64) -> Result<()>,
    ) -> Result<bool> {
        let units = self.mapping.prepare(records)?;
        require(units.len() as u64 == m.rows, "partition rows differ")?;
        sqlx::raw_sql("START TRANSACTION")
            .execute(&mut self.c)
            .await?;
        // Touch all three relations before introspection; transaction-duration
        // metadata locks prevent concurrent DDL from changing the admitted shape.
        for name in [
            table(self.mapping.target()),
            self.metadata_table("stream_cursor"),
            self.metadata_table("stream_receipts"),
        ] {
            sqlx::query(sqlx::AssertSqlSafe(format!("SELECT 1 FROM {name} LIMIT 0")))
                .fetch_all(&mut self.c)
                .await?;
        }
        admit(&mut self.c, self.mapping.target()).await?;
        let cursor = self.cursor(true).await?;
        let expected = cert(&self.plan, m)?;
        if cursor.next > m.index {
            require(
                self.receipt(m.index).await?.as_deref() == Some(expected.as_str()),
                "replayed partition receipt conflict",
            )?;
            sqlx::raw_sql("COMMIT").execute(&mut self.c).await?;
            return Ok(false);
        }
        require(cursor.next == m.index, "noncontiguous partition attempt")?;
        require(
            self.receipt(m.index).await?.is_none(),
            "receipt exists ahead of atomic cursor",
        )?;
        let fields = &self.mapping.target().fields;
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            table(self.mapping.target()),
            fields
                .iter()
                .map(|f| q(&f.name))
                .collect::<Vec<_>>()
                .join(","),
            vec!["?"; fields.len()].join(",")
        );
        for u in &units {
            let mut query = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()));
            for f in fields {
                query = match u
                    .fields()
                    .get(&f.name)
                    .ok_or_else(|| err("mapped field missing"))?
                {
                    Scalar::Bool(v) => query.bind(*v),
                    Scalar::I32(v) => query.bind(*v),
                    Scalar::I64(v) => query.bind(*v),
                    Scalar::Utf8(v) => query.bind(v),
                    Scalar::Null => query.bind(None::<String>),
                };
            }
            query.execute(&mut self.c).await?;
        }
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO {} (id,hash) VALUES (?,?)",
            self.metadata_table("stream_receipts")
        )))
        .bind(m.index as i64)
        .bind(expected.as_str())
        .execute(&mut self.c)
        .await?;
        let result = sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {} SET plan=?,next=?,certificate=? WHERE id=1",
            self.metadata_table("stream_cursor")
        )))
        .bind(self.plan.as_str())
        .bind((m.index + 1) as i64)
        .bind(expected.as_str())
        .execute(&mut self.c)
        .await?;
        require(result.rows_affected() == 1, "atomic cursor disappeared")?;
        observe(Boundary::BeforeCommit, m.index)?;
        sqlx::raw_sql("COMMIT").execute(&mut self.c).await?;
        observe(Boundary::AfterCommit, m.index)?;
        Ok(true)
    }
}
