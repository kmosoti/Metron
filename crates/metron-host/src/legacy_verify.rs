//! Independent verification of bound fixture evidence and published bytes.
use crate::legacy_publication as publication;
use metron_protocol::{
    domain::{ApprovedRequest, ReceiptOutput, parse_sum_input},
    store::{VerificationSelection, VerificationSnapshot},
    types::{MAX_INTERNAL, canonical_bytes, content_digest, structural_digest},
};
use std::path::Path;

#[derive(Debug, Eq, PartialEq)]
pub enum VerifyError {
    InvalidEvidence(&'static str),
    Operational,
}
impl From<&'static str> for VerifyError {
    fn from(value: &'static str) -> Self {
        Self::InvalidEvidence(value)
    }
}

/// Recompute the sum from the read evidence and inspect the persisted artifact.
/// The store must still CAS the returned selection against current evidence.
pub fn verify(
    run_dir: &Path,
    snapshot: &VerificationSnapshot,
) -> Result<(VerificationSelection, bool), VerifyError> {
    if snapshot.unresolved || snapshot.conflict {
        return Err(VerifyError::InvalidEvidence(
            "unresolved or conflicting evidence",
        ));
    }
    let read = snapshot.read.as_ref().ok_or("missing read receipt")?;
    let model = snapshot.model.as_ref().ok_or("missing model receipt")?;
    let publication_receipt = snapshot
        .publication
        .as_ref()
        .ok_or("missing publication receipt")?;
    let plan = snapshot.plan.as_ref().ok_or("missing artifact plan")?;
    let read_id = snapshot
        .read_evidence
        .clone()
        .ok_or("missing read evidence")?;
    let model_id = snapshot
        .model_evidence
        .clone()
        .ok_or("missing model evidence")?;
    let publication_id = snapshot
        .publication_evidence
        .clone()
        .ok_or("missing publication evidence")?;
    if read.evidence_id().map_err(|_| "bad receipt")? != read_id
        || model.evidence_id().map_err(|_| "bad receipt")? != model_id
        || publication_receipt
            .evidence_id()
            .map_err(|_| "bad receipt")?
            != publication_id
    {
        return Err(VerifyError::InvalidEvidence("evidence digest mismatch"));
    }
    if read.data().task_id != model.data().task_id
        || read.data().task_id != publication_receipt.data().task_id
    {
        return Err(VerifyError::InvalidEvidence("mixed task evidence"));
    }
    let (bytes, digest) = match &read.data().output {
        ReceiptOutput::Fixture { bytes, digest } => (bytes.as_str().as_bytes(), digest),
        _ => return Err(VerifyError::InvalidEvidence("invalid read output")),
    };
    if content_digest(bytes) != *digest {
        return Err(VerifyError::InvalidEvidence("fixture digest mismatch"));
    }
    let input = parse_sum_input(bytes).map_err(|_| "invalid fixture")?;
    let mut sum = 0_i64;
    for value in &input.values {
        sum = sum.checked_add(*value).ok_or("sum overflow")?;
    }
    let count = u32::try_from(input.values.len()).map_err(|_| "count overflow")?;
    let local_candidate;
    let candidate = match &model.data().output {
        ReceiptOutput::Model { candidate } => candidate,
        ReceiptOutput::LocalModel { observation } => {
            use metron_protocol::inference::{
                LocalOutputSchema, LocalResponse, parse_local_response,
            };
            let parsed = parse_local_response(
                observation.content.as_str().as_bytes(),
                LocalOutputSchema::Sum,
            )
            .map_err(|_| VerifyError::InvalidEvidence("invalid local model response"))?;
            if observation.response.as_ref() != Some(&parsed) || observation.input_digest != *digest
            {
                return Err(VerifyError::InvalidEvidence(
                    "local model observation mismatch",
                ));
            }
            let LocalResponse::Sum(result) = parsed else {
                return Err(VerifyError::InvalidEvidence("invalid local model schema"));
            };
            local_candidate = metron_protocol::CandidateResult {
                count: result.count,
                sum: result.sum,
                input_digest: digest.clone(),
            };
            &local_candidate
        }
        _ => return Err(VerifyError::InvalidEvidence("invalid model output")),
    };
    let published = match &publication_receipt.data().output {
        ReceiptOutput::Published {
            artifact_id,
            content_digest,
            content_length,
        } => {
            artifact_id == &plan.artifact_id
                && content_digest == &plan.content_digest
                && content_length == &plan.content_length
        }
        _ => false,
    };
    let correct = candidate.count == count
        && candidate.sum == sum
        && candidate.input_digest == *digest
        && *candidate == plan.candidate
        && published
        && publication_receipt.data().intent_id == plan.intent_id
        && publication_receipt.data().dispatch_id == plan.dispatch_id;
    let exists = publication::inspect(run_dir, plan).map_err(|error| {
        if error.kind() == std::io::ErrorKind::InvalidData {
            VerifyError::InvalidEvidence("invalid artifact plan")
        } else {
            VerifyError::Operational
        }
    })?;
    let plan_digest = structural_digest(
        "approved-request/v1",
        &[&canonical_bytes(
            &ApprovedRequest::PublishArtifact { plan: plan.clone() },
            MAX_INTERNAL,
        )
        .map_err(|_| "invalid plan")?],
    )
    .map_err(|_| "invalid plan")?;
    Ok((
        VerificationSelection {
            evidence_revision: snapshot.evidence_revision,
            read_evidence: read_id,
            model_evidence: model_id,
            publication_evidence: publication_id,
            plan_digest,
        },
        correct && exists,
    ))
}
