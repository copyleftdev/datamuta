use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataType {
    Bool,
    I32,
    I64,
    Utf8,
}

/// Tagged values keep integer widths explicit and preserve null versus empty text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Scalar {
    Bool(bool),
    I32(i32),
    I64(i64),
    Utf8(String),
    Null,
}
pub type Record = BTreeMap<String, Scalar>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub name: String,
    pub data_type: DataType,
    pub nullable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schema {
    pub version: u32,
    pub namespace: String,
    pub relation: String,
    /// Field order is not semantic; primary-key component order is semantic.
    pub fields: Vec<Field>,
    pub primary_key: Vec<String>,
}
impl Schema {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            return Err(Error::Version(self.version));
        }
        if !identifier(&self.namespace) || !identifier(&self.relation) {
            return Err(Error::Schema("identifier outside admitted ASCII subset"));
        }
        if self.fields.is_empty() || self.fields.len() > 64 {
            return Err(Error::Bounds("1..64 fields"));
        }
        let mut names = BTreeSet::new();
        for f in &self.fields {
            if !identifier(&f.name) || !names.insert(&f.name) {
                return Err(Error::Schema("invalid or duplicate field"));
            }
        }
        if self.primary_key.is_empty() || self.primary_key.len() > 8 {
            return Err(Error::Schema("1..8 identity fields required"));
        }
        let mut keys = BTreeSet::new();
        for key in &self.primary_key {
            let f = self
                .field(key)
                .ok_or(Error::Schema("unknown identity field"))?;
            if !keys.insert(key)
                || f.nullable
                || !matches!(f.data_type, DataType::I32 | DataType::I64)
            {
                return Err(Error::Schema(
                    "identity requires distinct non-null integer fields",
                ));
            }
        }
        Ok(())
    }
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }
    pub(crate) fn normalized(&self) -> Result<Self> {
        self.validate()?;
        let mut s = self.clone();
        s.fields.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(s)
    }
    pub fn fingerprint(&self) -> Result<datamuta_kernel::Fingerprint> {
        Ok(datamuta_kernel::canonical_hash(&self.normalized()?)?)
    }
    pub fn check_record(&self, record: &Record) -> Result<()> {
        self.validate()?;
        if record.len() != self.fields.len() {
            return Err(Error::Record("missing or extra field"));
        }
        for field in &self.fields {
            let value = record
                .get(&field.name)
                .ok_or(Error::Record("missing field"))?;
            let valid = match (field.data_type, value) {
                (_, Scalar::Null) => field.nullable,
                (DataType::Bool, Scalar::Bool(_))
                | (DataType::I32, Scalar::I32(_))
                | (DataType::I64, Scalar::I64(_)) => true,
                (DataType::Utf8, Scalar::Utf8(s)) => {
                    if s.len() > 32_768 {
                        return Err(Error::Bounds("text bytes"));
                    }
                    true
                }
                _ => false,
            };
            if !valid {
                return Err(Error::Record("type or nullability mismatch"));
            }
        }
        Ok(())
    }
}
