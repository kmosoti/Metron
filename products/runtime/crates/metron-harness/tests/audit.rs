use metron_harness::audit::verify;
use metron_protocol::*;

struct Snapshot {
    projection: Projection,
    journal: Vec<JournalRecord>,
    inbox: Vec<InboxSnapshot>,
    intents: Vec<IntentSnapshot>,
}
impl AuditSnapshot for Snapshot {
    fn projection(&mut self, _: &TaskId) -> Result<Projection, StoreError> {
        Ok(self.projection.clone())
    }
    fn journal_page(&mut self, after: Option<SqlU64>) -> Result<JournalPage, StoreError> {
        let records = self
            .journal
            .iter()
            .filter(|record| after.is_none_or(|value| record.data().seq > value))
            .take(15)
            .cloned()
            .collect::<Vec<_>>();
        let next = if records.len() == 15
            && self
                .journal
                .last()
                .is_some_and(|last| last.data().seq > records.last().unwrap().data().seq)
        {
            Some(records.last().unwrap().data().seq)
        } else {
            None
        };
        Ok(JournalPage { records, next })
    }
    fn pending_inbox(&mut self, _: &TaskId) -> Result<Option<InboxInput>, StoreError> {
        Ok(self
            .inbox
            .iter()
            .filter(|item| item.status == InboxStatus::Pending)
            .min_by_key(|item| item.input.source_seq)
            .map(|item| item.input.clone()))
    }
}
impl RuntimeSnapshot for Snapshot {
    fn intent(&mut self, id: &Digest) -> Result<Option<IntentSnapshot>, StoreError> {
        Ok(self
            .intents
            .iter()
            .find(|intent| &intent.intent_id == id)
            .cloned())
    }
    fn intents(
        &mut self,
        _: &TaskId,
        _: Option<SqlU64>,
    ) -> Result<SnapshotPage<IntentSnapshot>, StoreError> {
        Ok(SnapshotPage {
            records: self.intents.clone(),
            next: None,
        })
    }
    fn grant(&mut self, _: &PrincipalId) -> Result<GrantSnapshot, StoreError> {
        Ok(GrantSnapshot {
            principal_id: self.projection.task.data().principal_id.clone(),
            revision: SqlU64::new(0).unwrap(),
            revoked: false,
        })
    }
    fn evidence(
        &mut self,
        _: &Digest,
        _: Option<SqlU64>,
    ) -> Result<SnapshotPage<EvidenceSnapshot>, StoreError> {
        Ok(SnapshotPage {
            records: vec![],
            next: None,
        })
    }
    fn inbox(
        &mut self,
        _: &TaskId,
        _: Option<SqlU64>,
    ) -> Result<SnapshotPage<InboxSnapshot>, StoreError> {
        Ok(SnapshotPage {
            records: self.inbox.clone(),
            next: None,
        })
    }
    fn unposted_debits(
        &mut self,
        _: &TaskId,
        _: Option<SqlU64>,
    ) -> Result<SnapshotPage<UnpostedDebit>, StoreError> {
        Ok(SnapshotPage {
            records: vec![],
            next: None,
        })
    }
}
fn setup() -> Snapshot {
    let task: Task = parse_envelope(br#"{"schema_version":1,"task_id":"task","principal_id":"principal","objective":"sum","budget":{"max_model_requests":1,"max_tool_calls":2,"max_spend_micros_usd":0,"max_elapsed_ms":null}}"#).unwrap();
    let genesis = Genesis {
        mode: EngineMode::Agent,
        engine_version: BoundedText::new("sum-agent/v1").unwrap(),
        require_publication_approval: false,
        policy_digest: content_digest(b"policy"),
        local_profile: None,
    };
    let head = journal_genesis(&task, &genesis).unwrap();
    let command = StoreCommand::new(CommandData {
        schema_version: SchemaVersion::V1,
        command_id: CommandId::new("create").unwrap(),
        operation: StoreOperation::CreateTask {
            task: task.clone(),
            genesis: genesis.clone(),
        },
    })
    .unwrap();
    let event = JournalEvent::CommandCommitted {
        command,
        result: StoreResult::TaskCreated {
            genesis_digest: head.clone(),
        },
    };
    let seq = SqlU64::new(1).unwrap();
    let digest = journal_entry(&head, seq, &event).unwrap();
    let record = JournalRecord::new(JournalRecordData {
        seq,
        previous: head.clone(),
        event,
        digest: digest.clone(),
    })
    .unwrap();
    let mut input = InboxInput {
        id: inbox_id(&task.data().task_id, seq, 0).unwrap(),
        task_id: task.data().task_id.clone(),
        principal_id: task.data().principal_id.clone(),
        source_seq: seq,
        output_ordinal: 0,
        source: InboxSource::TaskAccepted,
        evidence_id: None,
        event: AgentEvent::TaskAccepted,
        digest: head.clone(),
    };
    input.digest = inbox_digest(&input).unwrap();
    let zero = SqlU64::new(0).unwrap();
    let account = AccountSnapshot {
        limit: Counters {
            model_requests: SqlU64::new(1).unwrap(),
            tool_calls: SqlU64::new(2).unwrap(),
            spend_micros_usd: zero,
        },
        posted: Counters {
            model_requests: zero,
            tool_calls: zero,
            spend_micros_usd: zero,
        },
        reserved: Counters {
            model_requests: zero,
            tool_calls: zero,
            spend_micros_usd: zero,
        },
        breached: false,
        incomplete: false,
        integrity_hold: false,
    };
    let state = AgentState {
        task_id: task.data().task_id.clone(),
        genesis_digest: head,
        max_decision_steps: 8,
        revision: zero,
        decision_steps: 0,
        phase: Phase::New,
        pending_intent: None,
    };
    let verification = VerificationSnapshot {
        evidence_revision: zero,
        read: None,
        model: None,
        publication: None,
        read_evidence: None,
        model_evidence: None,
        publication_evidence: None,
        plan: None,
        unresolved: false,
        conflict: false,
    };
    Snapshot {
        projection: Projection {
            genesis,
            task,
            state,
            tick: zero,
            task_account: account.clone(),
            principal_account: account,
            verification,
            replay_head: digest,
        },
        journal: vec![record],
        intents: vec![],
        inbox: vec![InboxSnapshot {
            input,
            status: InboxStatus::Pending,
            transition: None,
        }],
    }
}
#[test]
fn reconstructs_genesis_and_detects_independent_projection_tampering() {
    let mut snapshot = setup();
    let id = snapshot.projection.task.data().task_id.clone();
    let result = verify(&mut snapshot, &id).unwrap();
    assert!(result.chain_valid && result.accounting_valid && result.projection_valid);
    snapshot.projection.task_account.posted.tool_calls = SqlU64::new(1).unwrap();
    let result = verify(&mut snapshot, &id).unwrap();
    assert!(result.chain_valid && !result.accounting_valid);
    snapshot = setup();
    snapshot.inbox[0].input.event = AgentEvent::Verified {
        accepted: true,
        reason: BoundedText::new("forged").unwrap(),
    };
    let result = verify(&mut snapshot, &id).unwrap();
    assert!(result.chain_valid && result.accounting_valid && !result.projection_valid);
    snapshot = setup();
    snapshot.projection.replay_head = content_digest(b"forged");
    assert!(!verify(&mut snapshot, &id).unwrap().chain_valid);
}

#[test]
fn new_command_id_for_semantic_noop_is_journaled_without_new_inbox() {
    let mut snapshot = setup();
    let first = snapshot.journal[0].data();
    let JournalEvent::CommandCommitted { command, result } = &first.event;
    let command = StoreCommand::new(CommandData {
        schema_version: SchemaVersion::V1,
        command_id: CommandId::new("create-again").unwrap(),
        operation: command.data().operation.clone(),
    })
    .unwrap();
    let event = JournalEvent::CommandCommitted {
        command,
        result: result.clone(),
    };
    let seq = SqlU64::new(2).unwrap();
    let previous = first.digest.clone();
    let digest = journal_entry(&previous, seq, &event).unwrap();
    snapshot.journal.push(
        JournalRecord::new(JournalRecordData {
            seq,
            previous,
            event,
            digest: digest.clone(),
        })
        .unwrap(),
    );
    snapshot.projection.replay_head = digest;
    let task = snapshot.projection.task.data().task_id.clone();
    let result = verify(&mut snapshot, &task).unwrap();
    assert!(result.chain_valid && result.accounting_valid && result.projection_valid);
    assert_eq!(snapshot.inbox.len(), 1);
}

#[test]
fn detects_intent_projection_tampering_with_valid_chain() {
    let mut snapshot = setup();
    let input = snapshot.inbox[0].input.clone();
    let genesis = snapshot.projection.state.genesis_digest.bytes();
    let id = structural_digest(
        "intent-id/v1",
        &[
            input.task_id.as_str().as_bytes(),
            &genesis,
            &0_u32.to_be_bytes(),
        ],
    )
    .unwrap();
    let intent = Intent {
        schema_version: SchemaVersion::V1,
        task_id: input.task_id.clone(),
        ordinal: 0,
        kind: IntentKind::ReadFixture,
    };
    let mut next = snapshot.projection.state.clone();
    next.revision = SqlU64::new(1).unwrap();
    next.decision_steps = 1;
    next.phase = Phase::WaitingRead;
    next.pending_intent = Some(id.clone());
    let transition = Transition::Decision {
        next: next.clone(),
        intent: intent.clone(),
    };
    let command = StoreCommand::new(CommandData {
        schema_version: SchemaVersion::V1,
        command_id: CommandId::new("transition").unwrap(),
        operation: StoreOperation::CommitTransition {
            task_id: input.task_id.clone(),
            expected_revision: SqlU64::new(0).unwrap(),
            inbox_id: input.id.clone(),
            inbox_digest: input.digest.clone(),
            transition: transition.clone(),
        },
    })
    .unwrap();
    let event = JournalEvent::CommandCommitted {
        command,
        result: StoreResult::TransitionCommitted {
            revision: next.revision,
            intent_id: Some(id.clone()),
        },
    };
    let seq = SqlU64::new(2).unwrap();
    let previous = snapshot.projection.replay_head.clone();
    let digest = journal_entry(&previous, seq, &event).unwrap();
    snapshot.journal.push(
        JournalRecord::new(JournalRecordData {
            seq,
            previous,
            event,
            digest: digest.clone(),
        })
        .unwrap(),
    );
    snapshot.projection.replay_head = digest;
    snapshot.projection.state = next;
    snapshot.inbox[0].status = InboxStatus::Consumed;
    snapshot.inbox[0].transition = Some(transition);
    let zero = SqlU64::new(0).unwrap();
    snapshot.intents.push(IntentSnapshot {
        intent_id: id,
        intent,
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
    });
    let task = input.task_id;
    let result = verify(&mut snapshot, &task).unwrap();
    assert!(result.chain_valid && result.accounting_valid && result.projection_valid);
    snapshot.intents[0].status = DispatchStatus::Denied;
    let result = verify(&mut snapshot, &task).unwrap();
    assert!(result.chain_valid && result.accounting_valid && !result.projection_valid);
}

#[test]
fn audit_record_budget_is_operational_and_inclusive() {
    let mut snapshot = setup();
    let JournalEvent::CommandCommitted { command, result } = &snapshot.journal[0].data().event;
    let operation = command.data().operation.clone();
    let result = result.clone();
    let mut previous = snapshot.projection.replay_head.clone();
    for number in 2..=MAX_AUDIT_RECORDS {
        let command = StoreCommand::new(CommandData {
            schema_version: SchemaVersion::V1,
            command_id: CommandId::new(format!("noop-{number}")).unwrap(),
            operation: operation.clone(),
        })
        .unwrap();
        let event = JournalEvent::CommandCommitted {
            command,
            result: result.clone(),
        };
        let seq = SqlU64::new(number as u64).unwrap();
        let digest = journal_entry(&previous, seq, &event).unwrap();
        snapshot.journal.push(
            JournalRecord::new(JournalRecordData {
                seq,
                previous,
                event,
                digest: digest.clone(),
            })
            .unwrap(),
        );
        previous = digest;
    }
    snapshot.projection.replay_head = previous.clone();
    let id = snapshot.projection.task.data().task_id.clone();
    let valid = verify(&mut snapshot, &id).unwrap();
    assert!(valid.chain_valid && valid.accounting_valid && valid.projection_valid);
    let command = StoreCommand::new(CommandData {
        schema_version: SchemaVersion::V1,
        command_id: CommandId::new("over-limit").unwrap(),
        operation,
    })
    .unwrap();
    let event = JournalEvent::CommandCommitted { command, result };
    let seq = SqlU64::new((MAX_AUDIT_RECORDS + 1) as u64).unwrap();
    let digest = journal_entry(&previous, seq, &event).unwrap();
    snapshot.journal.push(
        JournalRecord::new(JournalRecordData {
            seq,
            previous,
            event,
            digest: digest.clone(),
        })
        .unwrap(),
    );
    snapshot.projection.replay_head = digest;
    assert!(
        matches!(verify(&mut snapshot, &id), Err(StoreError::Operational(reason)) if reason.as_str() == "audit resource limit exceeded")
    );
}

#[test]
fn audit_byte_budget_rejects_large_valid_pages_as_operational_limit() {
    let mut snapshot = setup();
    let data = ReceiptData {
        schema_version: SchemaVersion::V1,
        producer_receipt_id: ProducerReceiptId::new("producer").unwrap(),
        task_id: snapshot.projection.task.data().task_id.clone(),
        intent_id: content_digest(b"intent"),
        dispatch_id: content_digest(b"dispatch"),
        action_digest: content_digest(b"action"),
        usage: ActualUsage::Known {
            usage: UsageVector {
                model_requests: DecimalUsage::new("9".repeat(60_000)).unwrap(),
                tool_calls: DecimalUsage::new("0").unwrap(),
                spend_micros_usd: DecimalUsage::new("0").unwrap(),
            },
        },
        output: ReceiptOutput::Failure {
            diagnostic: BoundedText::new("failure").unwrap(),
        },
    };
    let receipt = Receipt::new(data).unwrap();
    let evidence = receipt.evidence_id().unwrap();
    let mut previous = snapshot.projection.replay_head.clone();
    for number in 2..=150 {
        let command = StoreCommand::new(CommandData {
            schema_version: SchemaVersion::V1,
            command_id: CommandId::new(format!("receipt-{number}")).unwrap(),
            operation: StoreOperation::RecordReceipt {
                receipt: receipt.clone(),
                lease_epoch: SqlU64::new(1).unwrap(),
            },
        })
        .unwrap();
        let event = JournalEvent::CommandCommitted {
            command,
            result: StoreResult::ReceiptRecorded {
                evidence_id: evidence.clone(),
                evidence_revision: SqlU64::new(1).unwrap(),
                settled: false,
                conflict: false,
            },
        };
        let seq = SqlU64::new(number).unwrap();
        let digest = journal_entry(&previous, seq, &event).unwrap();
        snapshot.journal.push(
            JournalRecord::new(JournalRecordData {
                seq,
                previous,
                event,
                digest: digest.clone(),
            })
            .unwrap(),
        );
        previous = digest;
    }
    snapshot.projection.replay_head = previous;
    let id = snapshot.projection.task.data().task_id.clone();
    assert!(
        matches!(verify(&mut snapshot, &id), Err(StoreError::Operational(reason)) if reason.as_str() == "audit resource limit exceeded")
    );
}
