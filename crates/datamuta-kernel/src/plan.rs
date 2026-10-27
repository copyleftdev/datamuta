use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Publication {
    AtomicTransaction,
    LocalDirectoryRename,
    ConditionalObject,
    NonAtomic,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceMode {
    FrozenSnapshot,
    ChangeStream,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contract {
    pub name: String,
    pub version: u32,
    pub sha256: Fingerprint,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceDescriptor {
    pub adapter_id: String,
    pub mode: SourceMode,
    pub snapshot_sha256: Fingerprint,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetDescriptor {
    pub adapter_id: String,
    pub adapter_version: u32,
    pub implementation_sha256: Fingerprint,
    pub receipt_domain: String,
    pub publication: Publication,
    pub durable_receipt_lookup: bool,
    pub contiguous_checkpoints: bool,
    pub max_batch_units: u64,
    pub max_unit_bytes: u64,
    pub contracts: Vec<Contract>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Guarantees {
    pub atomic_effects_and_receipts: bool,
    pub contiguous_checkpoints: bool,
    pub single_writer: bool,
}
impl Default for Guarantees {
    fn default() -> Self {
        Self {
            atomic_effects_and_receipts: true,
            contiguous_checkpoints: true,
            single_writer: true,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub max_units: u64,
    pub max_unit_bytes: u64,
    pub max_input_bytes: u64,
    pub max_batch_bytes: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_units: 4096,
            max_unit_bytes: 65_536,
            max_input_bytes: 64 * 1024 * 1024,
            max_batch_bytes: 4 * 1024 * 1024,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationPlan {
    pub version: u32,
    pub source: SourceDescriptor,
    pub target: TargetDescriptor,
    pub contract: Contract,
    pub guarantees: Guarantees,
    pub limits: Limits,
    pub units: u64,
    pub input_sha256: Fingerprint,
    pub batch_units: u64,
}
pub struct ValidatedPlan<'a, U: MigrationUnit> {
    pub(crate) plan: MigrationPlan,
    pub(crate) units: &'a [U],
    pub(crate) hashes: Vec<Fingerprint>,
    pub(crate) id: Fingerprint,
}
impl<U: MigrationUnit> ValidatedPlan<'_, U> {
    pub fn fingerprint(&self) -> &Fingerprint {
        &self.id
    }
    pub fn plan(&self) -> &MigrationPlan {
        &self.plan
    }
}
fn label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"./_-".contains(&b))
}
impl TargetDescriptor {
    pub fn validate(&self) -> Result<()> {
        if !label(&self.adapter_id)
            || self.adapter_version != 1
            || self.receipt_domain.is_empty()
            || self.receipt_domain.len() > 1024
            || self.receipt_domain.chars().any(char::is_control)
        {
            return Err(Error::InvalidDescriptor(
                "adapter version, identifier or receipt domain",
            ));
        }
        if !(1..=64).contains(&self.max_batch_units) || !(1..=65_536).contains(&self.max_unit_bytes)
        {
            return Err(Error::Bounds("target capabilities"));
        }
        if self.contracts.is_empty() || self.contracts.len() > 32 {
            return Err(Error::InvalidDescriptor("contract count"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for c in &self.contracts {
            if !label(&c.name) || c.version != 1 || !seen.insert((&c.name, c.version)) {
                return Err(Error::InvalidDescriptor("contract identity/version"));
            }
        }
        Ok(())
    }
}
fn inspect<U: MigrationUnit>(
    units: &[U],
    limits: &Limits,
    batch: u64,
    max_unit: u64,
) -> Result<(Vec<Fingerprint>, Fingerprint)> {
    if units.is_empty() || units.len() as u64 > limits.max_units {
        return Err(Error::Bounds("unit count"));
    }
    let mut hashes = Vec::with_capacity(units.len());
    let mut root = Sha256::new();
    root.update(b"datamuta/input/v1\0");
    let mut total = 0u64;
    let mut batch_bytes = 0u64;
    for (n, unit) in units.iter().enumerate() {
        if unit.ordinal()? != n as u64 {
            return Err(Error::InvalidOrdinal(n as u64));
        }
        let bytes = unit.canonical_bytes()?;
        let len = bytes.len() as u64;
        if len > limits.max_unit_bytes.min(max_unit) {
            return Err(Error::Bounds("unit bytes"));
        }
        total = total.checked_add(len).ok_or(Error::Bounds("input bytes"))?;
        if total > limits.max_input_bytes {
            return Err(Error::Bounds("input bytes"));
        }
        if (n as u64).is_multiple_of(batch) {
            batch_bytes = 0;
        }
        batch_bytes = batch_bytes
            .checked_add(len)
            .ok_or(Error::Bounds("batch bytes"))?;
        if batch_bytes > limits.max_batch_bytes {
            return Err(Error::Bounds("batch bytes"));
        }
        let hash = Fingerprint::of(&bytes);
        root.update((n as u64).to_le_bytes());
        root.update(hash.as_str().as_bytes());
        hashes.push(hash);
    }
    Ok((hashes, Fingerprint(hex(&root.finalize()))))
}
impl MigrationPlan {
    pub fn new<U: MigrationUnit>(
        source: SourceDescriptor,
        target: TargetDescriptor,
        contract: Contract,
        units: &[U],
        batch_units: u64,
    ) -> Result<Self> {
        let limits = Limits::default();
        if !(1..=64).contains(&batch_units) {
            return Err(Error::Bounds("batch units"));
        }
        let (_, input_sha256) = inspect(units, &limits, batch_units, target.max_unit_bytes)?;
        let plan = Self {
            version: 1,
            source,
            target,
            contract,
            guarantees: Guarantees::default(),
            limits,
            units: units.len() as u64,
            input_sha256,
            batch_units,
        };
        plan.validate(&plan.source, &plan.target, units)?;
        Ok(plan)
    }
    pub fn validate<'a, U: MigrationUnit>(
        &self,
        source: &SourceDescriptor,
        target: &TargetDescriptor,
        units: &'a [U],
    ) -> Result<ValidatedPlan<'a, U>> {
        if self.version != 1 {
            return Err(Error::UnsupportedVersion(self.version));
        }
        target.validate()?;
        if self.target != *target {
            return Err(Error::TargetDrift);
        }
        if self.source != *source {
            return Err(Error::SourceDrift);
        }
        if !label(&source.adapter_id) {
            return Err(Error::InvalidDescriptor("source adapter identity"));
        }
        if source.mode != SourceMode::FrozenSnapshot {
            return Err(Error::UnsupportedGuarantee(
                "only frozen snapshots are admitted",
            ));
        }
        if self.guarantees != Guarantees::default() {
            return Err(Error::UnsupportedGuarantee(
                "atomic receipts, contiguous checkpoints and one writer are mandatory in v1",
            ));
        }
        if !matches!(
            target.publication,
            Publication::AtomicTransaction | Publication::LocalDirectoryRename
        ) || !target.durable_receipt_lookup
            || !target.contiguous_checkpoints
        {
            return Err(Error::UnsupportedGuarantee(
                "target publication/receipt/checkpoint contract",
            ));
        }
        if !target.contracts.contains(&self.contract) {
            return Err(Error::UnsupportedContract);
        }
        let ceiling = Limits::default();
        if self.limits.max_units == 0
            || self.limits.max_units > ceiling.max_units
            || self.limits.max_unit_bytes == 0
            || self.limits.max_unit_bytes > ceiling.max_unit_bytes
            || self.limits.max_input_bytes == 0
            || self.limits.max_input_bytes > ceiling.max_input_bytes
            || self.limits.max_batch_bytes == 0
            || self.limits.max_batch_bytes > ceiling.max_batch_bytes
        {
            return Err(Error::Bounds("plan limits"));
        }
        if self.batch_units == 0
            || self.batch_units > target.max_batch_units
            || self.batch_units > 64
        {
            return Err(Error::Bounds("batch units"));
        }
        let (hashes, input) =
            inspect(units, &self.limits, self.batch_units, target.max_unit_bytes)?;
        if self.units != units.len() as u64 || self.input_sha256 != input {
            return Err(Error::InputDrift);
        }
        Ok(ValidatedPlan {
            plan: self.clone(),
            units,
            hashes,
            id: canonical_hash(self)?,
        })
    }
}
