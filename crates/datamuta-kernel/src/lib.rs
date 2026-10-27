//! Runtime-independent, bounded frozen-snapshot migration kernel.
//! Adapter declarations are trusted contracts, not proof of storage behavior.
#![forbid(unsafe_code)]
mod execute;
mod plan;
pub use execute::{Boundary, Replay, Target, execute};
pub use plan::{
    Contract, Guarantees, Limits, MigrationPlan, Publication, SourceDescriptor, SourceMode,
    TargetDescriptor, ValidatedPlan,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    UnsupportedVersion(u32),
    InvalidDescriptor(&'static str),
    UnsupportedGuarantee(&'static str),
    UnsupportedContract,
    TargetDrift,
    SourceDrift,
    InputDrift,
    Bounds(&'static str),
    InvalidOrdinal(u64),
    FrontierOutOfBounds(u64),
    MissingReceipt(u64),
    ReceiptConflict(u64),
    Encoding(String),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion(v) => write!(f, "unsupported migration plan version {v}"),
            Self::InvalidDescriptor(s) => write!(f, "invalid adapter descriptor: {s}"),
            Self::UnsupportedGuarantee(s) => write!(f, "unsupported guarantee: {s}"),
            Self::UnsupportedContract => {
                write!(f, "target does not support the declared mapping contract")
            }
            Self::TargetDrift => {
                write!(f, "target identity, implementation or capabilities changed")
            }
            Self::SourceDrift => write!(f, "source identity or frozen snapshot changed"),
            Self::InputDrift => write!(f, "input differs from migration plan"),
            Self::Bounds(s) => write!(f, "migration bound exceeded: {s}"),
            Self::InvalidOrdinal(n) => write!(f, "source identity/order changed at unit {n}"),
            Self::FrontierOutOfBounds(n) => write!(f, "frontier {n} exceeds planned units"),
            Self::MissingReceipt(n) => write!(f, "checkpoint crosses missing receipt at unit {n}"),
            Self::ReceiptConflict(n) => write!(f, "receipt/source or plan conflict at unit {n}"),
            Self::Encoding(s) => write!(f, "canonical encoding: {s}"),
        }
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Fingerprint(String);
impl Fingerprint {
    pub fn of(bytes: &[u8]) -> Self {
        Self(hex(&Sha256::digest(bytes)))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for Fingerprint {
    type Error = Error;
    fn try_from(value: String) -> Result<Self> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::InvalidDescriptor(
                "SHA-256 must be 64 lowercase hex characters",
            ));
        }
        Ok(Self(value))
    }
}
impl From<Fingerprint> for String {
    fn from(v: Fingerprint) -> Self {
        v.0
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Supply stable, deterministic bytes and a contiguous zero-based ordinal.
/// Custom unit types can represent records, aggregates, partitions or opaque tasks.
pub trait MigrationUnit {
    fn ordinal(&self) -> Result<u64>;
    fn canonical_bytes(&self) -> Result<Vec<u8>>;
}
impl MigrationUnit for serde_json::Value {
    fn ordinal(&self) -> Result<u64> {
        let id = self
            .get("id")
            .and_then(Self::as_str)
            .ok_or(Error::InvalidOrdinal(0))?;
        let n = id.parse::<u64>().map_err(|_| Error::InvalidOrdinal(0))?;
        if id != n.to_string() {
            return Err(Error::InvalidOrdinal(n));
        }
        Ok(n)
    }
    fn canonical_bytes(&self) -> Result<Vec<u8>> {
        let mut value = self.clone();
        value.sort_all_objects();
        serde_json::to_vec(&value).map_err(|e| Error::Encoding(e.to_string()))
    }
}
pub fn canonical_hash<T: Serialize>(value: &T) -> Result<Fingerprint> {
    let mut value = serde_json::to_value(value).map_err(|e| Error::Encoding(e.to_string()))?;
    value.sort_all_objects();
    Ok(Fingerprint::of(
        &serde_json::to_vec(&value).map_err(|e| Error::Encoding(e.to_string()))?,
    ))
}

#[cfg(test)]
mod tests;
