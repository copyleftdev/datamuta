//! Checked, deterministic compilation of untrusted flat-record mapping candidates.
//! Describers supply authoritative schemas; this library performs no database I/O.
#![forbid(unsafe_code)]
mod mapping;
mod schema;
mod selection;
pub use mapping::{Candidate, CompiledMapping, FieldMapping, MappedUnit, Operation, compile};
pub use schema::{DataType, Field, Record, Scalar, Schema};
pub use selection::{ChoiceSet, Decision, Evidence, MODEL, Response, SelectionRequest};
use serde::{Deserialize, Serialize};
use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Version(u32),
    Schema(&'static str),
    Mapping(&'static str),
    Drift,
    Record(&'static str),
    Bounds(&'static str),
    Selection(&'static str),
    Kernel(datamuta_kernel::Error),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Version(v) => write!(f, "unsupported schema/mapping version {v}"),
            Self::Schema(s) => write!(f, "invalid schema: {s}"),
            Self::Mapping(s) => write!(f, "mapping refused: {s}"),
            Self::Drift => write!(f, "observed schema differs from compiled mapping"),
            Self::Record(s) => write!(f, "record refused: {s}"),
            Self::Bounds(s) => write!(f, "planning bound exceeded: {s}"),
            Self::Selection(s) => write!(f, "selection refused: {s}"),
            Self::Kernel(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for Error {}
impl From<datamuta_kernel::Error> for Error {
    fn from(e: datamuta_kernel::Error) -> Self {
        Self::Kernel(e)
    }
}
pub(crate) fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.bytes()
            .enumerate()
            .all(|(i, b)| b.is_ascii_alphabetic() || b == b'_' || (i > 0 && b.is_ascii_digit()))
}
#[cfg(test)]
mod selection_tests;
#[cfg(test)]
mod tests;
