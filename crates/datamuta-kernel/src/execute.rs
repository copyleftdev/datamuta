use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Boundary {
    BeforeCommit,
    AfterCommit,
    AfterCheckpoint,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Replay {
    Resume,
    Full,
}

/// Effects and matching receipts MUST publish atomically in one declared domain.
/// Checkpoints may advance only over contiguous durable receipts.
/// On any execution error, discard this adapter/session before retrying: outcomes
/// are resolved from durable receipts on a fresh session, never by blind retry.
#[allow(async_fn_in_trait)]
pub trait Target<U: MigrationUnit = serde_json::Value> {
    type Error: From<Error>;
    fn descriptor(&self) -> TargetDescriptor;
    async fn frontier(&mut self) -> std::result::Result<u64, Self::Error>;
    async fn begin(&mut self) -> std::result::Result<(), Self::Error>;
    async fn receipt(&mut self, id: i64) -> std::result::Result<Option<String>, Self::Error>;
    async fn insert(&mut self, unit: &U, hash: &str) -> std::result::Result<(), Self::Error>;
    async fn commit(&mut self) -> std::result::Result<(), Self::Error>;
    async fn checkpoint(&mut self, end: u64) -> std::result::Result<(), Self::Error>;
}
fn receipt_hash(plan: &Fingerprint, ordinal: usize, unit: &Fingerprint) -> Fingerprint {
    Fingerprint::of(
        format!(
            "datamuta/receipt/v1\0{}:{ordinal}:{}",
            plan.as_str(),
            unit.as_str()
        )
        .as_bytes(),
    )
}
pub async fn execute<U: MigrationUnit, T: Target<U>>(
    target: &mut T,
    validated: ValidatedPlan<'_, U>,
    replay: Replay,
    mut boundary: impl FnMut(Boundary, u64) -> std::result::Result<(), T::Error>,
) -> std::result::Result<(), T::Error> {
    if target.descriptor() != validated.plan.target {
        return Err(Error::TargetDrift.into());
    }
    let frontier = target.frontier().await?;
    if frontier > validated.plan.units {
        return Err(Error::FrontierOutOfBounds(frontier).into());
    }
    let batch = validated.plan.batch_units as usize;
    if frontier != validated.plan.units && !frontier.is_multiple_of(batch as u64) {
        return Err(Error::Bounds("checkpoint batch boundary").into());
    }
    let hashes: Vec<_> = validated
        .hashes
        .iter()
        .enumerate()
        .map(|(n, h)| receipt_hash(&validated.id, n, h))
        .collect();
    // A complete checkpoint is not proof: verify the skipped receipt prefix first.
    // Read-only transactions keep the adapter's receipt lookup semantics intact.
    for start in (0..frontier as usize).step_by(batch) {
        target.begin().await?;
        for (n, hash) in hashes
            .iter()
            .enumerate()
            .take((start + batch).min(frontier as usize))
            .skip(start)
        {
            let saved = target
                .receipt(n as i64)
                .await?
                .ok_or_else(|| T::Error::from(Error::MissingReceipt(n as u64)))?;
            if saved != hash.as_str() {
                return Err(Error::ReceiptConflict(n as u64).into());
            }
        }
        target.commit().await?;
    }
    let start = if replay == Replay::Full {
        0
    } else {
        frontier as usize
    };
    for offset in (start..validated.units.len()).step_by(batch) {
        let end = (offset + batch).min(validated.units.len());
        target.begin().await?;
        for (n, unit) in validated.units.iter().enumerate().take(end).skip(offset) {
            if let Some(saved) = target.receipt(n as i64).await? {
                if saved != hashes[n].as_str() {
                    return Err(Error::ReceiptConflict(n as u64).into());
                }
            } else {
                target.insert(unit, hashes[n].as_str()).await?;
            }
        }
        boundary(Boundary::BeforeCommit, end as u64)?;
        target.commit().await?;
        boundary(Boundary::AfterCommit, end as u64)?;
        target.checkpoint((end as u64).max(frontier)).await?;
        boundary(Boundary::AfterCheckpoint, end as u64)?;
    }
    Ok(())
}
