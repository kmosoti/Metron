//! Bounded synchronous orchestration. All external calls follow durable admission.
use crate::{
    driver::DriverOutcome,
    policy::{self, AdmissionOutcome, EffectDriver},
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

/// Observation checkpoints around durable admission, effect return and settlement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Checkpoint {
    OutboxCommitted,
    AdmissionCommitted,
    EffectReturned,
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

fn command(operation: StoreOperation, version: SchemaVersion) -> Result<StoreCommand, DriveError> {
    let bytes = canonical_bytes(&operation, MAX_INTERNAL).map_err(|_| DriveError::Invalid)?;
    let id = structural_digest("command/v1", &[&bytes]).map_err(|_| DriveError::Invalid)?;
    StoreCommand::new(CommandData {
        schema_version: version,
        command_id: CommandId::new(id.as_str()).map_err(|_| DriveError::Invalid)?,
        operation,
    })
    .map_err(|_| DriveError::Invalid)
}
fn commit<S: StorePort>(
    store: &mut S,
    version: SchemaVersion,
    operation: StoreOperation,
) -> Result<Option<StoreResult>, DriveError> {
    match store.apply(command(operation, version)?)? {
        WriteOutcome::Committed(reply) => Ok(Some(reply.result)),
        WriteOutcome::NotCommitted(_) => Ok(None),
    }
}

/// Drive with caller-supplied effects, called only after fresh admission.
pub fn drive_with_driver<S: StorePort>(
    store: &mut S,
    task: &TaskId,
    engine: &dyn Cognition,
    observer: &mut dyn Observer,
    driver: &mut dyn EffectDriver<Outcome = DriverOutcome>,
) -> Result<DriveStatus, DriveError> {
    drive_inner(store, task, engine, observer, driver, None)
}

/// Independent host verifier; it receives only a coherent persisted projection.
/// A model finish proposal never supplies this result.
pub trait CodingVerifier {
    fn verify(
        &mut self,
        projection: &metron_protocol::Projection,
    ) -> Result<(metron_protocol::CodingVerificationSelection, bool), DriveError>;
}
pub fn drive_coding_with_driver<S: StorePort>(
    store: &mut S,
    task: &TaskId,
    engine: &dyn Cognition,
    observer: &mut dyn Observer,
    driver: &mut dyn EffectDriver<Outcome = DriverOutcome>,
    verifier: &mut dyn CodingVerifier,
) -> Result<DriveStatus, DriveError> {
    drive_inner(store, task, engine, observer, driver, Some(verifier))
}

fn drive_inner<S: StorePort>(
    store: &mut S,
    task: &TaskId,
    engine: &dyn Cognition,
    observer: &mut dyn Observer,
    local_driver: &mut dyn EffectDriver<Outcome = DriverOutcome>,
    mut coding_verifier: Option<&mut dyn CodingVerifier>,
) -> Result<DriveStatus, DriveError> {
    let version = {
        let mut snapshot = store.audit()?;
        snapshot.projection(task)?.task.data().schema_version
    };
    let worker = WorkerId::new("host").map_err(|_| DriveError::Invalid)?;
    let max_turns = if version == SchemaVersion::V3 {
        528
    } else {
        128
    };
    for _ in 0..max_turns {
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
                version,
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
        if projection.state.phase == Phase::AwaitingVerification
            && projection.task.coding().is_some()
        {
            let Some(verifier) = coding_verifier.as_mut() else {
                return Ok(DriveStatus::Blocked);
            };
            let (selection, accepted) = verifier.verify(&projection)?;
            if commit(
                store,
                version,
                StoreOperation::RecordCodingVerification {
                    task_id: task.clone(),
                    selection,
                    accepted,
                    reason: BoundedText::new(if accepted {
                        "independent coding acceptance passed"
                    } else {
                        "independent coding acceptance failed"
                    })
                    .map_err(|_| DriveError::Invalid)?,
                },
            )?
            .is_none()
            {
                return Ok(DriveStatus::Blocked);
            }
            observer.checkpoint(Checkpoint::VerificationCommitted);
            continue;
        }
        if projection.state.phase == Phase::AwaitingVerification {
            return Ok(DriveStatus::Blocked);
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
                version,
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
        let needs_approval = match &prepared.request {
            metron_protocol::ApprovedRequest::Coding { step, .. } => projection
                .genesis
                .coding_policy
                .as_ref()
                .is_some_and(|p| step.permitted(p) && step.requires_approval(p)),
            metron_protocol::ApprovedRequest::PublishArtifact { .. } => {
                projection.genesis.require_publication_approval
            }
            _ => false,
        };
        if needs_approval
            && !matches!(
                intent.status,
                DispatchStatus::Dispatched | DispatchStatus::Uncertain
            )
        {
            match &intent.approval {
                None => {
                    let request_id = ApprovalId::new(intent.intent_id.as_str())
                        .map_err(|_| DriveError::Invalid)?;
                    commit(
                        store,
                        version,
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
                        version,
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
                    version,
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
                version,
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
        let admission = command(
            StoreOperation::Admit {
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
            },
            version,
        )?;
        let effect = match policy::admit(store, admission, false, &projection, &intent, &grant)? {
            AdmissionOutcome::Authorized(effect) => *effect,
            AdmissionOutcome::Denied => continue,
            AdmissionOutcome::Blocked => return Ok(DriveStatus::Blocked),
        };
        observer.checkpoint(Checkpoint::AdmissionCommitted);
        let outcome = local_driver.execute(effect);
        observer.checkpoint(Checkpoint::EffectReturned);
        match outcome {
            DriverOutcome::Receipt(receipt) => {
                let result = commit(
                    store,
                    version,
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
                    version,
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
