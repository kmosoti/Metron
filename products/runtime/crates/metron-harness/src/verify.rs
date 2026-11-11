//! Independent verification of bound fixture evidence and published bytes.
use crate::publication;
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
    let candidate = match &model.data().output {
        ReceiptOutput::Model { candidate } => candidate,
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

#[cfg(test)]
mod tests {
    use super::*;
    use metron_protocol::{
        domain::{
            ActualUsage, CandidateResult, Intent, IntentKind, PreparedIntent, Receipt, ReceiptData,
            UsageVector,
        },
        types::{
            BoundedText, DecimalUsage, PrincipalId, ProducerReceiptId, SchemaVersion, SqlU64,
            TaskId,
        },
    };
    fn receipt(prepared: &PreparedIntent, output: ReceiptOutput) -> Receipt {
        Receipt::new(ReceiptData {
            schema_version: SchemaVersion::V1,
            producer_receipt_id: ProducerReceiptId::new(prepared.action.dispatch_id.as_str())
                .unwrap(),
            task_id: prepared.action.task_id.clone(),
            intent_id: prepared.intent_id.clone(),
            dispatch_id: prepared.action.dispatch_id.clone(),
            action_digest: prepared.action_digest.clone(),
            usage: ActualUsage::Known {
                usage: UsageVector {
                    model_requests: DecimalUsage::new("0").unwrap(),
                    tool_calls: DecimalUsage::new("1").unwrap(),
                    spend_micros_usd: DecimalUsage::new("0").unwrap(),
                },
            },
            output,
        })
        .unwrap()
    }
    #[test]
    fn rejects_wrong_candidate_even_when_receipts_and_artifact_match() {
        let dir = std::env::temp_dir().join(format!("metron-verifier-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).unwrap();
        let task = TaskId::new("task").unwrap();
        let principal = PrincipalId::new("principal").unwrap();
        let genesis = content_digest(b"genesis");
        let prepare = |ordinal, kind| {
            PreparedIntent::materialize(
                &Intent {
                    schema_version: SchemaVersion::V1,
                    task_id: task.clone(),
                    ordinal,
                    kind,
                },
                &principal,
                &genesis,
            )
            .unwrap()
        };
        let fixture = "{\"values\":[17,-4,9]}\n";
        let digest = content_digest(fixture.as_bytes());
        let read = receipt(
            &prepare(0, IntentKind::ReadFixture),
            ReceiptOutput::Fixture {
                bytes: BoundedText::new(fixture).unwrap(),
                digest: digest.clone(),
            },
        );
        let model_prepared = prepare(
            1,
            IntentKind::RequestModel {
                input_digest: digest.clone(),
                context: BoundedText::new(fixture).unwrap(),
            },
        );
        let candidate = CandidateResult {
            count: 3,
            sum: 23,
            input_digest: digest,
        };
        let model = receipt(
            &model_prepared,
            ReceiptOutput::Model {
                candidate: candidate.clone(),
            },
        );
        let publish_prepared = prepare(2, IntentKind::PublishArtifact { candidate });
        let ApprovedRequest::PublishArtifact { plan } = &publish_prepared.request else {
            panic!("expected plan")
        };
        crate::publication::publish(&dir, plan, &mut || {}).unwrap();
        let publication = receipt(
            &publish_prepared,
            ReceiptOutput::Published {
                artifact_id: plan.artifact_id.clone(),
                content_digest: plan.content_digest.clone(),
                content_length: plan.content_length,
            },
        );
        let snapshot = VerificationSnapshot {
            evidence_revision: SqlU64::new(3).unwrap(),
            read_evidence: Some(read.evidence_id().unwrap()),
            model_evidence: Some(model.evidence_id().unwrap()),
            publication_evidence: Some(publication.evidence_id().unwrap()),
            read: Some(read),
            model: Some(model),
            publication: Some(publication),
            plan: Some(plan.clone()),
            unresolved: false,
            conflict: false,
        };
        let (selection, accepted) = verify(&dir, &snapshot).unwrap();
        assert!(!accepted);
        assert_eq!(selection.plan_digest, publish_prepared.request_digest);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
