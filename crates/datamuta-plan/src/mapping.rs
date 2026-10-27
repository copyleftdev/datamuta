use super::*;
use datamuta_kernel::{Contract, Fingerprint, MigrationUnit, canonical_hash};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Copy,
    WidenI32ToI64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldMapping {
    pub source: String,
    pub target: String,
    pub operation: Operation,
}
/// An untrusted proposal, including proposals supplied by a planning engine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub version: u32,
    pub fields: Vec<FieldMapping>,
}
#[derive(Clone, Debug, Serialize)]
pub struct CompiledMapping {
    compiler: &'static str,
    source: Schema,
    target: Schema,
    candidate: Candidate,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MappedUnit {
    ordinal: u64,
    mapping: Fingerprint,
    fields: Record,
}
impl MappedUnit {
    pub fn fields(&self) -> &Record {
        &self.fields
    }
    pub fn mapping(&self) -> &Fingerprint {
        &self.mapping
    }
}
impl MigrationUnit for MappedUnit {
    fn ordinal(&self) -> datamuta_kernel::Result<u64> {
        Ok(self.ordinal)
    }
    fn canonical_bytes(&self) -> datamuta_kernel::Result<Vec<u8>> {
        serde_json::to_vec(self).map_err(|e| datamuta_kernel::Error::Encoding(e.to_string()))
    }
}
pub fn compile(source: &Schema, target: &Schema, proposal: &Candidate) -> Result<CompiledMapping> {
    let source = source.normalized()?;
    let target = target.normalized()?;
    if proposal.version != 1 {
        return Err(Error::Version(proposal.version));
    }
    if proposal.fields.len() != source.fields.len() || proposal.fields.len() != target.fields.len()
    {
        return Err(Error::Mapping(
            "every source and target field must be covered exactly once",
        ));
    }
    let mut src = BTreeSet::new();
    let mut dst = BTreeSet::new();
    for m in &proposal.fields {
        let s = source
            .field(&m.source)
            .ok_or(Error::Mapping("unknown source field"))?;
        let t = target
            .field(&m.target)
            .ok_or(Error::Mapping("unknown target field"))?;
        if !src.insert(&m.source) || !dst.insert(&m.target) {
            return Err(Error::Mapping("duplicate field assignment"));
        }
        if s.nullable && !t.nullable {
            return Err(Error::Mapping("nullable source into required target"));
        }
        let allowed = match m.operation {
            Operation::Copy => s.data_type == t.data_type,
            Operation::WidenI32ToI64 => {
                s.data_type == DataType::I32 && t.data_type == DataType::I64
            }
        };
        if !allowed {
            return Err(Error::Mapping("unsupported or lossy conversion"));
        }
    }
    let mapped_keys: Vec<_> = source
        .primary_key
        .iter()
        .map(|k| {
            proposal
                .fields
                .iter()
                .find(|m| m.source == *k)
                .map(|m| m.target.clone())
                .ok_or(Error::Mapping("missing identity mapping"))
        })
        .collect::<Result<_>>()?;
    if mapped_keys != target.primary_key {
        return Err(Error::Mapping("identity components or order changed"));
    }
    let mut candidate = proposal.clone();
    candidate.fields.sort_by(|a, b| a.target.cmp(&b.target));
    Ok(CompiledMapping {
        compiler: "datamuta/flat-mapping/v1",
        source,
        target,
        candidate,
    })
}
impl CompiledMapping {
    pub fn source(&self) -> &Schema {
        &self.source
    }
    pub fn target(&self) -> &Schema {
        &self.target
    }
    pub fn contract(&self) -> Result<Contract> {
        Ok(Contract {
            name: "flat-record-v1".into(),
            version: 1,
            sha256: canonical_hash(self)?,
        })
    }
    pub fn validate_observed(&self, source: &Schema, target: &Schema) -> Result<()> {
        if source.normalized()? != self.source || target.normalized()? != self.target {
            return Err(Error::Drift);
        }
        Ok(())
    }
    /// Fully validate and materialize a bounded snapshot before any target effects.
    /// Identity uniqueness is checked independently of unit ordinals.
    pub fn prepare(&self, rows: &[Record]) -> Result<Vec<MappedUnit>> {
        if rows.is_empty() || rows.len() > 4096 {
            return Err(Error::Bounds("1..4096 records"));
        }
        let mapping = self.contract()?.sha256;
        let mut identities = BTreeSet::new();
        let mut output = Vec::with_capacity(rows.len());
        let mut total = 0usize;
        for (n, row) in rows.iter().enumerate() {
            self.source.check_record(row)?;
            let identity = self
                .source
                .primary_key
                .iter()
                .map(|k| match row.get(k) {
                    Some(Scalar::I32(v)) => Ok(i64::from(*v)),
                    Some(Scalar::I64(v)) => Ok(*v),
                    _ => Err(Error::Record("invalid identity")),
                })
                .collect::<Result<Vec<_>>>()?;
            if !identities.insert(identity) {
                return Err(Error::Record("duplicate source identity"));
            }
            let mut fields = Record::new();
            for m in &self.candidate.fields {
                let v = row.get(&m.source).ok_or(Error::Record("missing field"))?;
                let value = match (m.operation, v) {
                    (Operation::WidenI32ToI64, Scalar::I32(n)) => Scalar::I64(i64::from(*n)),
                    _ => v.clone(),
                };
                fields.insert(m.target.clone(), value);
            }
            self.target.check_record(&fields)?;
            let unit = MappedUnit {
                ordinal: n as u64,
                mapping: mapping.clone(),
                fields,
            };
            let input_len = serde_json::to_vec(row)
                .map_err(|e| Error::Kernel(datamuta_kernel::Error::Encoding(e.to_string())))?
                .len();
            let output_len = unit.canonical_bytes()?.len();
            if input_len > 65_536 || output_len > 65_536 {
                return Err(Error::Bounds("record encoded bytes"));
            }
            total = total
                .checked_add(input_len + output_len)
                .ok_or(Error::Bounds("snapshot bytes"))?;
            if total > 64 * 1024 * 1024 {
                return Err(Error::Bounds("snapshot bytes"));
            }
            output.push(unit);
        }
        Ok(output)
    }
}
