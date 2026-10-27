use super::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;

struct Blob {
    ordinal: u64,
    bytes: Vec<u8>,
}
impl MigrationUnit for Blob {
    fn ordinal(&self) -> Result<u64> {
        Ok(self.ordinal)
    }
    fn canonical_bytes(&self) -> Result<Vec<u8>> {
        Ok(self.bytes.clone())
    }
}
#[derive(Default)]
struct Binary(Memory);
impl Target<Blob> for Binary {
    type Error = Error;
    fn descriptor(&self) -> TargetDescriptor {
        self.0.descriptor()
    }
    async fn frontier(&mut self) -> Result<u64> {
        self.0.frontier().await
    }
    async fn begin(&mut self) -> Result<()> {
        self.0.begin().await
    }
    async fn receipt(&mut self, id: i64) -> Result<Option<String>> {
        self.0.receipt(id).await
    }
    async fn insert(&mut self, row: &Blob, hash: &str) -> Result<()> {
        self.0
            .insert(
                &json!({"id":row.ordinal.to_string(),"bytes":row.bytes}),
                hash,
            )
            .await
    }
    async fn commit(&mut self) -> Result<()> {
        self.0.commit().await
    }
    async fn checkpoint(&mut self, end: u64) -> Result<()> {
        self.0.checkpoint(end).await
    }
}
#[tokio::test]
async fn non_json_units_bind_identical_payloads_to_distinct_ordinals() -> Result<()> {
    let rows = vec![
        Blob {
            ordinal: 0,
            bytes: vec![0, 255],
        },
        Blob {
            ordinal: 1,
            bytes: vec![0, 255],
        },
    ];
    let mut t = Binary::default();
    let caps = t.descriptor();
    let source = SourceDescriptor {
        adapter_id: "binary/source".into(),
        mode: SourceMode::FrozenSnapshot,
        snapshot_sha256: Fingerprint::of(b"frozen-binary"),
    };
    let p = MigrationPlan::new(
        source.clone(),
        caps.clone(),
        caps.contracts[0].clone(),
        &rows,
        1,
    )?;
    execute(
        &mut t,
        p.validate(&source, &caps, &rows)?,
        Replay::Resume,
        |_, _| Ok(()),
    )
    .await?;
    assert_ne!(t.0.durable[&0].1, t.0.durable[&1].1);
    execute(
        &mut t,
        p.validate(&source, &caps, &rows)?,
        Replay::Full,
        |_, _| Ok(()),
    )
    .await?;
    assert_eq!(t.0.effects, 2);
    Ok(())
}
#[tokio::test]
async fn invalid_checkpoint_boundaries_are_refused_before_begin() -> Result<()> {
    let rows = rows(8);
    let p = plan(&rows, &descriptor(), 2)?;
    for (frontier, error) in [
        (3, Error::Bounds("checkpoint batch boundary")),
        (9, Error::FrontierOutOfBounds(9)),
    ] {
        let mut t = Memory {
            frontier,
            ..Memory::default()
        };
        let valid = p.validate(&source(&rows)?, &t.descriptor, &rows)?;
        assert_eq!(
            execute(&mut t, valid, Replay::Resume, |_, _| Ok(())).await,
            Err(error)
        );
        assert_eq!(t.io, 1);
        assert_eq!(t.effects, 0);
    }
    Ok(())
}

fn descriptor() -> TargetDescriptor {
    TargetDescriptor {
        adapter_id: "memory/test".into(),
        adapter_version: 1,
        implementation_sha256: Fingerprint::of(b"implementation"),
        receipt_domain: "isolated-test".into(),
        publication: Publication::AtomicTransaction,
        durable_receipt_lookup: true,
        contiguous_checkpoints: true,
        max_batch_units: 64,
        max_unit_bytes: 65_536,
        contracts: vec![Contract {
            name: "test-v1".into(),
            version: 1,
            sha256: Fingerprint::of(b"mapping-v1"),
        }],
    }
}
fn rows(n: u64) -> Vec<Value> {
    (0..n)
        .map(|i| json!({"id":i.to_string(),"value":i}))
        .collect()
}
fn source(rows: &[Value]) -> Result<SourceDescriptor> {
    Ok(SourceDescriptor {
        adapter_id: "memory/source".into(),
        mode: SourceMode::FrozenSnapshot,
        snapshot_sha256: canonical_hash(&rows)?,
    })
}
fn plan(rows: &[Value], target: &TargetDescriptor, batch: u64) -> Result<MigrationPlan> {
    MigrationPlan::new(
        source(rows)?,
        target.clone(),
        target.contracts[0].clone(),
        rows,
        batch,
    )
}
struct Memory {
    descriptor: TargetDescriptor,
    durable: BTreeMap<i64, (Value, String)>,
    pending: BTreeMap<i64, (Value, String)>,
    frontier: u64,
    effects: usize,
    io: usize,
}
impl Default for Memory {
    fn default() -> Self {
        Self {
            descriptor: descriptor(),
            durable: BTreeMap::new(),
            pending: BTreeMap::new(),
            frontier: 0,
            effects: 0,
            io: 0,
        }
    }
}
impl Target for Memory {
    type Error = Error;
    fn descriptor(&self) -> TargetDescriptor {
        self.descriptor.clone()
    }
    async fn frontier(&mut self) -> Result<u64> {
        self.io += 1;
        Ok(self.frontier)
    }
    async fn begin(&mut self) -> Result<()> {
        self.io += 1;
        self.pending = self.durable.clone();
        Ok(())
    }
    async fn receipt(&mut self, id: i64) -> Result<Option<String>> {
        self.io += 1;
        Ok(self.pending.get(&id).map(|v| v.1.clone()))
    }
    async fn insert(&mut self, row: &Value, hash: &str) -> Result<()> {
        self.io += 1;
        let n = row.ordinal()? as i64;
        if self.pending.insert(n, (row.clone(), hash.into())).is_some() {
            return Err(Error::ReceiptConflict(n as u64));
        }
        Ok(())
    }
    async fn commit(&mut self) -> Result<()> {
        self.io += 1;
        self.effects += self.pending.len() - self.durable.len();
        self.durable = self.pending.clone();
        Ok(())
    }
    async fn checkpoint(&mut self, end: u64) -> Result<()> {
        self.io += 1;
        for n in 0..end {
            if !self.durable.contains_key(&(n as i64)) {
                return Err(Error::MissingReceipt(n));
            }
        }
        self.frontier = self.frontier.max(end);
        Ok(())
    }
}
#[test]
fn serialized_plan_is_strict_and_deterministic() -> Result<()> {
    let rows = rows(8);
    let p = plan(&rows, &descriptor(), 3)?;
    let encoded = serde_json::to_string(&p).map_err(|e| Error::Encoding(e.to_string()))?;
    let decoded: MigrationPlan =
        serde_json::from_str(&encoded).map_err(|e| Error::Encoding(e.to_string()))?;
    assert_eq!(p, decoded);
    assert_eq!(
        canonical_hash(&p)?,
        decoded
            .validate(&source(&rows)?, &descriptor(), &rows)?
            .fingerprint()
            .clone()
    );
    let mut unknown = serde_json::to_value(&p).map_err(|e| Error::Encoding(e.to_string()))?;
    unknown["ignore_errors"] = json!(true);
    assert!(serde_json::from_value::<MigrationPlan>(unknown).is_err());
    assert!(serde_json::from_str::<Fingerprint>("\"INVALID\"").is_err());
    assert_eq!(
        canonical_hash(&json!({"b":2,"a":1}))?,
        canonical_hash(&json!({"a":1,"b":2}))?
    );
    Ok(())
}
#[test]
fn capability_negotiation_refuses_weaker_guarantees_and_unavailable_mappings() -> Result<()> {
    let rows = rows(8);
    let caps = descriptor();
    let p = plan(&rows, &caps, 3)?;
    let src = source(&rows)?;
    let mut wrong = p.clone();
    wrong.version = 2;
    assert!(matches!(
        wrong.validate(&src, &caps, &rows),
        Err(Error::UnsupportedVersion(2))
    ));
    wrong = p.clone();
    wrong.contract.sha256 = Fingerprint::of(b"other");
    assert!(matches!(
        wrong.validate(&src, &caps, &rows),
        Err(Error::UnsupportedContract)
    ));
    wrong = p.clone();
    wrong.guarantees.single_writer = false;
    assert!(matches!(
        wrong.validate(&src, &caps, &rows),
        Err(Error::UnsupportedGuarantee(_))
    ));
    wrong = p.clone();
    wrong.guarantees.atomic_effects_and_receipts = false;
    assert!(wrong.validate(&src, &caps, &rows).is_err());
    for publication in [Publication::NonAtomic, Publication::ConditionalObject] {
        let mut c = caps.clone();
        c.publication = publication;
        assert!(matches!(
            plan(&rows, &c, 3),
            Err(Error::UnsupportedGuarantee(_))
        ));
    }
    let mut c = caps.clone();
    c.durable_receipt_lookup = false;
    assert!(plan(&rows, &c, 3).is_err());
    c = caps.clone();
    c.contiguous_checkpoints = false;
    assert!(plan(&rows, &c, 3).is_err());
    c = caps;
    c.publication = Publication::LocalDirectoryRename;
    c.max_batch_units = 1;
    assert!(plan(&rows, &c, 1).is_ok());
    assert!(matches!(
        plan(&rows, &c, 2),
        Err(Error::Bounds("batch units"))
    ));
    let mut streaming = src;
    streaming.mode = SourceMode::ChangeStream;
    assert!(matches!(
        MigrationPlan::new(streaming, descriptor(), p.contract, &rows, 3),
        Err(Error::UnsupportedGuarantee(_))
    ));
    Ok(())
}
#[test]
fn source_target_mapping_and_byte_drift_are_rejected() -> Result<()> {
    let rows = rows(8);
    let p = plan(&rows, &descriptor(), 3)?;
    let src = source(&rows)?;
    let mut changed = rows.clone();
    changed[3]["value"] = json!(999);
    assert!(matches!(
        p.validate(&src, &descriptor(), &changed),
        Err(Error::InputDrift)
    ));
    assert!(matches!(
        p.validate(&source(&changed)?, &descriptor(), &changed),
        Err(Error::SourceDrift)
    ));
    let mut target = descriptor();
    target.receipt_domain = "different-target".into();
    assert!(matches!(
        p.validate(&src, &target, &rows),
        Err(Error::TargetDrift)
    ));
    target = descriptor();
    target.implementation_sha256 = Fingerprint::of(b"different-adapter");
    assert!(matches!(
        p.validate(&src, &target, &rows),
        Err(Error::TargetDrift)
    ));
    changed = rows.clone();
    changed.swap(0, 1);
    assert!(matches!(
        p.validate(&src, &descriptor(), &changed),
        Err(Error::InvalidOrdinal(0))
    ));
    changed = rows.clone();
    changed[0]["id"] = json!("00");
    assert!(p.validate(&src, &descriptor(), &changed).is_err());
    let mut limited = p.clone();
    limited.limits.max_batch_bytes = 1;
    assert!(matches!(
        limited.validate(&src, &descriptor(), &rows),
        Err(Error::Bounds("batch bytes"))
    ));
    limited = p.clone();
    limited.limits.max_input_bytes = 1;
    assert!(matches!(
        limited.validate(&src, &descriptor(), &rows),
        Err(Error::Bounds("input bytes"))
    ));
    limited = p;
    limited.limits.max_units = 5000;
    assert!(limited.validate(&src, &descriptor(), &rows).is_err());
    changed = vec![json!({"id":"0","value":"x".repeat(65_536)})];
    assert!(matches!(
        plan(&changed, &descriptor(), 1),
        Err(Error::Bounds("unit bytes"))
    ));
    Ok(())
}
#[tokio::test]
async fn changed_target_is_rejected_before_any_target_io() -> Result<()> {
    let rows = rows(8);
    let mut t = Memory::default();
    let p = plan(&rows, &t.descriptor, 3)?;
    let valid = p.validate(&source(&rows)?, &t.descriptor, &rows)?;
    t.descriptor.receipt_domain = "changed-after-validation".into();
    assert!(matches!(
        execute(&mut t, valid, Replay::Resume, |_, _| Ok(())).await,
        Err(Error::TargetDrift)
    ));
    assert_eq!(t.io, 0);
    assert_eq!(t.effects, 0);
    Ok(())
}
#[tokio::test]
async fn all_boundaries_recover_and_replay_without_duplicates() -> Result<()> {
    let rows = rows(9);
    for batch in [1, 3, 4] {
        let mut t = Memory::default();
        let p = plan(&rows, &t.descriptor, batch)?;
        for fault in [
            Boundary::BeforeCommit,
            Boundary::AfterCommit,
            Boundary::AfterCheckpoint,
        ] {
            let valid = p.validate(&source(&rows)?, &t.descriptor, &rows)?;
            let outcome = execute(&mut t, valid, Replay::Resume, |at, end| {
                if at == fault && end == batch * 2 {
                    Err(Error::Encoding("injected crash".into()))
                } else {
                    Ok(())
                }
            })
            .await;
            assert!(matches!(outcome,Err(Error::Encoding(ref s)) if s=="injected crash"));
            assert_eq!(
                t.durable.len() as u64,
                if fault == Boundary::BeforeCommit {
                    batch
                } else {
                    batch * 2
                }
            );
            assert_eq!(
                t.frontier,
                if fault == Boundary::AfterCheckpoint {
                    batch * 2
                } else {
                    batch
                }
            );
            t.pending.clear();
        }
        for replay in [Replay::Resume, Replay::Full] {
            let valid = p.validate(&source(&rows)?, &t.descriptor, &rows)?;
            execute(&mut t, valid, replay, |_, _| Ok(())).await?;
        }
        assert_eq!(t.effects, 9);
        assert_eq!(t.frontier, 9);
        assert_eq!(
            t.durable.values().map(|v| v.0.clone()).collect::<Vec<_>>(),
            rows
        );
    }
    Ok(())
}
#[tokio::test]
async fn completed_checkpoint_cannot_hide_missing_or_conflicting_receipts() -> Result<()> {
    let rows = rows(8);
    let mut t = Memory {
        frontier: 8,
        ..Memory::default()
    };
    let p = plan(&rows, &t.descriptor, 3)?;
    let valid = p.validate(&source(&rows)?, &t.descriptor, &rows)?;
    assert!(matches!(
        execute(&mut t, valid, Replay::Resume, |_, _| Ok(())).await,
        Err(Error::MissingReceipt(0))
    ));
    assert_eq!(t.effects, 0);
    t = Memory::default();
    let valid = p.validate(&source(&rows)?, &t.descriptor, &rows)?;
    execute(&mut t, valid, Replay::Resume, |_, _| Ok(())).await?;
    let mut drift = p;
    drift.contract.sha256 = Fingerprint::of(b"changed-mapping");
    t.descriptor.contracts = vec![drift.contract.clone()];
    drift.target = t.descriptor.clone();
    let valid = drift.validate(&source(&rows)?, &t.descriptor, &rows)?;
    assert!(matches!(
        execute(&mut t, valid, Replay::Resume, |_, _| Ok(())).await,
        Err(Error::ReceiptConflict(0))
    ));
    assert_eq!(t.effects, 8);
    Ok(())
}
