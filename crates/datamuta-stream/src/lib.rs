//! Experimental detached snapshot execution. Fast recovery trusts atomic native
//! history; explicit receipt audit is separate from fast resume and data audit.
#![forbid(unsafe_code)]
pub mod firestore;
pub mod mongo;
pub mod mysql;
pub mod postgres;
pub mod publisher;
pub mod spool;
use datamuta_kernel::{Fingerprint, canonical_hash};
use datamuta_plan::{Candidate, CompiledMapping, Schema};
use datamuta_postgres::Endpoint;
use serde::{Deserialize, Serialize};
use std::{fmt, io::Read, path::Path};
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug)]
pub struct Error(String);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for Error {}
pub(crate) fn err(e: impl fmt::Display) -> Error {
    Error(e.to_string())
}
pub(crate) fn require(ok: bool, message: &str) -> Result<()> {
    if ok { Ok(()) } else { Err(err(message)) }
}
macro_rules! from {
    ($t:ty) => {
        impl From<$t> for Error {
            fn from(e: $t) -> Self {
                err(e)
            }
        }
    };
}
from!(std::io::Error);
from!(serde_json::Error);
from!(datamuta_kernel::Error);
from!(datamuta_plan::Error);
from!(datamuta_postgres::Error);
from!(tokio_postgres::Error);
from!(sqlx::Error);
from!(mongodb::error::Error);
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetEngine {
    #[default]
    Postgres,
    Mysql,
    Mongo,
    Firestore,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceEngine {
    #[default]
    Postgres,
    Firestore,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub source_endpoint: Endpoint,
    #[serde(default)]
    pub source_engine: SourceEngine,
    pub target_endpoint: Endpoint,
    #[serde(default)]
    pub target_engine: TargetEngine,
    pub source: Schema,
    pub target: Schema,
    pub candidate: Candidate,
    pub limits: spool::Limits,
}
impl Config {
    pub fn compile(&self) -> Result<CompiledMapping> {
        require(self.version == 2, "configuration version")?;
        self.source_endpoint.validate()?;
        self.target_endpoint.validate()?;
        if self.target_engine != TargetEngine::Postgres {
            require(
                self.target_endpoint.lab_only && self.target_endpoint.verified.is_none(),
                "Non-PostgreSQL targets currently require the loopback lab endpoint profile",
            )?;
        }
        if self.source_engine == SourceEngine::Firestore {
            require(
                self.source_endpoint.lab_only && self.source_endpoint.verified.is_none(),
                "Firestore requires the loopback lab endpoint profile",
            )?;
        }
        self.limits.validate()?;
        require(
            self.source.namespace == self.source_endpoint.namespace
                && self.target.namespace == self.target_endpoint.namespace,
            "schema namespace must match its own endpoint",
        )?;
        let location = |endpoint: &Endpoint| {
            let (host, database) = endpoint
                .verified
                .as_ref()
                .map(|v| (v.host.as_str(), v.database.as_str()))
                .unwrap_or(("127.0.0.1", "datamuta"));
            (
                host.to_owned(),
                endpoint.port,
                database.to_owned(),
                endpoint.namespace.clone(),
            )
        };
        require(
            !matches!(
                (self.source_engine, self.target_engine),
                (SourceEngine::Postgres, TargetEngine::Postgres)
                    | (SourceEngine::Firestore, TargetEngine::Firestore)
            ) || location(&self.source_endpoint) != location(&self.target_endpoint)
                || self.source.relation != self.target.relation,
            "source and target resolve to the same configured relation",
        )?;
        for name in [&self.source.relation, &self.target.relation] {
            require(
                !["receipts", "progress", "stream_cursor", "stream_receipts"]
                    .contains(&name.as_str()),
                "reserved metadata relation",
            )?;
        }
        Ok(datamuta_plan::compile(
            &self.source,
            &self.target,
            &self.candidate,
        )?)
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub version: u32,
    pub config: Config,
    pub snapshot: spool::Manifest,
    pub implementation: Fingerprint,
}
impl Plan {
    pub fn new(config: Config, snapshot: spool::Manifest) -> Result<Self> {
        let p = Self {
            version: 2,
            config,
            snapshot,
            implementation: implementation(),
        };
        p.validate()?;
        Ok(p)
    }
    pub fn validate(&self) -> Result<CompiledMapping> {
        require(
            self.version == 2 && self.implementation == implementation(),
            "stream implementation/version changed",
        )?;
        self.snapshot.validate()?;
        require(
            self.config.limits == self.snapshot.limits,
            "spool policy differs",
        )?;
        self.config.compile()
    }
    pub fn fingerprint(&self) -> Result<Fingerprint> {
        let m = self.validate()?;
        Ok(canonical_hash(&(
            "datamuta/stream-plan/v2",
            &self.implementation,
            (
                self.config.source_engine,
                self.config.source_endpoint.receipt_domain()?,
            ),
            (
                self.config.target_engine,
                self.config.target_endpoint.receipt_domain()?,
            ),
            m.contract()?,
            &self.snapshot,
        ))?)
    }
    pub fn approve(&self, expected: &Fingerprint) -> Result<()> {
        require(
            self.fingerprint()? == *expected,
            "exact stream plan approval required",
        )
    }
}
pub fn implementation() -> Fingerprint {
    Fingerprint::of(
        concat!(
            include_str!("lib.rs"),
            include_str!("spool.rs"),
            include_str!("postgres.rs"),
            include_str!("publisher.rs"),
            include_str!("mysql.rs"),
            include_str!("mongo.rs"),
            include_str!("firestore.rs"),
            include_str!("../Cargo.toml"),
            include_str!("../Cargo.lock"),
            include_str!("../../datamuta-postgres/src/lib.rs"),
            include_str!("../../datamuta-postgres/src/connection.rs"),
            include_str!("../../datamuta-postgres/src/admission.rs"),
            include_str!("../../datamuta-postgres/Cargo.toml"),
            include_str!("../../datamuta-postgres/Cargo.lock"),
            include_str!("../../datamuta-plan/src/mapping.rs"),
            include_str!("../../datamuta-plan/src/schema.rs"),
            include_str!("../../datamuta-kernel/src/lib.rs")
        )
        .as_bytes(),
    )
}
pub fn load<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let mut data = Vec::new();
    std::fs::File::open(path)?
        .take(131073)
        .read_to_end(&mut data)?;
    require(data.len() <= 131072, "configuration file bound")?;
    Ok(serde_json::from_slice(&data)?)
}
pub fn save<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    f.write_all(&serde_json::to_vec_pretty(value)?)?;
    f.sync_all()?;
    Ok(())
}

/// Capture dispatch is explicit and included in exact-plan identity.
pub async fn capture(
    config: Config,
    path: &Path,
    observe: impl FnMut(u64) -> Result<()>,
) -> Result<Plan> {
    match config.source_engine {
        SourceEngine::Postgres => postgres::capture(config, path, observe).await,
        SourceEngine::Firestore => firestore::capture(config, path, observe).await,
    }
}
