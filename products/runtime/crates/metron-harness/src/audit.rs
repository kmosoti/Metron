//! Independent reconstruction of durable state from the authoritative journal.
use metron_protocol::*;
use std::collections::{BTreeMap, BTreeSet};

/// Results for checks actually performed against one coherent snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditResult {
    pub chain_valid: bool,
    pub accounting_valid: bool,
    pub projection_valid: bool,
}

struct Reconstruction {
    task: Task,
    genesis: Genesis,
    state: AgentState,
    inputs: BTreeMap<Digest, InboxSnapshot>,
    intents: BTreeMap<Digest, IntentSnapshot>,
    evidence: BTreeMap<Digest, EvidenceSnapshot>,
    tick: SqlU64,
    verification: VerificationSnapshot,
    grant: GrantSnapshot,
    seen_receipts: BTreeSet<Digest>,
    uncertain: BTreeSet<Digest>,
    last_seq: SqlU64,
    head: Digest,
}

fn input_for(
    task: &Task,
    seq: SqlU64,
    source: InboxSource,
    evidence_id: Option<Digest>,
    event: AgentEvent,
) -> Result<InboxInput, ValidationError> {
    let id = inbox_id(&task.data().task_id, seq, 0)?;
    let mut input = InboxInput {
        id,
        task_id: task.data().task_id.clone(),
        principal_id: task.data().principal_id.clone(),
        source_seq: seq,
        output_ordinal: 0,
        source,
        evidence_id,
        event,
        digest: content_digest(&[]),
    };
    input.digest = inbox_digest(&input)?;
    Ok(input)
}

fn parse_journal(
    snapshot: &mut impl RuntimeSnapshot,
    task_id: &TaskId,
    accounts: &Accounts,
) -> Result<Option<Reconstruction>, StoreError> {
    let mut after = None;
    let mut expected = Some(1_u64);
    let mut previous = None;
    let mut target = None::<Reconstruction>;
    let mut tick = SqlU64::new(0).map_err(StoreError::Validation)?;
    let mut seen_commands = BTreeSet::new();
    let mut operations = BTreeMap::<Digest, StoreResult>::new();
    let mut grants = BTreeMap::<PrincipalId, GrantSnapshot>::new();
    let mut global_evidence = BTreeSet::<Digest>::new();
    loop {
        let page = snapshot.journal_page(after)?;
        if page.records.len() > MAX_PAGE_RECORDS {
            return Ok(None);
        }
        let mut bytes = 0_usize;
        for record in &page.records {
            let data = record.data();
            bytes = match canonical_bytes(record, MAX_INTERNAL)
                .ok()
                .and_then(|value| bytes.checked_add(value.len()))
            {
                Some(n) => n,
                None => return Ok(None),
            };
            if bytes > MAX_PAGE_BYTES || Some(data.seq.get()) != expected {
                return Ok(None);
            }
            let JournalEvent::CommandCommitted { command, result } = &data.event;
            if !compatible(&command.data().operation, result) {
                return Ok(None);
            }
            if !seen_commands.insert(command.data().command_id.clone()) {
                return Ok(None);
            }
            let Ok(bytes) = canonical_bytes(&command.data().operation, MAX_INTERNAL) else {
                return Ok(None);
            };
            let key = content_digest(&bytes);
            let duplicate_operation = match operations.get(&key) {
                Some(prior) if prior == result => true,
                Some(_)
                    if matches!(
                        &command.data().operation,
                        StoreOperation::RecordReceipt { .. }
                    ) =>
                {
                    operations.insert(key, result.clone());
                    false
                }
                Some(_) => return Ok(None),
                None => {
                    operations.insert(key, result.clone());
                    false
                }
            };
            if let (
                StoreOperation::AdvanceTick {
                    expected_tick,
                    new_tick,
                },
                StoreResult::TickAdvanced { tick: recorded },
            ) = (&command.data().operation, result)
                && !duplicate_operation
            {
                if *expected_tick != tick || *new_tick <= tick || *recorded != *new_tick {
                    return Ok(None);
                }
                tick = *new_tick;
            }
            if expected == Some(1) {
                let (
                    StoreOperation::CreateTask { task, genesis },
                    StoreResult::TaskCreated { genesis_digest },
                ) = (&command.data().operation, result)
                else {
                    return Ok(None);
                };
                let Ok(derived) = journal_genesis(task, genesis) else {
                    return Ok(None);
                };
                if derived != *genesis_digest {
                    return Ok(None);
                }
                previous = Some(derived);
            }
            if previous.as_ref() != Some(&data.previous)
                || journal_entry(&data.previous, data.seq, &data.event)
                    .ok()
                    .as_ref()
                    != Some(&data.digest)
            {
                return Ok(None);
            }
            if let (
                StoreOperation::RevokeGrant {
                    principal_id,
                    expected_revision,
                },
                StoreResult::GrantRevoked { revision },
            ) = (&command.data().operation, result)
                && !duplicate_operation
            {
                let Some(grant) = grants.get_mut(principal_id) else {
                    return Ok(None);
                };
                if grant.revision != *expected_revision
                    || expected_revision.checked_add(1).ok() != Some(*revision)
                {
                    return Ok(None);
                }
                grant.revision = *revision;
                grant.revoked = true;
            }
            if let (
                StoreOperation::CreateTask { task, genesis },
                StoreResult::TaskCreated { genesis_digest },
            ) = (&command.data().operation, result)
                && !duplicate_operation
            {
                let initial_grant = GrantSnapshot {
                    principal_id: task.data().principal_id.clone(),
                    revision: SqlU64::new(0).map_err(StoreError::Validation)?,
                    revoked: false,
                };
                grants
                    .entry(task.data().principal_id.clone())
                    .or_insert(initial_grant);
                if journal_genesis(task, genesis).ok().as_ref() != Some(genesis_digest) {
                    return Ok(None);
                }
                if &task.data().task_id == task_id {
                    if target.is_some() {
                        return Ok(None);
                    }
                    let Ok(input) = input_for(
                        task,
                        data.seq,
                        InboxSource::TaskAccepted,
                        None,
                        AgentEvent::TaskAccepted,
                    ) else {
                        return Ok(None);
                    };
                    let state = AgentState {
                        task_id: task_id.clone(),
                        genesis_digest: genesis_digest.clone(),
                        max_decision_steps: task.data().max_decision_steps,
                        revision: SqlU64::new(0).map_err(StoreError::Validation)?,
                        decision_steps: 0,
                        phase: Phase::New,
                        pending_intent: None,
                    };
                    let mut inputs = BTreeMap::new();
                    inputs.insert(
                        input.id.clone(),
                        InboxSnapshot {
                            input,
                            status: InboxStatus::Pending,
                            transition: None,
                        },
                    );
                    target = Some(Reconstruction {
                        task: task.clone(),
                        genesis: genesis.clone(),
                        state,
                        inputs,
                        intents: BTreeMap::new(),
                        evidence: BTreeMap::new(),
                        tick,
                        verification: VerificationSnapshot {
                            evidence_revision: SqlU64::new(0).map_err(StoreError::Validation)?,
                            read: None,
                            model: None,
                            publication: None,
                            read_evidence: None,
                            model_evidence: None,
                            publication_evidence: None,
                            plan: None,
                            unresolved: false,
                            conflict: false,
                        },
                        grant: grants
                            .get(&task.data().principal_id)
                            .cloned()
                            .ok_or(StoreError::Integrity)?,
                        seen_receipts: BTreeSet::new(),
                        uncertain: BTreeSet::new(),
                        last_seq: data.seq,
                        head: data.digest.clone(),
                    });
                }
            }
            let new_global_evidence = match (&command.data().operation, result) {
                (
                    StoreOperation::RecordReceipt { .. },
                    StoreResult::ReceiptRecorded { evidence_id, .. },
                ) => global_evidence.insert(evidence_id.clone()),
                (
                    StoreOperation::SettleReconciliation {
                        reconciled_receipt: Some(_),
                        ..
                    },
                    StoreResult::ReceiptRecorded { evidence_id, .. },
                ) => global_evidence.insert(evidence_id.clone()),
                (
                    StoreOperation::SettleReconciliation {
                        reconciled_receipt: Some(_),
                        evidence_id,
                        ..
                    },
                    StoreResult::ReconciliationSettled { .. },
                ) => global_evidence.insert(evidence_id.clone()),
                _ => false,
            };
            if let Some(state) = &mut target {
                state.tick = tick;
                let operation_task = match &command.data().operation {
                    StoreOperation::CreateTask { task, .. } => Some(&task.data().task_id),
                    StoreOperation::CommitTransition { task_id, .. }
                    | StoreOperation::PrepareIntent { task_id, .. }
                    | StoreOperation::Claim { task_id, .. }
                    | StoreOperation::Deny { task_id, .. }
                    | StoreOperation::CreateApprovalRequest { task_id, .. }
                    | StoreOperation::RecordUncertainty { task_id, .. }
                    | StoreOperation::ClaimReconciliation { task_id, .. }
                    | StoreOperation::SettleReconciliation { task_id, .. }
                    | StoreOperation::RecordVerification { task_id, .. } => Some(task_id),
                    StoreOperation::Admit { check } => Some(&check.prepared.action.task_id),
                    StoreOperation::RecordReceipt { receipt, .. } => Some(&receipt.data().task_id),
                    StoreOperation::RecordApproval { request_id, .. }
                        if state.intents.values().any(|i| {
                            i.approval
                                .as_ref()
                                .is_some_and(|a| &a.request_id == request_id)
                        }) =>
                    {
                        Some(&state.task.data().task_id)
                    }
                    _ => None,
                };
                if operation_task == Some(&state.task.data().task_id)
                    && command.data().schema_version != state.task.data().schema_version
                {
                    return Ok(None);
                }
                if duplicate_operation {
                    state.last_seq = data.seq;
                    state.head = data.digest.clone();
                    previous = Some(data.digest.clone());
                    expected = data
                        .seq
                        .get()
                        .checked_add(1)
                        .filter(|n| *n <= MAX_SQL_INTEGER);
                    continue;
                }
                let conflict_receipt = match (&command.data().operation, result) {
                    (
                        StoreOperation::RecordReceipt { receipt, .. },
                        StoreResult::ReceiptRecorded { conflict: true, .. },
                    ) => Some(receipt),
                    (
                        StoreOperation::SettleReconciliation {
                            reconciled_receipt: Some(receipt),
                            ..
                        },
                        StoreResult::ReceiptRecorded { conflict: true, .. },
                    ) => Some(receipt),
                    _ => None,
                };
                if let Some(receipt) = conflict_receipt
                    && &receipt.data().task_id != task_id
                    && new_global_evidence
                {
                    let affected: BTreeSet<Digest> = state
                        .evidence
                        .values_mut()
                        .filter(|old| {
                            old.receipt.data().producer_receipt_id
                                == receipt.data().producer_receipt_id
                        })
                        .map(|old| {
                            old.conflict = true;
                            old.receipt.data().intent_id.clone()
                        })
                        .collect();
                    if !affected.is_empty() {
                        state.verification.conflict = true;
                        let Ok(next) = state.verification.evidence_revision.checked_add(1) else {
                            return Ok(None);
                        };
                        state.verification.evidence_revision = next;
                        for id in affected {
                            let Some(item) = state.intents.get_mut(&id) else {
                                return Ok(None);
                            };
                            let Ok(next) = item.evidence_revision.checked_add(1) else {
                                return Ok(None);
                            };
                            item.evidence_revision = next;
                        }
                        for item in state.inputs.values_mut().filter(|item| {
                            item.status == InboxStatus::Pending
                                && item.input.source == InboxSource::Verification
                        }) {
                            item.status = InboxStatus::Superseded;
                        }
                    }
                }
                if !apply_lifecycle(state, &command.data().operation, result) {
                    return Ok(None);
                }
                let Some(grant) = grants.get(&state.grant.principal_id) else {
                    return Ok(None);
                };
                state.grant = grant.clone();
                state.last_seq = data.seq;
                state.head = data.digest.clone();
                let generated = match (&command.data().operation, result) {
                    (StoreOperation::Admit { check }, StoreResult::Denied)
                        if &check.prepared.action.task_id == task_id =>
                    {
                        let Some(reason) = accounts.denied_admission.get(&data.seq.get()) else {
                            return Ok(None);
                        };
                        Some((
                            InboxSource::Denial,
                            None,
                            AgentEvent::Denied {
                                intent_id: check.intent_id.clone(),
                                reason: reason.clone(),
                            },
                        ))
                    }
                    (
                        StoreOperation::Deny {
                            task_id: id,
                            intent_id,
                            reason,
                        },
                        StoreResult::Denied,
                    ) if id == task_id => Some((
                        InboxSource::Denial,
                        None,
                        AgentEvent::Denied {
                            intent_id: intent_id.clone(),
                            reason: reason.clone(),
                        },
                    )),
                    (
                        StoreOperation::RecordUncertainty {
                            task_id: id,
                            intent_id,
                            ..
                        },
                        StoreResult::UncertaintyRecorded,
                    ) if id == task_id && state.uncertain.insert(intent_id.clone()) => Some((
                        InboxSource::Uncertainty,
                        None,
                        AgentEvent::EffectUncertain {
                            intent_id: intent_id.clone(),
                        },
                    )),
                    (
                        StoreOperation::RecordReceipt { receipt, .. },
                        StoreResult::ReceiptRecorded {
                            evidence_id,
                            settled,
                            ..
                        },
                    ) if &receipt.data().task_id == task_id => {
                        if receipt.evidence_id().ok().as_ref() != Some(evidence_id) {
                            return Ok(None);
                        }
                        let is_new = state.seen_receipts.insert(evidence_id.clone());
                        if is_new {
                            for item in state.inputs.values_mut().filter(|item| {
                                item.status == InboxStatus::Pending
                                    && item.input.source == InboxSource::Verification
                            }) {
                                item.status = InboxStatus::Superseded;
                            }
                        }
                        if *settled && is_new {
                            Some((
                                InboxSource::Settlement,
                                Some(evidence_id.clone()),
                                AgentEvent::EffectSettled {
                                    intent_id: receipt.data().intent_id.clone(),
                                    evidence_id: evidence_id.clone(),
                                    output: receipt.data().output.clone(),
                                },
                            ))
                        } else if is_new
                            && accounts.receipt_uncertainty.get(&data.seq.get())
                                == Some(&receipt.data().intent_id)
                        {
                            state.uncertain.insert(receipt.data().intent_id.clone());
                            if let Some(item) = state.intents.get_mut(&receipt.data().intent_id) {
                                item.status = DispatchStatus::Uncertain;
                            }
                            Some((
                                InboxSource::Uncertainty,
                                Some(evidence_id.clone()),
                                AgentEvent::EffectUncertain {
                                    intent_id: receipt.data().intent_id.clone(),
                                },
                            ))
                        } else {
                            None
                        }
                    }
                    (
                        StoreOperation::SettleReconciliation {
                            task_id: id,
                            reconciled_receipt: Some(receipt),
                            ..
                        },
                        StoreResult::ReceiptRecorded {
                            evidence_id,
                            settled: false,
                            ..
                        },
                    ) if id == task_id => {
                        if receipt.evidence_id().ok().as_ref() != Some(evidence_id) {
                            return Ok(None);
                        }
                        if state.seen_receipts.insert(evidence_id.clone()) {
                            for item in state.inputs.values_mut().filter(|item| {
                                item.status == InboxStatus::Pending
                                    && item.input.source == InboxSource::Verification
                            }) {
                                item.status = InboxStatus::Superseded;
                            }
                        }
                        None
                    }
                    (
                        StoreOperation::SettleReconciliation {
                            task_id: id,
                            intent_id,
                            evidence_id,
                            reconciled_receipt,
                            ..
                        },
                        StoreResult::ReconciliationSettled {
                            evidence_id: selected,
                        },
                    ) if id == task_id => {
                        if evidence_id != selected {
                            return Ok(None);
                        }
                        if let Some(receipt) = reconciled_receipt {
                            if receipt.evidence_id().ok().as_ref() != Some(evidence_id) {
                                return Ok(None);
                            }
                            state.seen_receipts.insert(evidence_id.clone());
                        }
                        let Some(evidence) = state.evidence.get(evidence_id) else {
                            return Ok(None);
                        };
                        let receipt = &evidence.receipt;
                        Some((
                            InboxSource::Settlement,
                            Some(evidence_id.clone()),
                            AgentEvent::EffectSettled {
                                intent_id: intent_id.clone(),
                                evidence_id: evidence_id.clone(),
                                output: receipt.data().output.clone(),
                            },
                        ))
                    }
                    (
                        StoreOperation::RecordVerification {
                            task_id: id,
                            accepted,
                            reason,
                            ..
                        },
                        StoreResult::VerificationRecorded,
                    ) if id == task_id => Some((
                        InboxSource::Verification,
                        None,
                        AgentEvent::Verified {
                            accepted: *accepted,
                            reason: reason.clone(),
                        },
                    )),
                    _ => None,
                };
                if let Some((source, evidence, event)) = generated {
                    let Ok(input) = input_for(&state.task, data.seq, source, evidence, event)
                    else {
                        return Ok(None);
                    };
                    if state
                        .inputs
                        .insert(
                            input.id.clone(),
                            InboxSnapshot {
                                input,
                                status: InboxStatus::Pending,
                                transition: None,
                            },
                        )
                        .is_some()
                    {
                        return Ok(None);
                    }
                }
                if let (
                    StoreOperation::CommitTransition {
                        task_id: id,
                        expected_revision,
                        inbox_id: id_input,
                        inbox_digest: digest,
                        transition,
                    },
                    StoreResult::TransitionCommitted {
                        revision,
                        intent_id,
                    },
                ) = (&command.data().operation, result)
                    && id == task_id
                {
                    if *expected_revision != state.state.revision
                        || transition.validate_from(&state.state).is_err()
                    {
                        return Ok(None);
                    }
                    let Some(oldest) = state
                        .inputs
                        .values()
                        .filter(|item| item.status == InboxStatus::Pending)
                        .min_by_key(|item| (item.input.source_seq, item.input.output_ordinal))
                    else {
                        return Ok(None);
                    };
                    if &oldest.input.id != id_input || &oldest.input.digest != digest {
                        return Ok(None);
                    }
                    let (next_state, intent) = match transition {
                        Transition::Decision { next, intent } => (next, Some(intent)),
                        Transition::Finalization { next } => (next, None),
                    };
                    if *revision != next_state.revision
                        || next_state.pending_intent.as_ref() != intent_id.as_ref()
                            && intent.is_some()
                    {
                        return Ok(None);
                    }
                    if let Some(intent) = intent {
                        let Some(id) = intent_id else { return Ok(None) };
                        let genesis = state.state.genesis_digest.bytes();
                        let derived = structural_digest(
                            "intent-id/v1",
                            &[
                                state.task.data().task_id.as_str().as_bytes(),
                                &genesis,
                                &intent.ordinal.to_be_bytes(),
                            ],
                        );
                        if derived.ok().as_ref() != Some(id) {
                            return Ok(None);
                        }
                        let zero = SqlU64::new(0).map_err(StoreError::Validation)?;
                        let value = IntentSnapshot {
                            intent_id: id.clone(),
                            intent: intent.clone(),
                            prepared: None,
                            status: DispatchStatus::Ready,
                            dispatch_id: None,
                            lease_epoch: zero,
                            lease: None,
                            reconciliation_epoch: zero,
                            reconciliation_lease: None,
                            reservation: None,
                            approval: None,
                            evidence_revision: zero,
                            selected_evidence: None,
                        };
                        if state.intents.insert(id.clone(), value).is_some() {
                            return Ok(None);
                        }
                    } else if intent_id.is_some() {
                        return Ok(None);
                    }
                    let Some(item) = state.inputs.get_mut(id_input) else {
                        return Ok(None);
                    };
                    item.status = InboxStatus::Consumed;
                    item.transition = Some(transition.clone());
                    state.state = next_state.clone();
                }
            }
            previous = Some(data.digest.clone());
            expected = data
                .seq
                .get()
                .checked_add(1)
                .filter(|n| *n <= MAX_SQL_INTEGER);
        }
        match page.next {
            Some(cursor)
                if page
                    .records
                    .last()
                    .is_some_and(|item| item.data().seq == cursor)
                    && Some(cursor) != after =>
            {
                after = Some(cursor)
            }
            Some(_) => return Ok(None),
            None => break,
        }
    }
    Ok(target)
}

fn compatible(operation: &StoreOperation, result: &StoreResult) -> bool {
    matches!(
        (operation, result),
        (
            StoreOperation::CreateTask { .. },
            StoreResult::TaskCreated { .. }
        ) | (
            StoreOperation::CommitTransition { .. },
            StoreResult::TransitionCommitted { .. }
        ) | (
            StoreOperation::PrepareIntent { .. },
            StoreResult::Prepared { .. }
        ) | (StoreOperation::Claim { .. }, StoreResult::Claimed { .. })
            | (
                StoreOperation::AdvanceTick { .. },
                StoreResult::TickAdvanced { .. }
            )
            | (
                StoreOperation::Admit { .. },
                StoreResult::Admitted { .. } | StoreResult::Denied
            )
            | (StoreOperation::Deny { .. }, StoreResult::Denied)
            | (
                StoreOperation::CreateApprovalRequest { .. },
                StoreResult::ApprovalRequestCreated
            )
            | (
                StoreOperation::RecordApproval { .. },
                StoreResult::ApprovalRecorded
            )
            | (
                StoreOperation::RevokeGrant { .. },
                StoreResult::GrantRevoked { .. }
            )
            | (
                StoreOperation::RecordUncertainty { .. },
                StoreResult::UncertaintyRecorded
            )
            | (
                StoreOperation::RecordReceipt { .. },
                StoreResult::ReceiptRecorded { .. }
            )
            | (
                StoreOperation::ClaimReconciliation { .. },
                StoreResult::ReconciliationClaimed { .. }
            )
            | (
                StoreOperation::SettleReconciliation { .. },
                StoreResult::ReconciliationSettled { .. } | StoreResult::ReceiptRecorded { .. }
            )
            | (
                StoreOperation::RecordVerification { .. },
                StoreResult::VerificationRecorded
            )
    )
}

fn counters(model: u64, tools: u64, money: u64) -> Option<Counters> {
    Some(Counters {
        model_requests: SqlU64::new(model).ok()?,
        tool_calls: SqlU64::new(tools).ok()?,
        spend_micros_usd: SqlU64::new(money).ok()?,
    })
}
fn add(a: &Counters, b: &Counters) -> Option<Counters> {
    counters(
        a.model_requests.get().checked_add(b.model_requests.get())?,
        a.tool_calls.get().checked_add(b.tool_calls.get())?,
        a.spend_micros_usd
            .get()
            .checked_add(b.spend_micros_usd.get())?,
    )
}
fn subtract(a: &Counters, b: &Counters) -> Option<Counters> {
    counters(
        a.model_requests.get().checked_sub(b.model_requests.get())?,
        a.tool_calls.get().checked_sub(b.tool_calls.get())?,
        a.spend_micros_usd
            .get()
            .checked_sub(b.spend_micros_usd.get())?,
    )
}
fn exceeds(a: &Counters, b: &Counters) -> bool {
    a.model_requests > b.model_requests
        || a.tool_calls > b.tool_calls
        || a.spend_micros_usd > b.spend_micros_usd
}
fn budget(task: &Task) -> Counters {
    let b = &task.data().budget;
    Counters {
        model_requests: b.max_model_requests,
        tool_calls: b.max_tool_calls,
        spend_micros_usd: b.max_spend_micros_usd,
    }
}
fn empty_account(limit: Counters) -> AccountSnapshot {
    let zero = counters(0, 0, 0).expect("zero counters are representable");
    AccountSnapshot {
        limit,
        posted: zero.clone(),
        reserved: zero,
        breached: false,
        incomplete: false,
        integrity_hold: false,
    }
}
fn tariff(request: &ApprovedRequest) -> Counters {
    let value = match request {
        ApprovedRequest::ReadFixture { tariff, .. }
        | ApprovedRequest::RequestModel { tariff, .. }
        | ApprovedRequest::RequestLocalModel { tariff, .. } => tariff,
        ApprovedRequest::PublishArtifact { plan } => &plan.tariff,
    };
    Counters {
        model_requests: value.model_requests,
        tool_calls: value.tool_calls,
        spend_micros_usd: value.spend_micros_usd,
    }
}

fn receipt_shape_matches(request: &ApprovedRequest, output: &ReceiptOutput) -> bool {
    matches!(
        (request, output),
        (
            ApprovedRequest::ReadFixture { .. },
            ReceiptOutput::Fixture { .. } | ReceiptOutput::Failure { .. }
        ) | (
            ApprovedRequest::RequestModel { .. },
            ReceiptOutput::Model { .. } | ReceiptOutput::Failure { .. }
        ) | (
            ApprovedRequest::RequestLocalModel { .. },
            ReceiptOutput::LocalModel { .. } | ReceiptOutput::Failure { .. }
        ) | (
            ApprovedRequest::PublishArtifact { .. },
            ReceiptOutput::Published { .. } | ReceiptOutput::Failure { .. }
        )
    )
}
struct Reservation {
    task: TaskId,
    principal: PrincipalId,
    amount: Counters,
    lease_epoch: SqlU64,
    expires_at: SqlU64,
    id: Digest,
}
#[derive(Default)]
struct Accounts {
    tasks: BTreeMap<TaskId, AccountSnapshot>,
    principals: BTreeMap<PrincipalId, AccountSnapshot>,
    owners: BTreeMap<TaskId, PrincipalId>,
    reservations: BTreeMap<Digest, Reservation>,
    retired_reservations: BTreeMap<Digest, Reservation>,
    reservation_ids: BTreeMap<Digest, Digest>,
    receipts: BTreeMap<Digest, Receipt>,
    unposted: BTreeMap<TaskId, Vec<UnpostedDebit>>,
    leases: BTreeMap<Digest, LeaseSnapshot>,
    tick: u64,
    uncertain: BTreeSet<Digest>,
    conflicted: BTreeSet<Digest>,
    pending_evidence: BTreeSet<Digest>,
    receipt_uncertainty: BTreeMap<u64, Digest>,
    denied_admission: BTreeMap<u64, BoundedText<MAX_TEXT>>,
    revoked: BTreeSet<PrincipalId>,
    quarantined: BTreeMap<Digest, TaskId>,
    permanent_holds: BTreeSet<TaskId>,
    verification_inputs: BTreeMap<Digest, TaskId>,
    verification_consumed: BTreeSet<TaskId>,
}
impl Accounts {
    fn create(&mut self, task: &Task) -> bool {
        let data = task.data();
        if self.tasks.contains_key(&data.task_id) {
            return false;
        }
        self.tasks
            .insert(data.task_id.clone(), empty_account(budget(task)));
        self.principals
            .entry(data.principal_id.clone())
            .or_insert_with(|| empty_account(budget(task)));
        self.owners
            .insert(data.task_id.clone(), data.principal_id.clone());
        true
    }
    fn reserve(&mut self, check: &AdmissionCheck) -> bool {
        let task = &check.prepared.action.task_id;
        let principal = &check.prepared.action.principal_id;
        if self.owners.get(task) != Some(principal) {
            return false;
        }
        if let Some(existing) = self
            .reservations
            .get(&check.intent_id)
            .or_else(|| self.retired_reservations.get(&check.intent_id))
        {
            return existing.task == *task
                && existing.id == check.reservation_id
                && existing.principal == *principal
                && existing.lease_epoch == check.lease_epoch
                && existing.amount == tariff(&check.prepared.request)
                && self.leases.get(&check.intent_id).is_some_and(|lease| {
                    lease.worker == check.worker && lease.epoch == check.lease_epoch
                });
        }
        if self.reservation_ids.contains_key(&check.reservation_id) {
            return false;
        }
        let Some(a) = self.tasks.get_mut(task) else {
            return false;
        };
        let Some(b) = self.principals.get_mut(principal) else {
            return false;
        };
        let Some(lease) = self.leases.get(&check.intent_id) else {
            return false;
        };
        if lease.epoch != check.lease_epoch
            || lease.worker != check.worker
            || lease.expires_at.get() <= self.tick
            || check.expected_tick.get() != self.tick
        {
            return false;
        }
        let amount = tariff(&check.prepared.request);
        let (Some(ar), Some(br)) = (add(&a.reserved, &amount), add(&b.reserved, &amount)) else {
            return false;
        };
        let (Some(at), Some(bt)) = (add(&a.posted, &ar), add(&b.posted, &br)) else {
            return false;
        };
        if exceeds(&at, &a.limit)
            || exceeds(&bt, &b.limit)
            || a.breached
            || a.incomplete
            || a.integrity_hold
            || b.breached
            || b.incomplete
            || b.integrity_hold
        {
            return false;
        }
        a.reserved = ar;
        b.reserved = br;
        self.reservation_ids
            .insert(check.reservation_id.clone(), check.intent_id.clone());
        self.reservations.insert(
            check.intent_id.clone(),
            Reservation {
                task: task.clone(),
                principal: principal.clone(),
                amount,
                lease_epoch: lease.epoch,
                expires_at: lease.expires_at,
                id: check.reservation_id.clone(),
            },
        );
        true
    }
    fn clear_reconciled_hold(&mut self, intent: &Digest, task: &TaskId) {
        self.quarantined.remove(intent);
        self.pending_evidence.remove(intent);
        let has_quarantine = self.quarantined.values().any(|id| id == task);
        let has_unresolved = self
            .reservations
            .values()
            .any(|reservation| &reservation.task == task);
        if let Some(account) = self.tasks.get_mut(task)
            && !has_quarantine
            && !has_unresolved
            && !self.permanent_holds.contains(task)
            && !account.incomplete
        {
            account.integrity_hold = false;
        }
        if let Some(principal) = self.owners.get(task) {
            let any_hold = self.owners.iter().any(|(id, owner)| {
                owner == principal
                    && self
                        .tasks
                        .get(id)
                        .is_some_and(|account| account.integrity_hold)
            });
            if let Some(account) = self.principals.get_mut(principal) {
                account.integrity_hold = any_hold;
            }
        }
    }
    fn receipt(
        &mut self,
        receipt: &Receipt,
        evidence: &Digest,
        settled: bool,
        conflict: bool,
        lease_epoch: Option<SqlU64>,
    ) -> bool {
        if receipt.evidence_id().ok().as_ref() != Some(evidence) {
            return false;
        }
        if lease_epoch.is_some()
            && let Some(existing) = self.receipts.get(evidence)
        {
            return existing == receipt;
        }
        if lease_epoch.is_none() {
            let unposted = self
                .unposted
                .values()
                .flatten()
                .any(|debit| debit.dispatch_id == receipt.data().dispatch_id);
            let contradictory = self
                .receipts
                .values()
                .filter(|prior| prior.data().dispatch_id == receipt.data().dispatch_id)
                .any(|prior| match (&prior.data().usage, &receipt.data().usage) {
                    (ActualUsage::Known { usage: left }, ActualUsage::Known { usage: right }) => {
                        left != right
                    }
                    _ => true,
                });
            if unposted || contradictory {
                return false;
            }
        }
        if lease_epoch.is_some() {
            let observed_conflict = self.conflicted.contains(&receipt.data().intent_id)
                || self.receipts.values().any(|prior| {
                    prior.data().producer_receipt_id == receipt.data().producer_receipt_id
                        && prior != receipt
                });
            if observed_conflict != conflict {
                return false;
            }
        }
        if conflict {
            self.conflicted.insert(receipt.data().intent_id.clone());
            for prior in self.receipts.values().filter(|prior| {
                prior.data().producer_receipt_id == receipt.data().producer_receipt_id
            }) {
                self.conflicted.insert(prior.data().intent_id.clone());
            }
        }
        let affected: BTreeSet<TaskId> = if conflict {
            self.receipts
                .values()
                .filter(|prior| {
                    prior.data().producer_receipt_id == receipt.data().producer_receipt_id
                        && prior != &receipt
                })
                .map(|prior| prior.data().task_id.clone())
                .collect()
        } else {
            BTreeSet::new()
        };
        for id in affected {
            self.permanent_holds.insert(id.clone());
            if let Some(account) = self.tasks.get_mut(&id) {
                account.integrity_hold = true;
            }
            if let Some(principal) = self.owners.get(&id)
                && let Some(account) = self.principals.get_mut(principal)
            {
                account.integrity_hold = true;
            }
        }
        self.receipts.insert(evidence.clone(), receipt.clone());
        let Some(principal) = self.owners.get(&receipt.data().task_id).cloned() else {
            return false;
        };
        let Some(task) = self.tasks.get_mut(&receipt.data().task_id) else {
            return false;
        };
        let Some(account) = self.principals.get_mut(&principal) else {
            return false;
        };
        if conflict {
            self.permanent_holds.insert(receipt.data().task_id.clone());
            task.integrity_hold = true;
            account.integrity_hold = true;
        }
        if matches!(receipt.data().usage, ActualUsage::Unknown { .. }) {
            task.incomplete = true;
            account.incomplete = true;
        }
        let Some(reservation) = self.reservations.get(&receipt.data().intent_id) else {
            self.pending_evidence
                .insert(receipt.data().intent_id.clone());
            self.quarantined.insert(
                receipt.data().intent_id.clone(),
                receipt.data().task_id.clone(),
            );
            if self.verification_consumed.contains(&receipt.data().task_id) {
                self.permanent_holds.insert(receipt.data().task_id.clone());
            }
            task.integrity_hold = true;
            account.integrity_hold = true;
            return !settled;
        };
        let fresh = lease_epoch.is_none_or(|epoch| {
            !self.uncertain.contains(&receipt.data().intent_id)
                && !self.pending_evidence.contains(&receipt.data().intent_id)
                && !self.conflicted.contains(&receipt.data().intent_id)
                && epoch == reservation.lease_epoch
                && self.tick < reservation.expires_at.get()
        });
        if !fresh || conflict {
            self.pending_evidence
                .insert(receipt.data().intent_id.clone());
            self.quarantined.insert(
                receipt.data().intent_id.clone(),
                receipt.data().task_id.clone(),
            );
            if self.verification_consumed.contains(&receipt.data().task_id) {
                self.permanent_holds.insert(receipt.data().task_id.clone());
            }
            task.integrity_hold = true;
            account.integrity_hold = true;
            return !settled;
        }
        if reservation.task != receipt.data().task_id || reservation.principal != principal {
            return false;
        }
        let ActualUsage::Known { usage } = &receipt.data().usage else {
            self.pending_evidence
                .insert(receipt.data().intent_id.clone());
            self.uncertain.insert(receipt.data().intent_id.clone());
            return !settled;
        };
        let actual = usage
            .model_requests
            .as_u64()
            .zip(usage.tool_calls.as_u64())
            .zip(usage.spend_micros_usd.as_u64())
            .and_then(|((m, t), s)| counters(m, t, s));
        let posted = actual
            .as_ref()
            .and_then(|amount| Some((add(&task.posted, amount)?, add(&account.posted, amount)?)));
        if let Some((task_posted, principal_posted)) = posted {
            if !settled {
                return false;
            }
            let (Some(task_reserved), Some(principal_reserved)) = (
                subtract(&task.reserved, &reservation.amount),
                subtract(&account.reserved, &reservation.amount),
            ) else {
                return false;
            };
            let exceeded_tariff = actual
                .as_ref()
                .is_some_and(|amount| exceeds(amount, &reservation.amount));
            task.breached |= exceeded_tariff || exceeds(&task_posted, &task.limit);
            account.breached |= exceeded_tariff || exceeds(&principal_posted, &account.limit);
            task.posted = task_posted;
            account.posted = principal_posted;
            task.reserved = task_reserved;
            account.reserved = principal_reserved;
            if let Some(reservation) = self.reservations.remove(&receipt.data().intent_id) {
                self.retired_reservations
                    .insert(receipt.data().intent_id.clone(), reservation);
            }
        } else {
            if settled {
                return false;
            }
            task.incomplete = true;
            account.incomplete = true;
            self.pending_evidence
                .insert(receipt.data().intent_id.clone());
            self.uncertain.insert(receipt.data().intent_id.clone());
            self.unposted
                .entry(receipt.data().task_id.clone())
                .or_default()
                .push(UnpostedDebit {
                    dispatch_id: receipt.data().dispatch_id.clone(),
                    usage: usage.clone(),
                });
        }
        true
    }
}

fn reconstruct_accounts(
    snapshot: &mut impl RuntimeSnapshot,
) -> Result<Option<Accounts>, StoreError> {
    let mut state = Accounts::default();
    let mut after = None;
    let mut operations = BTreeMap::<Digest, StoreResult>::new();
    loop {
        let page = snapshot.journal_page(after)?;
        for record in &page.records {
            let JournalEvent::CommandCommitted { command, result } = &record.data().event;
            let Ok(bytes) = canonical_bytes(&command.data().operation, MAX_INTERNAL) else {
                return Ok(None);
            };
            let key = content_digest(&bytes);
            match operations.get(&key) {
                Some(prior) if prior == result => continue,
                Some(_)
                    if matches!(
                        &command.data().operation,
                        StoreOperation::RecordReceipt { .. }
                    ) =>
                {
                    operations.insert(key, result.clone());
                }
                Some(_) => return Ok(None),
                None => {
                    operations.insert(key, result.clone());
                }
            }
            let valid = match (&command.data().operation, result) {
                (
                    StoreOperation::RecordVerification { task_id, .. },
                    StoreResult::VerificationRecorded,
                ) => {
                    let Some(principal) = state.owners.get(task_id) else {
                        return Ok(None);
                    };
                    let Some(task_account) = state.tasks.get(task_id) else {
                        return Ok(None);
                    };
                    let Some(principal_account) = state.principals.get(principal) else {
                        return Ok(None);
                    };
                    if task_account.incomplete
                        || task_account.integrity_hold
                        || task_account.breached
                        || principal_account.incomplete
                        || principal_account.integrity_hold
                        || principal_account.breached
                        || state
                            .reservations
                            .values()
                            .any(|reservation| &reservation.task == task_id)
                    {
                        return Ok(None);
                    }
                    let Ok(id) = inbox_id(task_id, record.data().seq, 0) else {
                        return Ok(None);
                    };
                    state.verification_inputs.insert(id, task_id.clone());
                    true
                }
                (
                    StoreOperation::CommitTransition { inbox_id, .. },
                    StoreResult::TransitionCommitted { .. },
                ) => {
                    if let Some(task) = state.verification_inputs.get(inbox_id) {
                        state.verification_consumed.insert(task.clone());
                    }
                    true
                }
                (
                    StoreOperation::RecordUncertainty { intent_id, .. },
                    StoreResult::UncertaintyRecorded,
                ) => {
                    state.uncertain.insert(intent_id.clone());
                    true
                }
                (StoreOperation::CreateTask { task, .. }, StoreResult::TaskCreated { .. }) => {
                    state.create(task)
                }
                (
                    StoreOperation::AdvanceTick { new_tick, .. },
                    StoreResult::TickAdvanced { .. },
                ) => {
                    state.tick = new_tick.get();
                    true
                }
                (
                    StoreOperation::Claim {
                        intent_id, worker, ..
                    },
                    StoreResult::Claimed {
                        lease_epoch,
                        expires_at,
                    },
                ) => {
                    state.leases.insert(
                        intent_id.clone(),
                        LeaseSnapshot {
                            worker: worker.clone(),
                            epoch: *lease_epoch,
                            expires_at: *expires_at,
                        },
                    );
                    true
                }
                (
                    StoreOperation::RevokeGrant { principal_id, .. },
                    StoreResult::GrantRevoked { .. },
                ) => {
                    state.revoked.insert(principal_id.clone());
                    true
                }
                (StoreOperation::Admit { check }, StoreResult::Denied) => {
                    let Some(task) = state.tasks.get(&check.prepared.action.task_id) else {
                        return Ok(None);
                    };
                    let Some(principal) = state.principals.get(&check.prepared.action.principal_id)
                    else {
                        return Ok(None);
                    };
                    let reason = if state.revoked.contains(&check.prepared.action.principal_id) {
                        "grant revoked"
                    } else if task.breached
                        || task.incomplete
                        || task.integrity_hold
                        || principal.breached
                        || principal.incomplete
                        || principal.integrity_hold
                    {
                        "account unavailable"
                    } else {
                        let amount = tariff(&check.prepared.request);
                        let task_total = add(&task.reserved, &amount)
                            .and_then(|reserved| add(&task.posted, &reserved));
                        let principal_total = add(&principal.reserved, &amount)
                            .and_then(|reserved| add(&principal.posted, &reserved));
                        if task_total
                            .as_ref()
                            .is_some_and(|total| !exceeds(total, &task.limit))
                            && principal_total
                                .as_ref()
                                .is_some_and(|total| !exceeds(total, &principal.limit))
                        {
                            return Ok(None);
                        }
                        "budget exceeded"
                    };
                    let Ok(reason) = BoundedText::new(reason) else {
                        return Ok(None);
                    };
                    state
                        .denied_admission
                        .insert(record.data().seq.get(), reason);
                    true
                }
                (StoreOperation::Admit { check }, StoreResult::Admitted { dispatch_id }) => {
                    *dispatch_id == check.prepared.action.dispatch_id && state.reserve(check)
                }
                (
                    StoreOperation::RecordReceipt {
                        receipt,
                        lease_epoch,
                    },
                    StoreResult::ReceiptRecorded {
                        evidence_id,
                        settled,
                        conflict,
                        ..
                    },
                ) => {
                    let was_uncertain = state.uncertain.contains(&receipt.data().intent_id);
                    let ok = state.receipt(
                        receipt,
                        evidence_id,
                        *settled,
                        *conflict,
                        Some(*lease_epoch),
                    );
                    if !was_uncertain && state.uncertain.contains(&receipt.data().intent_id) {
                        state
                            .receipt_uncertainty
                            .insert(record.data().seq.get(), receipt.data().intent_id.clone());
                    }
                    ok
                }
                (
                    StoreOperation::SettleReconciliation {
                        reconciled_receipt: Some(receipt),
                        ..
                    },
                    StoreResult::ReceiptRecorded {
                        evidence_id,
                        settled,
                        conflict,
                        ..
                    },
                ) => {
                    if *settled {
                        false
                    } else {
                        state.receipt(
                            receipt,
                            evidence_id,
                            false,
                            *conflict,
                            Some(SqlU64::new(0).map_err(StoreError::Validation)?),
                        )
                    }
                }
                (
                    StoreOperation::SettleReconciliation {
                        evidence_id,
                        reconciled_receipt,
                        ..
                    },
                    StoreResult::ReconciliationSettled {
                        evidence_id: selected,
                    },
                ) => {
                    if selected != evidence_id {
                        false
                    } else {
                        let receipt = reconciled_receipt
                            .clone()
                            .or_else(|| state.receipts.get(evidence_id).cloned());
                        receipt.is_some_and(|value| {
                            let ok = state.receipt(&value, evidence_id, true, false, None);
                            if ok {
                                state.clear_reconciled_hold(
                                    &value.data().intent_id,
                                    &value.data().task_id,
                                );
                            }
                            ok
                        })
                    }
                }
                _ => true,
            };
            if !valid {
                return Ok(None);
            }
        }
        match page.next {
            Some(cursor) if Some(cursor) != after => after = Some(cursor),
            Some(_) => return Ok(None),
            None => break,
        }
    }
    Ok(Some(state))
}

fn unposted_matches(
    snapshot: &mut impl RuntimeSnapshot,
    task: &TaskId,
    expected: &[UnpostedDebit],
) -> Result<bool, StoreError> {
    let mut actual = Vec::new();
    let mut cursor = None;
    loop {
        let page = snapshot.unposted_debits(task, cursor)?;
        if page.records.len() > MAX_PAGE_RECORDS {
            return Ok(false);
        }
        let mut size = 0usize;
        for item in page.records {
            let Ok(bytes) = canonical_bytes(&item, MAX_INTERNAL) else {
                return Ok(false);
            };
            let Some(next) = size.checked_add(bytes.len()) else {
                return Ok(false);
            };
            size = next;
            if size > MAX_PAGE_BYTES {
                return Ok(false);
            }
            actual.push(item);
        }
        match page.next {
            Some(next) if cursor.is_none_or(|old| next > old) => cursor = Some(next),
            Some(_) => return Ok(false),
            None => break,
        }
    }
    let mut expected = expected.to_vec();
    actual.sort_by(|a, b| a.dispatch_id.cmp(&b.dispatch_id));
    expected.sort_by(|a, b| a.dispatch_id.cmp(&b.dispatch_id));
    Ok(actual == expected)
}

fn apply_lifecycle(state: &mut Reconstruction, op: &StoreOperation, result: &StoreResult) -> bool {
    let task_id = &state.task.data().task_id;
    match (op, result) {
        (
            StoreOperation::PrepareIntent {
                task_id: id,
                prepared,
            },
            StoreResult::Prepared { intent_id },
        ) if id == task_id => {
            let Some(item) = state.intents.get_mut(intent_id) else {
                return false;
            };
            if &prepared.intent_id != intent_id
                || prepared
                    .validate_against_genesis(
                        &item.intent,
                        &state.task.data().principal_id,
                        &state.state.genesis_digest,
                        &state.genesis,
                    )
                    .is_err()
            {
                return false;
            }
            if let ApprovedRequest::RequestModel {
                input_digest,
                context,
                ..
            }
            | ApprovedRequest::RequestLocalModel {
                input_digest,
                context,
                ..
            } = &prepared.request
            {
                let Some(read) = &state.verification.read else {
                    return false;
                };
                let ReceiptOutput::Fixture { bytes, digest } = &read.data().output else {
                    return false;
                };
                if input_digest != digest
                    || context.as_str() != bytes.as_str()
                    || content_digest(bytes.as_str().as_bytes()) != *digest
                {
                    return false;
                }
            }
            if item
                .prepared
                .as_ref()
                .is_some_and(|prior| prior != prepared)
            {
                return false;
            }
            item.prepared = Some(prepared.clone());
        }
        (
            StoreOperation::Claim {
                task_id: id,
                intent_id,
                worker,
                expected_epoch,
            },
            StoreResult::Claimed {
                lease_epoch,
                expires_at,
            },
        ) if id == task_id => {
            let Some(item) = state.intents.get_mut(intent_id) else {
                return false;
            };
            let eligible = item.status == DispatchStatus::Ready
                || (item.status == DispatchStatus::Leased
                    && item
                        .lease
                        .as_ref()
                        .is_some_and(|lease| lease.expires_at <= state.tick));
            if !eligible || item.dispatch_id.is_some() {
                return false;
            }
            if item.lease_epoch != *expected_epoch
                || expected_epoch.checked_add(1).ok() != Some(*lease_epoch)
                || state.tick.checked_add(30).ok() != Some(*expires_at)
            {
                return false;
            }
            item.lease_epoch = *lease_epoch;
            item.lease = Some(LeaseSnapshot {
                worker: worker.clone(),
                epoch: *lease_epoch,
                expires_at: *expires_at,
            });
            item.status = DispatchStatus::Leased;
        }
        (
            StoreOperation::CreateApprovalRequest {
                task_id: id,
                request_id,
                intent_id,
                action_digest,
            },
            StoreResult::ApprovalRequestCreated,
        ) if id == task_id => {
            let Some(item) = state.intents.get_mut(intent_id) else {
                return false;
            };
            if item.status != DispatchStatus::Ready
                || item.approval.is_some()
                || item
                    .prepared
                    .as_ref()
                    .is_none_or(|prepared| &prepared.action_digest != action_digest)
            {
                return false;
            }
            let Some(expires_at) = state.tick.checked_add(APPROVAL_TTL_TICKS).ok() else {
                return false;
            };
            item.approval = Some(ApprovalSnapshot {
                task_id: id.clone(),
                intent_id: intent_id.clone(),
                request_id: request_id.clone(),
                action_digest: action_digest.clone(),
                expires_at,
                decision: None,
            });
            item.status = DispatchStatus::AwaitingApproval;
        }
        (
            StoreOperation::RecordApproval {
                request_id,
                action_digest,
                decision,
            },
            StoreResult::ApprovalRecorded,
        ) => {
            if let Some(item) = state.intents.values_mut().find(|item| {
                item.approval
                    .as_ref()
                    .is_some_and(|approval| &approval.request_id == request_id)
            }) {
                let Some(approval) = &mut item.approval else {
                    return false;
                };
                if approval.decision == Some(*decision) && &approval.action_digest == action_digest
                {
                    return true;
                }
                if &approval.action_digest != action_digest || approval.expires_at <= state.tick {
                    return false;
                }
                if item.status != DispatchStatus::AwaitingApproval || approval.decision.is_some() {
                    return false;
                }
                approval.decision = Some(*decision);
                if *decision == ApprovalDecision::Approve {
                    item.status = DispatchStatus::Ready;
                }
            }
        }
        (StoreOperation::Admit { check }, StoreResult::Denied)
            if &check.prepared.action.task_id == task_id =>
        {
            let Some(item) = state.intents.get_mut(&check.intent_id) else {
                return false;
            };
            item.status = DispatchStatus::Denied;
        }
        (StoreOperation::Admit { check }, StoreResult::Admitted { dispatch_id })
            if &check.prepared.action.task_id == task_id =>
        {
            let Some(item) = state.intents.get_mut(&check.intent_id) else {
                return false;
            };
            if item.prepared.as_ref() != Some(&check.prepared)
                || &check.prepared.action.dispatch_id != dispatch_id
            {
                return false;
            }
            if item.dispatch_id.as_ref() == Some(dispatch_id) {
                return item.prepared.as_ref() == Some(&check.prepared)
                    && item.lease.as_ref().is_some_and(|lease| {
                        lease.worker == check.worker && lease.epoch == check.lease_epoch
                    })
                    && item
                        .reservation
                        .as_ref()
                        .is_some_and(|reservation| reservation.id == check.reservation_id);
            }
            let Some(lease) = &item.lease else {
                return false;
            };
            if lease.worker != check.worker
                || lease.epoch != check.lease_epoch
                || lease.expires_at <= state.tick
                || check.expected_tick != state.tick
                || state.grant.revoked
                || state.grant.revision != check.grant_revision
            {
                return false;
            }
            if state.genesis.require_publication_approval
                && matches!(
                    check.prepared.request,
                    ApprovedRequest::PublishArtifact { .. }
                )
            {
                let Some(approval) = &item.approval else {
                    return false;
                };
                if check.approval_id.as_ref() != Some(&approval.request_id)
                    || approval.action_digest != check.prepared.action_digest
                    || approval.decision != Some(ApprovalDecision::Approve)
                    || approval.expires_at <= state.tick
                {
                    return false;
                }
            }
            item.status = DispatchStatus::Dispatched;
            item.dispatch_id = Some(dispatch_id.clone());
            item.reservation = Some(ReservationSnapshot {
                id: check.reservation_id.clone(),
                amount: tariff(&check.prepared.request),
                status: ReservationStatus::Held,
            });
        }
        (
            StoreOperation::Deny {
                task_id: id,
                intent_id,
                ..
            },
            StoreResult::Denied,
        ) if id == task_id => {
            let Some(item) = state.intents.get_mut(intent_id) else {
                return false;
            };
            item.status = DispatchStatus::Denied;
        }
        (
            StoreOperation::RecordUncertainty {
                task_id: id,
                intent_id,
                ..
            },
            StoreResult::UncertaintyRecorded,
        ) if id == task_id => {
            if state.uncertain.contains(intent_id) {
                return true;
            }
            let Some(item) = state.intents.get_mut(intent_id) else {
                return false;
            };
            item.status = DispatchStatus::Uncertain;
        }
        (
            StoreOperation::ClaimReconciliation {
                task_id: id,
                intent_id,
                worker,
                expected_epoch,
                expected_evidence_revision,
                ..
            },
            StoreResult::ReconciliationClaimed {
                lease_epoch,
                expires_at,
            },
        ) if id == task_id => {
            let Some(item) = state.intents.get_mut(intent_id) else {
                return false;
            };
            if item.evidence_revision != *expected_evidence_revision
                || item
                    .reconciliation_lease
                    .as_ref()
                    .is_some_and(|lease| lease.expires_at > state.tick)
            {
                return false;
            }
            if item.reconciliation_epoch != *expected_epoch
                || expected_epoch.checked_add(1).ok() != Some(*lease_epoch)
                || state.tick.checked_add(30).ok() != Some(*expires_at)
            {
                return false;
            }
            item.reconciliation_epoch = *lease_epoch;
            item.reconciliation_lease = Some(LeaseSnapshot {
                worker: worker.clone(),
                epoch: *lease_epoch,
                expires_at: *expires_at,
            });
        }
        (
            StoreOperation::RecordReceipt { receipt, .. },
            StoreResult::ReceiptRecorded {
                evidence_id,
                evidence_revision,
                settled,
                conflict,
            },
        ) if &receipt.data().task_id == task_id => {
            if let Some(existing) = state.evidence.get(evidence_id) {
                let Some(intent) = state.intents.get(&receipt.data().intent_id) else {
                    return false;
                };
                return existing.receipt == *receipt
                    && existing.selected == *settled
                    && existing.conflict == *conflict
                    && intent.evidence_revision == *evidence_revision;
            }
            if *conflict {
                let affected: BTreeSet<Digest> = state
                    .evidence
                    .values_mut()
                    .filter(|old| {
                        old.receipt.data().producer_receipt_id == receipt.data().producer_receipt_id
                    })
                    .map(|old| {
                        old.conflict = true;
                        old.receipt.data().intent_id.clone()
                    })
                    .filter(|id| id != &receipt.data().intent_id)
                    .collect();
                for id in affected {
                    let Some(prior) = state.intents.get_mut(&id) else {
                        return false;
                    };
                    let Ok(next) = prior.evidence_revision.checked_add(1) else {
                        return false;
                    };
                    prior.evidence_revision = next;
                }
            }
            let Some(item) = state.intents.get_mut(&receipt.data().intent_id) else {
                return false;
            };
            let Some(prepared) = &item.prepared else {
                return false;
            };
            if receipt.data().dispatch_id != prepared.action.dispatch_id
                || receipt.data().action_digest != prepared.action_digest
                || receipt.data().schema_version != state.task.data().schema_version
                || !receipt_shape_matches(&prepared.request, &receipt.data().output)
            {
                return false;
            }
            if let (
                ApprovedRequest::RequestLocalModel {
                    input_digest,
                    output_schema,
                    ..
                },
                ReceiptOutput::LocalModel { observation },
            ) = (&prepared.request, &receipt.data().output)
            {
                let Some(profile) = &state.genesis.local_profile else {
                    return false;
                };
                if matches!(&receipt.data().usage, ActualUsage::Known { usage } if usage.model_requests.as_str() == "0" || usage.spend_micros_usd.as_str() != "0")
                    || observation.input_digest != *input_digest
                    || observation
                        .validate(profile, &prepared.request_digest, *output_schema)
                        .is_err()
                {
                    return false;
                }
            }
            if !state.evidence.contains_key(evidence_id) {
                let Ok(next_intent) = item.evidence_revision.checked_add(1) else {
                    return false;
                };
                let Ok(next_task) = state.verification.evidence_revision.checked_add(1) else {
                    return false;
                };
                item.evidence_revision = next_intent;
                state.verification.evidence_revision = next_task;
            }
            if item.evidence_revision != *evidence_revision {
                return false;
            }
            if *conflict {
                state.verification.conflict = true;
                for old in state.evidence.values_mut().filter(|old| {
                    old.receipt.data().producer_receipt_id == receipt.data().producer_receipt_id
                }) {
                    old.conflict = true;
                }
            }
            state.evidence.insert(
                evidence_id.clone(),
                EvidenceSnapshot {
                    evidence_id: evidence_id.clone(),
                    receipt: receipt.clone(),
                    selected: *settled,
                    conflict: *conflict,
                },
            );
            if *settled && !select_evidence(state, evidence_id) {
                return false;
            }
        }
        (
            StoreOperation::SettleReconciliation {
                task_id: id,
                reconciled_receipt: Some(receipt),
                ..
            },
            StoreResult::ReceiptRecorded { settled: false, .. },
        ) if id == task_id => {
            let operation = StoreOperation::RecordReceipt {
                receipt: receipt.clone(),
                lease_epoch: SqlU64::new(0).expect("zero is representable"),
            };
            return apply_lifecycle(state, &operation, result);
        }
        (
            StoreOperation::SettleReconciliation {
                task_id: id,
                intent_id,
                worker,
                lease_epoch,
                expected_evidence_revision,
                evidence_id,
                reconciled_receipt,
                ..
            },
            StoreResult::ReconciliationSettled {
                evidence_id: selected,
            },
        ) if id == task_id => {
            if selected != evidence_id {
                return false;
            }
            let Some(item) = state.intents.get_mut(intent_id) else {
                return false;
            };
            if item.evidence_revision != *expected_evidence_revision
                || !item.reconciliation_lease.as_ref().is_some_and(|lease| {
                    &lease.worker == worker
                        && lease.epoch == *lease_epoch
                        && lease.expires_at > state.tick
                })
            {
                return false;
            }
            if let Some(receipt) = reconciled_receipt {
                if receipt.evidence_id().ok().as_ref() != Some(evidence_id)
                    || receipt.data().intent_id != *intent_id
                    || receipt.data().schema_version != state.task.data().schema_version
                {
                    return false;
                }
                if item.prepared.as_ref().is_none_or(|prepared| {
                    !receipt_shape_matches(&prepared.request, &receipt.data().output)
                        || receipt.data().task_id != *id
                        || receipt.data().dispatch_id != prepared.action.dispatch_id
                        || receipt.data().action_digest != prepared.action_digest
                }) {
                    return false;
                }
                if let Some(prepared) = &item.prepared
                    && let (
                        ApprovedRequest::RequestLocalModel {
                            input_digest,
                            output_schema,
                            ..
                        },
                        ReceiptOutput::LocalModel { observation },
                    ) = (&prepared.request, &receipt.data().output)
                {
                    let Some(profile) = &state.genesis.local_profile else {
                        return false;
                    };
                    if matches!(&receipt.data().usage, ActualUsage::Known { usage } if usage.model_requests.as_str() == "0" || usage.spend_micros_usd.as_str() != "0")
                        || observation.input_digest != *input_digest
                        || observation
                            .validate(profile, &prepared.request_digest, *output_schema)
                            .is_err()
                    {
                        return false;
                    }
                }
                if !state.evidence.contains_key(evidence_id) {
                    let Ok(next_intent) = item.evidence_revision.checked_add(1) else {
                        return false;
                    };
                    let Ok(next_task) = state.verification.evidence_revision.checked_add(1) else {
                        return false;
                    };
                    item.evidence_revision = next_intent;
                    state.verification.evidence_revision = next_task;
                    state.evidence.insert(
                        evidence_id.clone(),
                        EvidenceSnapshot {
                            evidence_id: evidence_id.clone(),
                            receipt: receipt.clone(),
                            selected: false,
                            conflict: false,
                        },
                    );
                }
            }
            if state.evidence.get(evidence_id).is_none_or(|evidence| {
                evidence.receipt.data().task_id != *id
                    || evidence.receipt.data().intent_id != *intent_id
            }) || !select_evidence(state, evidence_id)
            {
                return false;
            }
        }
        (
            StoreOperation::RecordVerification {
                task_id: id,
                selection,
                ..
            },
            StoreResult::VerificationRecorded,
        ) if id == task_id => {
            if state.intents.values().any(|item| {
                matches!(
                    item.status,
                    DispatchStatus::Dispatched | DispatchStatus::Uncertain
                ) || item
                    .reservation
                    .as_ref()
                    .is_some_and(|reservation| reservation.status == ReservationStatus::Held)
            }) {
                return false;
            }
            if state.state.phase != Phase::AwaitingVerification
                || selection.evidence_revision != state.verification.evidence_revision
                || state.verification.read_evidence.as_ref() != Some(&selection.read_evidence)
                || state.verification.model_evidence.as_ref() != Some(&selection.model_evidence)
                || state.verification.publication_evidence.as_ref()
                    != Some(&selection.publication_evidence)
                || state.verification.conflict
            {
                return false;
            }
            let Some(publication) = state.intents.values().find(|item| {
                item.selected_evidence.as_ref() == Some(&selection.publication_evidence)
            }) else {
                return false;
            };
            if publication
                .prepared
                .as_ref()
                .map(|value| &value.request_digest)
                != Some(&selection.plan_digest)
            {
                return false;
            }
        }
        _ => {}
    }
    true
}

fn select_evidence(state: &mut Reconstruction, evidence_id: &Digest) -> bool {
    let Some(evidence) = state.evidence.get_mut(evidence_id) else {
        return false;
    };
    let receipt = &evidence.receipt;
    let Some(item) = state.intents.get_mut(&receipt.data().intent_id) else {
        return false;
    };
    if item
        .selected_evidence
        .as_ref()
        .is_some_and(|selected| selected != evidence_id)
    {
        return false;
    }
    evidence.selected = true;
    if let Some(reservation) = &mut item.reservation {
        reservation.status = ReservationStatus::Settled;
    }
    item.status = DispatchStatus::Settled;
    item.selected_evidence = Some(evidence_id.clone());
    match &receipt.data().output {
        ReceiptOutput::Fixture { .. } => {
            state.verification.read = Some(receipt.clone());
            state.verification.read_evidence = Some(evidence_id.clone());
        }
        ReceiptOutput::Model { .. } | ReceiptOutput::LocalModel { .. } => {
            state.verification.model = Some(receipt.clone());
            state.verification.model_evidence = Some(evidence_id.clone());
        }
        ReceiptOutput::Published { .. } => {
            let Some(PreparedIntent {
                request: ApprovedRequest::PublishArtifact { plan },
                ..
            }) = &item.prepared
            else {
                return false;
            };
            state.verification.plan = Some(plan.clone());
            state.verification.publication = Some(receipt.clone());
            state.verification.publication_evidence = Some(evidence_id.clone());
        }
        ReceiptOutput::Failure { .. } => {}
    }
    true
}

fn compare_inbox(
    snapshot: &mut impl RuntimeSnapshot,
    state: &Reconstruction,
) -> Result<bool, StoreError> {
    let mut actual = BTreeMap::new();
    let mut cursor = None;
    loop {
        let page = snapshot.inbox(&state.task.data().task_id, cursor)?;
        if page.records.len() > MAX_PAGE_RECORDS {
            return Ok(false);
        }
        let mut size = 0usize;
        for item in page.records {
            let Ok(bytes) = canonical_bytes(&item, MAX_INTERNAL) else {
                return Ok(false);
            };
            let Some(next) = size.checked_add(bytes.len()) else {
                return Ok(false);
            };
            size = next;
            if size > MAX_PAGE_BYTES || actual.insert(item.input.id.clone(), item).is_some() {
                return Ok(false);
            }
        }
        match page.next {
            Some(next) if cursor.is_none_or(|old| next > old) => cursor = Some(next),
            Some(_) => return Ok(false),
            None => break,
        }
    }
    let oldest = state
        .inputs
        .values()
        .filter(|item| item.status == InboxStatus::Pending)
        .min_by_key(|item| (item.input.source_seq, item.input.output_ordinal))
        .map(|item| item.input.clone());
    Ok(actual == state.inputs && snapshot.pending_inbox(&state.task.data().task_id)? == oldest)
}

fn compare_intents(
    snapshot: &mut impl RuntimeSnapshot,
    state: &Reconstruction,
) -> Result<bool, StoreError> {
    let mut actual = BTreeMap::new();
    let mut cursor = None;
    loop {
        let page = snapshot.intents(&state.task.data().task_id, cursor)?;
        if page.records.len() > MAX_PAGE_RECORDS {
            return Ok(false);
        }
        let mut size = 0usize;
        for item in page.records {
            let Ok(bytes) = canonical_bytes(&item, MAX_INTERNAL) else {
                return Ok(false);
            };
            let Some(next) = size.checked_add(bytes.len()) else {
                return Ok(false);
            };
            size = next;
            if size > MAX_PAGE_BYTES || actual.insert(item.intent_id.clone(), item).is_some() {
                return Ok(false);
            }
        }
        match page.next {
            Some(next) if cursor.is_none_or(|old| next > old) => cursor = Some(next),
            Some(_) => return Ok(false),
            None => break,
        }
    }
    if actual != state.intents {
        return Ok(false);
    }
    for (id, expected) in &state.intents {
        if snapshot.intent(id)?.as_ref() != Some(expected) {
            return Ok(false);
        }
        let mut evidence = BTreeMap::new();
        let mut after = None;
        loop {
            let page = snapshot.evidence(id, after)?;
            if page.records.len() > MAX_PAGE_RECORDS {
                return Ok(false);
            }
            let mut size = 0usize;
            for entry in page.records {
                let Ok(bytes) = canonical_bytes(&entry, MAX_INTERNAL) else {
                    return Ok(false);
                };
                let Some(next) = size.checked_add(bytes.len()) else {
                    return Ok(false);
                };
                size = next;
                if size > MAX_PAGE_BYTES
                    || evidence.insert(entry.evidence_id.clone(), entry).is_some()
                {
                    return Ok(false);
                }
            }
            match page.next {
                Some(next) if after.is_none_or(|old| next > old) => after = Some(next),
                Some(_) => return Ok(false),
                None => break,
            }
        }
        let expected: BTreeMap<_, _> = state
            .evidence
            .iter()
            .filter(|(_, item)| &item.receipt.data().intent_id == id)
            .map(|(id, item)| (id.clone(), item.clone()))
            .collect();
        if evidence != expected {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Reconstruct and compare the chain, accounting and materialized state in one
/// coherent snapshot. Corruption is reported in the result; read failures propagate.
pub fn verify(
    snapshot: &mut impl RuntimeSnapshot,
    task: &TaskId,
) -> Result<AuditResult, StoreError> {
    let invalid = AuditResult {
        chain_valid: false,
        accounting_valid: false,
        projection_valid: false,
    };
    let projection = snapshot.projection(task)?;
    if !check_chain(snapshot, &projection.replay_head)? {
        return Ok(invalid);
    }
    let Some(mut accounts) = reconstruct_accounts(snapshot)? else {
        return Ok(AuditResult {
            chain_valid: true,
            ..invalid
        });
    };
    // Receipt payloads were needed for the accounting fold, but are reconstructed
    // separately for projection checks; release them before the next pass.
    accounts.receipts.clear();
    let Some(mut state) = parse_journal(snapshot, task, &accounts)? else {
        return Ok(AuditResult {
            chain_valid: true,
            ..invalid
        });
    };
    let Some(task_account) = accounts.tasks.get(task) else {
        return Ok(AuditResult {
            chain_valid: true,
            ..invalid
        });
    };
    let Some(principal_account) = accounts.principals.get(&state.task.data().principal_id) else {
        return Ok(AuditResult {
            chain_valid: true,
            ..invalid
        });
    };
    let empty = Vec::new();
    let unposted = accounts.unposted.get(task).unwrap_or(&empty);
    let accounting_valid = &projection.task_account == task_account
        && &projection.principal_account == principal_account
        && unposted_matches(snapshot, task, unposted)?;
    for intent in state.intents.values_mut() {
        if intent.status == DispatchStatus::Settled
            && let Some(reservation) = &mut intent.reservation
            && !intent
                .dispatch_id
                .as_ref()
                .is_some_and(|id| unposted.iter().any(|debit| &debit.dispatch_id == id))
        {
            reservation.status = ReservationStatus::Settled;
        }
    }
    state.verification.unresolved = task_account.incomplete
        || task_account.integrity_hold
        || state.intents.values().any(|item| {
            matches!(
                item.status,
                DispatchStatus::Dispatched | DispatchStatus::Uncertain
            ) || item
                .reservation
                .as_ref()
                .is_some_and(|reservation| reservation.status == ReservationStatus::Held)
        });
    let projection_valid = projection.task == state.task
        && projection.genesis == state.genesis
        && projection.state == state.state
        && projection.tick == state.tick
        && projection.verification == state.verification
        && snapshot.grant(&state.task.data().principal_id)? == state.grant
        && compare_inbox(snapshot, &state)?
        && compare_intents(snapshot, &state)?;
    Ok(AuditResult {
        chain_valid: true,
        accounting_valid,
        projection_valid,
    })
}

fn check_chain(
    snapshot: &mut impl RuntimeSnapshot,
    expected_head: &Digest,
) -> Result<bool, StoreError> {
    let mut cursor = None;
    let mut expected_seq = Some(1_u64);
    let mut previous = None;
    let mut saw_record = false;
    let mut record_count = 0usize;
    let mut journal_bytes = 0usize;
    loop {
        let page = snapshot.journal_page(cursor)?;
        if page.records.len() > MAX_PAGE_RECORDS {
            return Ok(false);
        }
        let mut size = 0usize;
        for record in &page.records {
            let data = record.data();
            let Ok(bytes) = canonical_bytes(record, MAX_INTERNAL) else {
                return Ok(false);
            };
            record_count = record_count.saturating_add(1);
            journal_bytes = journal_bytes.saturating_add(bytes.len());
            if record_count > MAX_AUDIT_RECORDS || journal_bytes > MAX_AUDIT_JOURNAL_BYTES {
                return Err(StoreError::Operational(
                    BoundedText::new("audit resource limit exceeded")
                        .expect("fixed diagnostic is bounded"),
                ));
            }
            let Some(next) = size.checked_add(bytes.len()) else {
                return Ok(false);
            };
            size = next;
            if size > MAX_PAGE_BYTES || Some(data.seq.get()) != expected_seq {
                return Ok(false);
            }
            if !saw_record {
                let JournalEvent::CommandCommitted { command, result } = &data.event;
                let (
                    StoreOperation::CreateTask { task, genesis },
                    StoreResult::TaskCreated { genesis_digest },
                ) = (&command.data().operation, result)
                else {
                    return Ok(false);
                };
                let Ok(head) = journal_genesis(task, genesis) else {
                    return Ok(false);
                };
                if &head != genesis_digest {
                    return Ok(false);
                }
                previous = Some(head);
                saw_record = true;
            }
            if previous.as_ref() != Some(&data.previous)
                || journal_entry(&data.previous, data.seq, &data.event)
                    .ok()
                    .as_ref()
                    != Some(&data.digest)
            {
                return Ok(false);
            }
            previous = Some(data.digest.clone());
            expected_seq = data
                .seq
                .get()
                .checked_add(1)
                .filter(|seq| *seq <= MAX_SQL_INTEGER);
        }
        match page.next {
            Some(next)
                if page.records.last().is_some_and(|r| r.data().seq == next)
                    && cursor.is_none_or(|old| next > old) =>
            {
                cursor = Some(next)
            }
            Some(_) => return Ok(false),
            None => break,
        }
    }
    Ok(saw_record && previous.as_ref() == Some(expected_head))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scripted_receipt_cannot_substitute_for_local_request() {
        let request = ApprovedRequest::RequestLocalModel {
            input_digest: content_digest(b"input"),
            context: BoundedText::new("input").unwrap(),
            profile_digest: content_digest(b"profile"),
            role: metron_protocol::LocalRole::Solver,
            output_schema: metron_protocol::LocalOutputSchema::Sum,
            tariff: Tariff {
                model_requests: SqlU64::new(1).unwrap(),
                tool_calls: SqlU64::new(0).unwrap(),
                spend_micros_usd: SqlU64::new(0).unwrap(),
            },
        };
        let output = ReceiptOutput::Model {
            candidate: CandidateResult {
                count: 1,
                sum: 1,
                input_digest: content_digest(b"input"),
            },
        };
        assert!(!receipt_shape_matches(&request, &output));
    }
    fn admitted() -> (Accounts, AdmissionCheck) {
        let task: Task = parse_envelope(br#"{"schema_version":1,"task_id":"task","principal_id":"principal","objective":"sum","budget":{"max_model_requests":1,"max_tool_calls":2,"max_spend_micros_usd":0,"max_elapsed_ms":null}}"#).unwrap();
        let genesis = content_digest(b"genesis");
        let intent = Intent {
            schema_version: SchemaVersion::V1,
            task_id: task.data().task_id.clone(),
            ordinal: 0,
            kind: IntentKind::ReadFixture,
        };
        let prepared =
            PreparedIntent::materialize(&intent, &task.data().principal_id, &genesis).unwrap();
        let zero = SqlU64::new(0).unwrap();
        let epoch = SqlU64::new(1).unwrap();
        let worker = WorkerId::new("worker").unwrap();
        let check = AdmissionCheck {
            intent_id: prepared.intent_id.clone(),
            prepared,
            worker: worker.clone(),
            lease_epoch: epoch,
            expected_tick: zero,
            grant_revision: zero,
            approval_id: None,
            reservation_id: content_digest(b"reservation"),
        };
        let mut accounts = Accounts::default();
        assert!(accounts.create(&task));
        accounts.leases.insert(
            check.intent_id.clone(),
            LeaseSnapshot {
                worker,
                epoch,
                expires_at: SqlU64::new(30).unwrap(),
            },
        );
        assert!(accounts.reserve(&check));
        (accounts, check)
    }
    fn usage_receipt(check: &AdmissionCheck, model: &str, tools: &str) -> Receipt {
        Receipt::new(ReceiptData {
            schema_version: SchemaVersion::V1,
            producer_receipt_id: ProducerReceiptId::new("producer").unwrap(),
            task_id: check.prepared.action.task_id.clone(),
            intent_id: check.intent_id.clone(),
            dispatch_id: check.prepared.action.dispatch_id.clone(),
            action_digest: check.prepared.action_digest.clone(),
            usage: ActualUsage::Known {
                usage: UsageVector {
                    model_requests: DecimalUsage::new(model).unwrap(),
                    tool_calls: DecimalUsage::new(tools).unwrap(),
                    spend_micros_usd: DecimalUsage::new("0").unwrap(),
                },
            },
            output: ReceiptOutput::Failure {
                diagnostic: BoundedText::new("failure").unwrap(),
            },
        })
        .unwrap()
    }
    #[test]
    fn posts_full_overrun_once_and_flags_both_accounts() {
        let (mut accounts, check) = admitted();
        let receipt = usage_receipt(&check, "0", "3");
        let evidence = receipt.evidence_id().unwrap();
        assert!(accounts.receipt(&receipt, &evidence, true, false, Some(check.lease_epoch)));
        assert!(accounts.receipt(&receipt, &evidence, true, false, Some(check.lease_epoch)));
        for account in [
            &accounts.tasks[&check.prepared.action.task_id],
            &accounts.principals[&check.prepared.action.principal_id],
        ] {
            assert_eq!(account.posted.tool_calls.get(), 3);
            assert_eq!(account.reserved.tool_calls.get(), 0);
            assert!(account.breached);
        }
    }
    #[test]
    fn unrepresentable_vector_posts_nothing_and_holds_reservation() {
        let (mut accounts, check) = admitted();
        let receipt = usage_receipt(&check, "18446744073709551616", "2");
        let evidence = receipt.evidence_id().unwrap();
        assert!(accounts.receipt(&receipt, &evidence, false, false, Some(check.lease_epoch)));
        assert!(accounts.receipt(&receipt, &evidence, false, false, Some(check.lease_epoch)));
        for account in [
            &accounts.tasks[&check.prepared.action.task_id],
            &accounts.principals[&check.prepared.action.principal_id],
        ] {
            assert_eq!(account.posted.tool_calls.get(), 0);
            assert_eq!(account.reserved.tool_calls.get(), 1);
            assert!(account.incomplete);
        }
        assert_eq!(accounts.unposted[&check.prepared.action.task_id].len(), 1);
        assert_eq!(
            accounts.unposted[&check.prepared.action.task_id][0]
                .usage
                .model_requests
                .as_str(),
            "18446744073709551616"
        );
    }
    #[test]
    fn fresh_representable_receipt_cannot_claim_no_settlement() {
        let (mut accounts, check) = admitted();
        let receipt = usage_receipt(&check, "0", "1");
        let evidence = receipt.evidence_id().unwrap();
        assert!(!accounts.receipt(&receipt, &evidence, false, false, Some(check.lease_epoch)));
    }
    #[test]
    fn false_collision_claim_is_rejected() {
        let (mut accounts, check) = admitted();
        let receipt = usage_receipt(&check, "0", "1");
        let evidence = receipt.evidence_id().unwrap();
        assert!(!accounts.receipt(&receipt, &evidence, false, true, Some(check.lease_epoch)));
    }
    #[test]
    fn reservation_identifier_cannot_be_reused_for_another_intent() {
        let (mut accounts, check) = admitted();
        let mut other = check.clone();
        other.intent_id = content_digest(b"different intent");
        accounts.leases.insert(
            other.intent_id.clone(),
            LeaseSnapshot {
                worker: other.worker.clone(),
                epoch: other.lease_epoch,
                expires_at: SqlU64::new(30).unwrap(),
            },
        );
        assert!(!accounts.reserve(&other));
    }
    #[test]
    fn tariff_overrun_breaches_even_with_budget_headroom() {
        let (mut accounts, check) = admitted();
        accounts
            .tasks
            .get_mut(&check.prepared.action.task_id)
            .unwrap()
            .limit
            .tool_calls = SqlU64::new(8).unwrap();
        accounts
            .principals
            .get_mut(&check.prepared.action.principal_id)
            .unwrap()
            .limit
            .tool_calls = SqlU64::new(8).unwrap();
        let receipt = usage_receipt(&check, "0", "2");
        let evidence = receipt.evidence_id().unwrap();
        assert!(accounts.receipt(&receipt, &evidence, true, false, Some(check.lease_epoch)));
        for account in [
            &accounts.tasks[&check.prepared.action.task_id],
            &accounts.principals[&check.prepared.action.principal_id],
        ] {
            assert_eq!(account.posted.tool_calls.get(), 2);
            assert!(account.breached);
        }
    }
    #[test]
    fn dispatch_epoch_does_not_authorize_reconciliation_settlement() {
        let (_, check) = admitted();
        let task: Task = parse_envelope(br#"{"schema_version":1,"task_id":"task","principal_id":"principal","objective":"sum","budget":{"max_model_requests":1,"max_tool_calls":2,"max_spend_micros_usd":0,"max_elapsed_ms":null}}"#).unwrap();
        let zero = SqlU64::new(0).unwrap();
        let genesis = Genesis {
            mode: EngineMode::Agent,
            engine_version: BoundedText::new("sum-agent/v1").unwrap(),
            require_publication_approval: false,
            policy_digest: content_digest(b"policy"),
            local_profile: None,
        };
        let initial = AgentState {
            task_id: task.data().task_id.clone(),
            genesis_digest: content_digest(b"genesis"),
            max_decision_steps: 8,
            revision: zero,
            decision_steps: 0,
            phase: Phase::New,
            pending_intent: None,
        };
        let mut state = Reconstruction {
            task: task.clone(),
            genesis,
            state: initial,
            inputs: BTreeMap::new(),
            intents: BTreeMap::new(),
            evidence: BTreeMap::new(),
            tick: zero,
            verification: VerificationSnapshot {
                evidence_revision: zero,
                read: None,
                model: None,
                publication: None,
                read_evidence: None,
                model_evidence: None,
                publication_evidence: None,
                plan: None,
                unresolved: true,
                conflict: false,
            },
            grant: GrantSnapshot {
                principal_id: task.data().principal_id.clone(),
                revision: zero,
                revoked: false,
            },
            seen_receipts: BTreeSet::new(),
            uncertain: BTreeSet::new(),
            last_seq: zero,
            head: content_digest(b"head"),
        };
        state.intents.insert(
            check.intent_id.clone(),
            IntentSnapshot {
                intent_id: check.intent_id.clone(),
                intent: Intent {
                    schema_version: SchemaVersion::V1,
                    task_id: task.data().task_id.clone(),
                    ordinal: 0,
                    kind: IntentKind::ReadFixture,
                },
                prepared: Some(check.prepared.clone()),
                status: DispatchStatus::Dispatched,
                dispatch_id: Some(check.prepared.action.dispatch_id.clone()),
                lease_epoch: check.lease_epoch,
                lease: Some(LeaseSnapshot {
                    worker: check.worker.clone(),
                    epoch: check.lease_epoch,
                    expires_at: SqlU64::new(30).unwrap(),
                }),
                reconciliation_epoch: zero,
                reconciliation_lease: None,
                reservation: None,
                approval: None,
                evidence_revision: zero,
                selected_evidence: None,
            },
        );
        let receipt = usage_receipt(&check, "0", "1");
        let evidence_id = receipt.evidence_id().unwrap();
        let operation = StoreOperation::SettleReconciliation {
            task_id: task.data().task_id.clone(),
            intent_id: check.intent_id.clone(),
            worker: check.worker.clone(),
            lease_epoch: check.lease_epoch,
            expected_evidence_revision: zero,
            evidence_id: evidence_id.clone(),
            reconciled_receipt: Some(receipt),
        };
        assert!(!apply_lifecycle(
            &mut state,
            &operation,
            &StoreResult::ReconciliationSettled { evidence_id }
        ));

        let receipt = usage_receipt(&check, "0", "1");
        let evidence_id = receipt.evidence_id().unwrap();
        state.evidence.insert(
            evidence_id.clone(),
            EvidenceSnapshot {
                evidence_id: evidence_id.clone(),
                receipt,
                selected: false,
                conflict: false,
            },
        );
        let other_id = content_digest(b"other intent");
        let mut other = state.intents[&check.intent_id].clone();
        other.intent_id = other_id.clone();
        other.reconciliation_lease = Some(LeaseSnapshot {
            worker: check.worker.clone(),
            epoch: check.lease_epoch,
            expires_at: SqlU64::new(30).unwrap(),
        });
        state.intents.insert(other_id.clone(), other);
        let cross_intent = StoreOperation::SettleReconciliation {
            task_id: task.data().task_id.clone(),
            intent_id: other_id,
            worker: check.worker.clone(),
            lease_epoch: check.lease_epoch,
            expected_evidence_revision: zero,
            evidence_id: evidence_id.clone(),
            reconciled_receipt: None,
        };
        assert!(!apply_lifecycle(
            &mut state,
            &cross_intent,
            &StoreResult::ReconciliationSettled { evidence_id }
        ));

        state
            .intents
            .get_mut(&check.intent_id)
            .unwrap()
            .reconciliation_lease = Some(LeaseSnapshot {
            worker: check.worker.clone(),
            epoch: check.lease_epoch,
            expires_at: SqlU64::new(30).unwrap(),
        });
        let mut forged = usage_receipt(&check, "0", "1").data().clone();
        forged.action_digest = content_digest(b"forged action");
        let forged = Receipt::new(forged).unwrap();
        let forged_id = forged.evidence_id().unwrap();
        let operation = StoreOperation::SettleReconciliation {
            task_id: task.data().task_id.clone(),
            intent_id: check.intent_id.clone(),
            worker: check.worker.clone(),
            lease_epoch: check.lease_epoch,
            expected_evidence_revision: zero,
            evidence_id: forged_id.clone(),
            reconciled_receipt: Some(forged),
        };
        assert!(!apply_lifecycle(
            &mut state,
            &operation,
            &StoreResult::ReconciliationSettled {
                evidence_id: forged_id
            }
        ));

        let request_id = ApprovalId::new("approval").unwrap();
        let item = state.intents.get_mut(&check.intent_id).unwrap();
        item.status = DispatchStatus::Denied;
        item.approval = Some(ApprovalSnapshot {
            task_id: task.data().task_id.clone(),
            intent_id: check.intent_id.clone(),
            request_id: request_id.clone(),
            action_digest: check.prepared.action_digest.clone(),
            expires_at: SqlU64::new(30).unwrap(),
            decision: None,
        });
        let approval = StoreOperation::RecordApproval {
            request_id,
            action_digest: check.prepared.action_digest.clone(),
            decision: ApprovalDecision::Approve,
        };
        assert!(!apply_lifecycle(
            &mut state,
            &approval,
            &StoreResult::ApprovalRecorded
        ));
        let item = state.intents.get_mut(&check.intent_id).unwrap();
        item.approval.as_mut().unwrap().decision = Some(ApprovalDecision::Approve);
        assert!(apply_lifecycle(
            &mut state,
            &approval,
            &StoreResult::ApprovalRecorded
        ));
        assert_eq!(
            state.intents[&check.intent_id].status,
            DispatchStatus::Denied
        );
    }
    #[test]
    fn previously_committed_admission_after_settlement_does_not_reserve_again() {
        let (mut accounts, mut check) = admitted();
        let receipt = usage_receipt(&check, "0", "1");
        let evidence = receipt.evidence_id().unwrap();
        assert!(accounts.receipt(&receipt, &evidence, true, false, Some(check.lease_epoch)));
        accounts.tick = 31;
        check.expected_tick = SqlU64::new(31).unwrap();
        assert!(accounts.reserve(&check));
        let account = &accounts.tasks[&check.prepared.action.task_id];
        assert_eq!(account.posted.tool_calls.get(), 1);
        assert_eq!(account.reserved.tool_calls.get(), 0);
    }
    #[test]
    fn unknown_usage_fences_later_direct_receipt_from_settlement() {
        let (mut accounts, check) = admitted();
        let original = usage_receipt(&check, "0", "1");
        let mut data = original.data().clone();
        data.usage = ActualUsage::Unknown {
            reason: BoundedText::new("unknown").unwrap(),
            evidence: None,
        };
        let unknown = Receipt::new(data.clone()).unwrap();
        assert!(accounts.receipt(
            &unknown,
            &unknown.evidence_id().unwrap(),
            false,
            false,
            Some(check.lease_epoch)
        ));
        data.producer_receipt_id = ProducerReceiptId::new("other").unwrap();
        data.usage = original.data().usage.clone();
        let later = Receipt::new(data).unwrap();
        assert!(accounts.receipt(
            &later,
            &later.evidence_id().unwrap(),
            false,
            false,
            Some(check.lease_epoch)
        ));
        for account in [
            &accounts.tasks[&check.prepared.action.task_id],
            &accounts.principals[&check.prepared.action.principal_id],
        ] {
            assert!(account.incomplete && account.integrity_hold);
            assert_eq!(account.posted.tool_calls.get(), 0);
            assert_eq!(account.reserved.tool_calls.get(), 1);
        }
    }
    #[test]
    fn previous_conflict_fences_later_different_producer_receipt() {
        let (mut accounts, check) = admitted();
        accounts.conflicted.insert(check.intent_id.clone());
        let receipt = usage_receipt(&check, "0", "1");
        assert!(accounts.receipt(
            &receipt,
            &receipt.evidence_id().unwrap(),
            false,
            true,
            Some(check.lease_epoch)
        ));
        for account in [
            &accounts.tasks[&check.prepared.action.task_id],
            &accounts.principals[&check.prepared.action.principal_id],
        ] {
            assert!(account.integrity_hold);
            assert_eq!(account.posted.tool_calls.get(), 0);
            assert_eq!(account.reserved.tool_calls.get(), 1);
        }
    }
    #[test]
    fn stale_unselected_evidence_fences_later_live_receipt() {
        let (mut accounts, check) = admitted();
        let stale = usage_receipt(&check, "0", "1");
        assert!(accounts.receipt(
            &stale,
            &stale.evidence_id().unwrap(),
            false,
            false,
            Some(SqlU64::new(0).unwrap())
        ));
        let mut data = stale.data().clone();
        data.producer_receipt_id = ProducerReceiptId::new("later").unwrap();
        let later = Receipt::new(data).unwrap();
        assert!(accounts.receipt(
            &later,
            &later.evidence_id().unwrap(),
            false,
            false,
            Some(check.lease_epoch)
        ));
        for account in [
            &accounts.tasks[&check.prepared.action.task_id],
            &accounts.principals[&check.prepared.action.principal_id],
        ] {
            assert!(account.integrity_hold);
            assert_eq!(account.posted.tool_calls.get(), 0);
            assert_eq!(account.reserved.tool_calls.get(), 1);
        }
    }
}
