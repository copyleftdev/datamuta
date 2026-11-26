//! Mapping suggestion through a saved JEV Choice exchange.
//!
//! `prepare` enumerates the structurally valid mappings between two discovered
//! schemas and, when more than one exists, writes the exact JEV request bytes.
//! `admit` rebuilds that request from the same live schemas and evidence,
//! refuses it if anything changed, and runs the frozen admission policy on a
//! saved response. Datamuta never contacts the provider here.
//!
//! A suggested mapping grants no authority. It is an input to `configure`, and
//! the resulting plan still needs inspection and exact-hash approval.
use crate::{Result, err, require};
use datamuta_kernel::Fingerprint;
use datamuta_plan::{
    Candidate, ChoiceSet, Decision, Evidence, MODEL, Response, Schema, SelectionRequest,
};
use serde::Serialize;
use std::{collections::BTreeMap, io::Read, path::Path};

/// Upper bound for a saved provider response.
pub const MAX_RESPONSE_BYTES: usize = 131_072;
/// Upper bound for a saved request; `SelectionRequest` itself caps payloads at 32 KiB.
pub const MAX_REQUEST_BYTES: usize = 32_768;

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Prepared {
    /// Exactly one structurally valid mapping exists, so there is nothing to choose.
    /// Structural uniqueness still does not establish business meaning.
    UniqueCandidate { mapping: Candidate },
    /// Several valid mappings exist; send `payload` to JEV byte for byte.
    RequestPrepared {
        model: &'static str,
        request_sha256: Fingerprint,
        candidates: BTreeMap<String, Candidate>,
        #[serde(skip)]
        payload: Vec<u8>,
    },
}

/// What was asked, what came back, and how the frozen policy judged it.
#[derive(Clone, Debug, Serialize)]
pub struct Provenance {
    pub version: u32,
    pub pinned_model: &'static str,
    pub response_model: String,
    pub request_sha256: Fingerprint,
    pub response_sha256: Fingerprint,
    pub choice: String,
    pub confidence: f64,
    pub probabilities: BTreeMap<String, f64>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    pub decision: Decision,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Admitted {
    Accepted {
        candidate_id: String,
        mapping: Candidate,
        provenance: Provenance,
    },
    ReviewRequired {
        reason: String,
        candidates: BTreeMap<String, Candidate>,
        provenance: Provenance,
    },
}

fn choices(source: &Schema, target: &Schema) -> Result<ChoiceSet> {
    let choices = ChoiceSet::generate(source, target)?;
    require(
        !choices.candidates().is_empty(),
        "no structurally valid mapping between these schemas",
    )?;
    Ok(choices)
}

pub fn prepare(source: &Schema, target: &Schema, evidence: &Evidence) -> Result<Prepared> {
    let choices = choices(source, target)?;
    if choices.candidates().len() == 1 {
        let mapping = choices
            .candidates()
            .values()
            .next()
            .cloned()
            .ok_or_else(|| err("candidate set changed during preparation"))?;
        return Ok(Prepared::UniqueCandidate { mapping });
    }
    let candidates = choices.candidates().clone();
    let request = SelectionRequest::new(choices, evidence.clone())?;
    Ok(Prepared::RequestPrepared {
        model: MODEL,
        request_sha256: request.fingerprint()?,
        candidates,
        payload: request.payload_bytes()?,
    })
}

pub fn admit(
    source: &Schema,
    target: &Schema,
    evidence: &Evidence,
    request_bytes: &[u8],
    response_bytes: &[u8],
) -> Result<Admitted> {
    let choices = choices(source, target)?;
    require(
        choices.candidates().len() > 1,
        "a single structural candidate needs no selection; run prepare",
    )?;
    let candidates = choices.candidates().clone();
    let request = SelectionRequest::new(choices, evidence.clone())?;
    require(
        request.payload_bytes()? == request_bytes,
        "saved request does not match the live schemas and evidence; prepare a new request",
    )?;
    require(
        response_bytes.len() <= MAX_RESPONSE_BYTES,
        "provider response bytes",
    )?;
    let response: Response = serde_json::from_slice(response_bytes)?;
    let decision = request.decide(&response)?;
    let pick = &response.answers.pick;
    let provenance = Provenance {
        version: 1,
        pinned_model: MODEL,
        response_model: response.model.clone(),
        request_sha256: request.fingerprint()?,
        response_sha256: Fingerprint::of(response_bytes),
        choice: pick.choice.clone(),
        confidence: pick.confidence,
        probabilities: pick.probabilities.clone(),
        input_tokens: response.usage.input_tokens,
        output_tokens: response.usage.output_tokens,
        cost: response.usage.cost,
        decision: decision.clone(),
    };
    Ok(match decision {
        Decision::Accepted { candidate_id, .. } => {
            let mapping = candidates
                .get(&candidate_id)
                .cloned()
                .ok_or_else(|| err("admitted candidate is not in the enumerated set"))?;
            Admitted::Accepted {
                candidate_id,
                mapping,
                provenance,
            }
        }
        Decision::Review { reason, .. } => Admitted::ReviewRequired {
            reason,
            candidates,
            provenance,
        },
    })
}

/// Read at most `limit` bytes; a larger file is refused rather than truncated.
pub fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    std::fs::File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut data)?;
    require(data.len() <= limit, "input file bound")?;
    Ok(data)
}

/// Write exact bytes to a new file; an existing path is never overwritten.
pub fn save_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}
