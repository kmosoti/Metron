//! Bounded synchronous orchestration. All external calls follow durable admission.
use crate::{
    driver::{DriverOutcome, ScriptedDriver},
    policy::{self, AdmissionOutcome, EffectDriver},
    verify,
};
use metron_protocol::{
    domain::{Cognition, DecisionError, Phase, PreparedIntent},
    store::{
        AdmissionCheck, AuditSnapshot, CommandData, DispatchStatus, RuntimeSnapshot, StoreCommand,
        StoreError, StoreOperation, StorePort, StoreResult, WriteOutcome,
    },
    types::{
        ApprovalId, BoundedText, CommandId, MAX_INTERNAL, SchemaVersion, TaskId, WorkerId,
        canonical_bytes, structural_digest,
    },
};
use std::path::Path;

/// Durable checkpoints at which a test observer can block a real child process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Checkpoint {
    OutboxCommitted,
    AdmissionCommitted,
    PublicationSynced,
    ReceiptCommitted,
    VerificationCommitted,
}
pub trait Observer {
    fn checkpoint(&mut self, point: Checkpoint);
}
pub struct NoObserver;
impl Observer for NoObserver {
    fn checkpoint(&mut self, _: Checkpoint) {}
}
#[derive(Debug)]
pub enum DriveError {
    Store(StoreError),
    Decision(DecisionError),
    Integrity,
    Invalid,
    Operational,
}
impl From<StoreError> for DriveError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriveStatus {
    Complete,
    Blocked,
    Rejected,
    Denied,
    MultipleEvidence,
}

fn command(operation: StoreOperation) -> Result<StoreCommand, DriveError> {
    let bytes = canonical_bytes(&operation, MAX_INTERNAL).map_err(|_| DriveError::Invalid)?;
    let id = structural_digest("command/v1", &[&bytes]).map_err(|_| DriveError::Invalid)?;
    StoreCommand::new(CommandData {
        schema_version: SchemaVersion::V1,
        command_id: CommandId::new(id.as_str()).map_err(|_| DriveError::Invalid)?,
        operation,
    })
    .map_err(|_| DriveError::Invalid)
}
fn commit<S: StorePort>(
    store: &mut S,
    operation: StoreOperation,
) -> Result<Option<StoreResult>, DriveError> {
    match store.apply(command(operation)?)? {
        WriteOutcome::Committed(reply) => Ok(Some(reply.result)),
        WriteOutcome::NotCommitted(_) => Ok(None),
    }
}

/// Drive at most a fixed number of steps; callers never poll or advance the clock implicitly.
pub fn drive<S: StorePort>(
    store: &mut S,
    task: &TaskId,
    engine: &dyn Cognition,
    run_dir: &Path,
    observer: &mut dyn Observer,
) -> Result<DriveStatus, DriveError> {
    let worker = WorkerId::new("host").map_err(|_| DriveError::Invalid)?;
    for _ in 0..128 {
        let (projection, pending, intents) = {
            let mut snapshot = store.audit()?;
            let projection = snapshot.projection(task)?;
            let pending = snapshot.pending_inbox(task)?;
            let mut cursor = None;
            let mut intents = Vec::new();
            loop {
                let page = snapshot.intents(task, cursor)?;
                intents.extend(page.records);
                cursor = page.next;
                if cursor.is_none() {
                    break;
                }
            }
            (projection, pending, intents)
        };
        if let Some(input) = pending {
            let transition = match engine.transition(&projection.state, &input.event) {
                Ok(t) => t,
                Err(DecisionError::DecisionLimit) => return Ok(DriveStatus::Blocked),
                Err(e) => return Err(DriveError::Decision(e)),
            };
            let is_decision = matches!(
                &transition,
                metron_protocol::domain::Transition::Decision { .. }
            );
            if commit(
                store,
                StoreOperation::CommitTransition {
                    task_id: task.clone(),
                    expected_revision: projection.state.revision,
                    inbox_id: input.id,
                    inbox_digest: input.digest,
                    transition,
                },
            )?
            .is_none()
            {
                return Ok(DriveStatus::Blocked);
            }
            if is_decision {
                observer.checkpoint(Checkpoint::OutboxCommitted);
            }
            continue;
        }
        if projection.state.phase == Phase::AwaitingVerification {
            let (selection, accepted) =
                verify::verify(run_dir, &projection.verification).map_err(|error| match error {
                    verify::VerifyError::InvalidEvidence(_) => DriveError::Integrity,
                    verify::VerifyError::Operational => DriveError::Operational,
                })?;
            let reason = BoundedText::new(if accepted {
                "verified"
            } else {
                "candidate or artifact mismatch"
            })
            .map_err(|_| DriveError::Invalid)?;
            if commit(
                store,
                StoreOperation::RecordVerification {
                    task_id: task.clone(),
                    selection,
                    accepted,
                    reason,
                },
            )?
            .is_none()
            {
                return Ok(DriveStatus::Blocked);
            }
            observer.checkpoint(Checkpoint::VerificationCommitted);
            continue;
        }
        if matches!(projection.state.phase, Phase::Completed | Phase::Rejected) {
            if projection.state.phase == Phase::Rejected {
                let mut snapshot = store.audit()?;
                let mut cursor = None;
                loop {
                    let page = snapshot.inbox(task, cursor)?;
                    if page.records.iter().any(|row| {
                        row.status == metron_protocol::store::InboxStatus::Consumed
                            && matches!(
                                row.input.event,
                                metron_protocol::domain::AgentEvent::Denied { .. }
                            )
                    }) {
                        return Ok(DriveStatus::Denied);
                    }
                    cursor = page.next;
                    if cursor.is_none() {
                        break;
                    }
                }
            }
            return Ok(
                if projection.state.phase == Phase::Completed
                    && !projection.verification.unresolved
                    && !projection.verification.conflict
                    && !projection.task_account.integrity_hold
                    && !projection.task_account.incomplete
                    && !projection.task_account.breached
                    && !projection.principal_account.integrity_hold
                    && !projection.principal_account.incomplete
                    && !projection.principal_account.breached
                    && projection.task_account.reserved.model_requests.get() == 0
                    && projection.task_account.reserved.tool_calls.get() == 0
                    && projection.task_account.reserved.spend_micros_usd.get() == 0
                    && projection.principal_account.reserved.model_requests.get() == 0
                    && projection.principal_account.reserved.tool_calls.get() == 0
                    && projection.principal_account.reserved.spend_micros_usd.get() == 0
                {
                    DriveStatus::Complete
                } else {
                    DriveStatus::Rejected
                },
            );
        }
        let intent = match intents
            .into_iter()
            .find(|i| !matches!(i.status, DispatchStatus::Settled | DispatchStatus::Denied))
        {
            Some(i) => i,
            None => return Ok(DriveStatus::Blocked),
        };
        if intent.prepared.is_none() {
            let prepared = PreparedIntent::materialize(
                &intent.intent,
                &projection.task.data().principal_id,
                &projection.state.genesis_digest,
            )
            .map_err(|_| DriveError::Invalid)?;
            if commit(
                store,
                StoreOperation::PrepareIntent {
                    task_id: task.clone(),
                    prepared,
                },
            )?
            .is_none()
            {
                return Ok(DriveStatus::Blocked);
            }
            continue;
        }
        let prepared = intent.prepared.clone().ok_or(DriveError::Integrity)?;
        if projection.genesis.require_publication_approval
            && !matches!(
                intent.status,
                DispatchStatus::Dispatched | DispatchStatus::Uncertain
            )
            && matches!(
                &prepared.request,
                metron_protocol::domain::ApprovedRequest::PublishArtifact { .. }
            )
        {
            match &intent.approval {
                None => {
                    let request_id = ApprovalId::new(intent.intent_id.as_str())
                        .map_err(|_| DriveError::Invalid)?;
                    commit(
                        store,
                        StoreOperation::CreateApprovalRequest {
                            task_id: task.clone(),
                            request_id,
                            intent_id: intent.intent_id,
                            action_digest: prepared.action_digest,
                        },
                    )?;
                    return Ok(DriveStatus::Blocked);
                }
                Some(approval)
                    if approval.decision.is_none() || approval.expires_at <= projection.tick =>
                {
                    return Ok(DriveStatus::Blocked);
                }
                Some(approval)
                    if approval.decision
                        == Some(metron_protocol::store::ApprovalDecision::Deny) =>
                {
                    if commit(
                        store,
                        StoreOperation::Deny {
                            task_id: task.clone(),
                            intent_id: intent.intent_id,
                            reason: BoundedText::new("approval denied")
                                .map_err(|_| DriveError::Invalid)?,
                        },
                    )?
                    .is_none()
                    {
                        return Ok(DriveStatus::Blocked);
                    }
                    continue;
                }
                _ => {}
            }
        }
        if intent.status == DispatchStatus::Dispatched {
            if let Some(lease) = &intent.lease
                && lease.expires_at <= projection.tick
            {
                commit(
                    store,
                    StoreOperation::RecordUncertainty {
                        task_id: task.clone(),
                        intent_id: intent.intent_id.clone(),
                        dispatch_id: prepared.action.dispatch_id.clone(),
                        lease_epoch: lease.epoch,
                        reason: BoundedText::new("dispatch lease expired")
                            .map_err(|_| DriveError::Invalid)?,
                    },
                )?;
                continue;
            }
            return Ok(DriveStatus::Blocked);
        }
        if intent.status == DispatchStatus::Uncertain {
            return Ok(DriveStatus::Blocked);
        }
        let lease_epoch = match intent.lease.as_ref() {
            Some(lease) if lease.worker == worker && lease.expires_at > projection.tick => {
                lease.epoch
            }
            _ => match commit(
                store,
                StoreOperation::Claim {
                    task_id: task.clone(),
                    intent_id: intent.intent_id.clone(),
                    worker: worker.clone(),
                    expected_epoch: intent.lease_epoch,
                },
            )? {
                Some(StoreResult::Claimed { lease_epoch, .. }) => lease_epoch,
                _ => return Ok(DriveStatus::Blocked),
            },
        };
        let grant = {
            let mut snapshot = store.audit()?;
            snapshot.grant(&projection.task.data().principal_id)?
        };
        let reservation_id =
            structural_digest("dispatch-id/v1", &[intent.intent_id.as_str().as_bytes()])
                .map_err(|_| DriveError::Invalid)?;
        let admission = command(StoreOperation::Admit {
            check: AdmissionCheck {
                intent_id: intent.intent_id.clone(),
                prepared: prepared.clone(),
                worker: worker.clone(),
                lease_epoch,
                expected_tick: projection.tick,
                grant_revision: grant.revision,
                approval_id: intent.approval.as_ref().map(|a| a.request_id.clone()),
                reservation_id,
            },
        })?;
        let effect = match policy::admit(store, admission, false, &projection, &intent, &grant)? {
            AdmissionOutcome::Authorized(effect) => *effect,
            AdmissionOutcome::Denied => continue,
            AdmissionOutcome::Blocked => return Ok(DriveStatus::Blocked),
        };
        observer.checkpoint(Checkpoint::AdmissionCommitted);
        let mut callback = || observer.checkpoint(Checkpoint::PublicationSynced);
        let outcome = ScriptedDriver {
            run_dir: run_dir.to_path_buf(),
            publication_synced: &mut callback,
        }
        .execute(effect);
        match outcome {
            DriverOutcome::Receipt(receipt) => {
                let result = commit(
                    store,
                    StoreOperation::RecordReceipt {
                        receipt: *receipt,
                        lease_epoch,
                    },
                )?;
                if result.is_none() {
                    return Ok(DriveStatus::Blocked);
                }
                observer.checkpoint(Checkpoint::ReceiptCommitted);
            }
            DriverOutcome::Uncertain => {
                commit(
                    store,
                    StoreOperation::RecordUncertainty {
                        task_id: task.clone(),
                        intent_id: intent.intent_id,
                        dispatch_id: prepared.action.dispatch_id,
                        lease_epoch,
                        reason: BoundedText::new("effect outcome uncertain")
                            .map_err(|_| DriveError::Invalid)?,
                    },
                )?;
                return Ok(DriveStatus::Blocked);
            }
        }
    }
    Ok(DriveStatus::Blocked)
}

/// Reconcile only a previously dispatched publication. It does not drive cognition.
pub fn reconcile<S: StorePort>(
    store: &mut S,
    task: &TaskId,
    intent_id: &metron_protocol::types::Digest,
    run_dir: &Path,
) -> Result<DriveStatus, DriveError> {
    use metron_protocol::{
        domain::{ActualUsage, ApprovedRequest, Receipt, ReceiptData, ReceiptOutput, UsageVector},
        types::{DecimalUsage, ProducerReceiptId},
    };
    let worker = WorkerId::new("reconciler").map_err(|_| DriveError::Invalid)?;
    let (intent, evidence, tick) = {
        let mut snapshot = store.audit()?;
        let tick = snapshot.projection(task)?.tick;
        let intent = snapshot.intent(intent_id)?.ok_or(DriveError::Invalid)?;
        let mut evidence = Vec::new();
        let mut cursor = None;
        loop {
            let page = snapshot.evidence(intent_id, cursor)?;
            evidence.extend(page.records);
            cursor = page.next;
            if evidence.len() > 1 || cursor.is_none() {
                break;
            }
        }
        (intent, evidence, tick)
    };
    if intent.status == DispatchStatus::Dispatched {
        let Some(lease) = intent.lease.as_ref() else {
            return Ok(DriveStatus::Blocked);
        };
        if lease.expires_at > tick {
            return Ok(DriveStatus::Blocked);
        }
        let Some(dispatch_id) = intent.dispatch_id.clone() else {
            return Err(DriveError::Integrity);
        };
        if !matches!(
            commit(
                store,
                StoreOperation::RecordUncertainty {
                    task_id: task.clone(),
                    intent_id: intent_id.clone(),
                    dispatch_id,
                    lease_epoch: lease.epoch,
                    reason: BoundedText::new("dispatch lease expired")
                        .map_err(|_| DriveError::Invalid)?,
                }
            )?,
            Some(StoreResult::UncertaintyRecorded)
        ) {
            return Ok(DriveStatus::Blocked);
        }
        let status = {
            let mut snapshot = store.audit()?;
            snapshot
                .intent(intent_id)?
                .ok_or(DriveError::Integrity)?
                .status
        };
        if status != DispatchStatus::Uncertain {
            return Ok(DriveStatus::Blocked);
        }
        return reconcile(store, task, intent_id, run_dir);
    }
    let prepared = intent.prepared.ok_or(DriveError::Integrity)?;
    let plan = match &prepared.request {
        ApprovedRequest::PublishArtifact { plan } => plan,
        _ => return Ok(DriveStatus::Blocked),
    };
    let existing = match supported_publication_evidence(&evidence, &prepared) {
        Ok(existing) => existing,
        Err(EvidenceIssue::Multiple) => return Ok(DriveStatus::MultipleEvidence),
        Err(EvidenceIssue::Unsupported) => return Ok(DriveStatus::Blocked),
    };
    if intent.status == DispatchStatus::Settled {
        if existing != intent.selected_evidence || existing.is_none() {
            return Err(DriveError::Integrity);
        }
        return if crate::publication::inspect(run_dir, plan).map_err(|_| DriveError::Integrity)? {
            Ok(DriveStatus::Complete)
        } else {
            Err(DriveError::Integrity)
        };
    }
    if intent.status != DispatchStatus::Uncertain {
        return Ok(DriveStatus::Blocked);
    }
    let lease_epoch = match commit(
        store,
        StoreOperation::ClaimReconciliation {
            task_id: task.clone(),
            intent_id: intent_id.clone(),
            worker: worker.clone(),
            expected_epoch: intent.reconciliation_epoch,
            expected_evidence_revision: intent.evidence_revision,
        },
    )? {
        Some(StoreResult::ReconciliationClaimed { lease_epoch, .. }) => lease_epoch,
        _ => return Ok(DriveStatus::Blocked),
    };
    if !crate::publication::reconcile(run_dir, plan).map_err(|_| DriveError::Integrity)? {
        return Ok(DriveStatus::Blocked);
    }
    let usage = ActualUsage::Known {
        usage: UsageVector {
            model_requests: DecimalUsage::new("0").map_err(|_| DriveError::Invalid)?,
            tool_calls: DecimalUsage::new("1").map_err(|_| DriveError::Invalid)?,
            spend_micros_usd: DecimalUsage::new("0").map_err(|_| DriveError::Invalid)?,
        },
    };
    let receipt = Receipt::new(ReceiptData {
        schema_version: SchemaVersion::V1,
        producer_receipt_id: ProducerReceiptId::new(prepared.action.dispatch_id.as_str())
            .map_err(|_| DriveError::Invalid)?,
        task_id: task.clone(),
        intent_id: intent_id.clone(),
        dispatch_id: prepared.action.dispatch_id.clone(),
        action_digest: prepared.action_digest.clone(),
        usage,
        output: ReceiptOutput::Published {
            artifact_id: plan.artifact_id.clone(),
            content_digest: plan.content_digest.clone(),
            content_length: plan.content_length,
        },
    })
    .map_err(|_| DriveError::Invalid)?;
    let (evidence_id, reconciled_receipt) = match existing {
        Some(id) => (id, None),
        None => (
            receipt.evidence_id().map_err(|_| DriveError::Invalid)?,
            Some(receipt),
        ),
    };
    match commit(
        store,
        StoreOperation::SettleReconciliation {
            task_id: task.clone(),
            intent_id: intent_id.clone(),
            worker,
            lease_epoch,
            expected_evidence_revision: intent.evidence_revision,
            evidence_id,
            reconciled_receipt,
        },
    )? {
        Some(StoreResult::ReconciliationSettled { .. }) => Ok(DriveStatus::Complete),
        _ => Ok(DriveStatus::Blocked),
    }
}

#[derive(Debug, Eq, PartialEq)]
enum EvidenceIssue {
    Multiple,
    Unsupported,
}

/// Existing evidence must never be replaced by an inferred tariff receipt.
fn supported_publication_evidence(
    evidence: &[metron_protocol::store::EvidenceSnapshot],
    prepared: &PreparedIntent,
) -> Result<Option<metron_protocol::types::Digest>, EvidenceIssue> {
    use metron_protocol::domain::{ActualUsage, ApprovedRequest, ReceiptOutput};
    let ApprovedRequest::PublishArtifact { plan } = &prepared.request else {
        return Err(EvidenceIssue::Unsupported);
    };
    if evidence.len() > 1 {
        return Err(EvidenceIssue::Multiple);
    }
    let Some(evidence) = evidence.first() else {
        return Ok(None);
    };
    let data = evidence.receipt.data();
    let supported_usage = match &data.usage {
        ActualUsage::Known { usage } => [
            &usage.model_requests,
            &usage.tool_calls,
            &usage.spend_micros_usd,
        ]
        .iter()
        .all(|value| value.as_u64().is_some_and(|n| n <= i64::MAX as u64)),
        ActualUsage::Unknown { .. } => false,
    };
    let published = matches!(&data.output, ReceiptOutput::Published { artifact_id, content_digest, content_length } if *artifact_id == plan.artifact_id && *content_digest == plan.content_digest && *content_length == plan.content_length);
    if evidence.conflict
        || !supported_usage
        || !published
        || data.task_id != prepared.action.task_id
        || data.intent_id != prepared.intent_id
        || data.dispatch_id != prepared.action.dispatch_id
        || data.action_digest != prepared.action_digest
        || evidence
            .receipt
            .evidence_id()
            .map_err(|_| EvidenceIssue::Unsupported)?
            != evidence.evidence_id
    {
        return Err(EvidenceIssue::Unsupported);
    }
    Ok(Some(evidence.evidence_id.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use metron_protocol::{
        domain::{
            ActualUsage, CandidateResult, Intent, IntentKind, Receipt, ReceiptData, ReceiptOutput,
            UsageVector,
        },
        store::EvidenceSnapshot,
        types::{DecimalUsage, PrincipalId, ProducerReceiptId, content_digest},
    };
    #[test]
    fn reconciliation_never_infers_away_unknown_or_conflicting_usage() {
        let digest = content_digest(b"fixture");
        let intent = Intent {
            schema_version: SchemaVersion::V1,
            task_id: TaskId::new("task").unwrap(),
            ordinal: 2,
            kind: IntentKind::PublishArtifact {
                candidate: CandidateResult {
                    count: 3,
                    sum: 22,
                    input_digest: digest.clone(),
                },
            },
        };
        let prepared =
            PreparedIntent::materialize(&intent, &PrincipalId::new("principal").unwrap(), &digest)
                .unwrap();
        let metron_protocol::domain::ApprovedRequest::PublishArtifact { plan } = &prepared.request
        else {
            panic!("expected plan")
        };
        let make = |usage| {
            Receipt::new(ReceiptData {
                schema_version: SchemaVersion::V1,
                producer_receipt_id: ProducerReceiptId::new("producer").unwrap(),
                task_id: intent.task_id.clone(),
                intent_id: prepared.intent_id.clone(),
                dispatch_id: prepared.action.dispatch_id.clone(),
                action_digest: prepared.action_digest.clone(),
                usage,
                output: ReceiptOutput::Published {
                    artifact_id: plan.artifact_id.clone(),
                    content_digest: plan.content_digest.clone(),
                    content_length: plan.content_length,
                },
            })
            .unwrap()
        };
        let unknown = make(ActualUsage::Unknown {
            reason: BoundedText::new("unknown charge").unwrap(),
            evidence: None,
        });
        let row = EvidenceSnapshot {
            evidence_id: unknown.evidence_id().unwrap(),
            receipt: unknown,
            selected: false,
            conflict: false,
        };
        assert!(supported_publication_evidence(&[row], &prepared).is_err());
        let known = make(ActualUsage::Known {
            usage: UsageVector {
                model_requests: DecimalUsage::new("0").unwrap(),
                tool_calls: DecimalUsage::new("1").unwrap(),
                spend_micros_usd: DecimalUsage::new("0").unwrap(),
            },
        });
        let id = known.evidence_id().unwrap();
        let row = EvidenceSnapshot {
            evidence_id: id.clone(),
            receipt: known,
            selected: false,
            conflict: false,
        };
        assert_eq!(
            supported_publication_evidence(std::slice::from_ref(&row), &prepared).unwrap(),
            Some(id)
        );
        let mut other_data = row.receipt.data().clone();
        other_data.producer_receipt_id = ProducerReceiptId::new("other-producer").unwrap();
        let other = Receipt::new(other_data).unwrap();
        let other = EvidenceSnapshot {
            evidence_id: other.evidence_id().unwrap(),
            receipt: other,
            selected: false,
            conflict: false,
        };
        assert_eq!(
            supported_publication_evidence(&[row, other], &prepared),
            Err(EvidenceIssue::Multiple)
        );
    }
}
