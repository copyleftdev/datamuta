//! Shared authenticated partition protocol; adapters own native atomic commits.
use crate::*;
use datamuta_plan::Record;
use spool::{Meta, Reader};
use std::time::Instant;
#[derive(Clone, Debug, Serialize)]
pub struct Cursor {
    pub next: u64,
    pub plan: String,
    pub certificate: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boundary {
    BeforePartition,
    BeforeCommit,
    AfterCommit,
}
pub(crate) fn cert(plan: &Fingerprint, m: &Meta) -> Result<Fingerprint> {
    Ok(canonical_hash(&("datamuta/partition-commit/v1", plan, m))?)
}
pub(crate) fn check_cursor(c: Cursor, plan: &Fingerprint, partitions: u64) -> Result<Cursor> {
    require(c.next <= partitions, "stream frontier bounds")?;
    if c.next == 0 {
        require(
            c.plan.is_empty() && c.certificate.is_empty(),
            "initial cursor not empty",
        )?;
    } else {
        require(
            c.plan == plan.as_str(),
            "cursor belongs to another approved plan",
        )?;
    }
    Ok(c)
}
pub async fn admit_target(config: &Config) -> Result<()> {
    let mapping = config.compile()?;
    match config.target_engine {
        TargetEngine::Postgres => {
            let c = datamuta_postgres::connect(&config.target_endpoint).await?;
            mapping.validate_observed(
                mapping.source(),
                &datamuta_postgres::describe(&c, &config.target.namespace, &config.target.relation)
                    .await?,
            )?;
            Ok(())
        }
        TargetEngine::Mysql => mysql::admit_config(config).await,
        TargetEngine::Mongo => mongo::admit_config(config).await,
        TargetEngine::Firestore => firestore::admit_config(config).await,
    }
}
// Static dispatch keeps the native session owned by the executor. On any error
// it is dropped, never returned to a pool with an uncertain transaction.
enum Destination {
    Postgres(postgres::Destination),
    Mysql(mysql::Destination),
    Mongo(mongo::Destination),
    Firestore(firestore::Destination),
}
impl Destination {
    async fn open(p: &Plan) -> Result<Self> {
        p.validate()?;
        Ok(match p.config.target_engine {
            TargetEngine::Postgres => Self::Postgres(postgres::Destination::open(p).await?),
            TargetEngine::Mysql => Self::Mysql(mysql::Destination::open(p).await?),
            TargetEngine::Mongo => Self::Mongo(mongo::Destination::open(p).await?),
            TargetEngine::Firestore => Self::Firestore(firestore::Destination::open(p).await?),
        })
    }
    async fn cursor(&mut self) -> Result<Cursor> {
        match self {
            Self::Postgres(d) => d.cursor(false).await,
            Self::Mysql(d) => d.cursor(false).await,
            Self::Mongo(d) => d.cursor().await,
            Self::Firestore(d) => d.cursor().await,
        }
    }
    async fn receipt(&mut self, i: u64) -> Result<Option<String>> {
        match self {
            Self::Postgres(d) => d.receipt(i).await,
            Self::Mysql(d) => d.receipt(i).await,
            Self::Mongo(d) => d.receipt(i).await,
            Self::Firestore(d) => d.receipt(i).await,
        }
    }
    async fn apply(
        &mut self,
        m: &Meta,
        r: &[Record],
        o: &mut impl FnMut(Boundary, u64) -> Result<()>,
    ) -> Result<bool> {
        match self {
            Self::Postgres(d) => d.apply(m, r, o).await,
            Self::Mysql(d) => d.apply(m, r, o).await,
            Self::Mongo(d) => d.apply(m, r, o).await,
            Self::Firestore(d) => d.apply(m, r, o).await,
        }
    }
    async fn resume(&mut self, p: &Plan, spool: &mut Reader) -> Result<Cursor> {
        let c = self.cursor().await?;
        if c.next > 0 {
            let m = spool.meta(c.next - 1)?;
            let expected = cert(&p.fingerprint()?, &m)?;
            require(
                c.certificate == expected.as_str()
                    && self.receipt(m.index).await?.as_deref() == Some(expected.as_str()),
                "last atomic commit certificate missing or changed",
            )?;
        }
        Ok(c)
    }
}
#[derive(Serialize)]
pub struct Report {
    pub start_partition: u64,
    pub end_partition: u64,
    pub partitions_applied: u64,
    pub receipt_checks: u64,
    pub spool: spool::Metrics,
    pub elapsed_seconds: f64,
}
pub async fn status(plan: &Plan, path: &Path) -> Result<Report> {
    let time = Instant::now();
    let mut spool = Reader::open(path, &plan.snapshot)?;
    let mut target = Destination::open(plan).await?;
    let c = target.resume(plan, &mut spool).await?;
    Ok(Report {
        start_partition: c.next,
        end_partition: c.next,
        partitions_applied: 0,
        receipt_checks: u64::from(c.next > 0),
        spool: spool.metrics,
        elapsed_seconds: time.elapsed().as_secs_f64(),
    })
}
pub async fn run(
    plan: &Plan,
    path: &Path,
    approved: &Fingerprint,
    mut observe: impl FnMut(Boundary, u64) -> Result<()>,
) -> Result<Report> {
    plan.approve(approved)?;
    let time = Instant::now();
    let mut spool = Reader::open(path, &plan.snapshot)?;
    let mut target = Destination::open(plan).await?;
    let state = target.resume(plan, &mut spool).await?;
    let mut applied = 0;
    for i in state.next..plan.snapshot.partitions {
        observe(Boundary::BeforePartition, i)?;
        let meta = spool.meta(i)?;
        let records = spool.records(&meta)?;
        if target.apply(&meta, &records, &mut observe).await? {
            applied += 1;
        }
    }
    Ok(Report {
        start_partition: state.next,
        end_partition: plan.snapshot.partitions,
        partitions_applied: applied,
        receipt_checks: u64::from(state.next > 0) + plan.snapshot.partitions - state.next,
        spool: spool.metrics,
        elapsed_seconds: time.elapsed().as_secs_f64(),
    })
}
/// Receipt history audit; native value verification is a separate operation.
pub async fn audit(plan: &Plan, path: &Path) -> Result<Report> {
    let time = Instant::now();
    let mut spool = Reader::open(path, &plan.snapshot)?;
    let mut target = Destination::open(plan).await?;
    let state = target.resume(plan, &mut spool).await?;
    let mut end = 0;
    let fingerprint = plan.fingerprint()?;
    for i in 0..state.next {
        let meta = spool.meta(i)?;
        require(meta.first == end, "noncontiguous source partition metadata")?;
        end += meta.rows;
        let expected = cert(&fingerprint, &meta)?;
        require(
            target.receipt(i).await?.as_deref() == Some(expected.as_str()),
            "historical receipt missing or changed",
        )?;
    }
    if state.next == plan.snapshot.partitions {
        require(end == plan.snapshot.rows, "snapshot row count differs")?;
    }
    Ok(Report {
        start_partition: state.next,
        end_partition: state.next,
        partitions_applied: 0,
        receipt_checks: state.next + u64::from(state.next > 0),
        spool: spool.metrics,
        elapsed_seconds: time.elapsed().as_secs_f64(),
    })
}
