//! Transactional application of authoritative commands.
use super::{SqliteStore, operational};
use metron_protocol::domain::*;
use metron_protocol::store::*;
use metron_protocol::types::*;
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

macro_rules! bytes {
    ($value:expr) => {
        canonical_bytes($value, MAX_INTERNAL).map_err(StoreError::Validation)?
    };
}
macro_rules! decode {
    ($value:expr, $ty:ty) => {
        parse_internal::<$ty>($value).map_err(|_| StoreError::Integrity)?
    };
}

fn sql_error(error: rusqlite::Error) -> StoreError {
    if matches!(&error, rusqlite::Error::SqliteFailure(code, _) if matches!(code.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked))
    {
        StoreError::Busy
    } else {
        operational(error)
    }
}
fn bounded_blob(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Vec<u8>> {
    let value = row.get_ref(index)?.as_blob()?;
    if value.len() > MAX_INTERNAL {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Blob,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "oversized stored payload",
            )),
        ));
    }
    Ok(value.to_vec())
}
fn read_blob(tx: &Transaction<'_>, sql: &str, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
    tx.query_row(sql, [key], |r| bounded_blob(r, 0))
        .optional()
        .map_err(sql_error)
}
fn tick(tx: &Transaction<'_>) -> Result<SqlU64, StoreError> {
    let n: Option<i64> = tx
        .query_row("SELECT value FROM metadata WHERE key='tick'", [], |r| {
            r.get(0)
        })
        .optional()
        .map_err(sql_error)?;
    SqlU64::new(
        n.unwrap_or(0)
            .try_into()
            .map_err(|_| StoreError::Integrity)?,
    )
    .map_err(StoreError::Validation)
}

impl SqliteStore {
    /// Apply one command atomically; authority must only follow a fresh admission acknowledgement.
    pub fn apply_command(
        &mut self,
        command: StoreCommand,
    ) -> Result<WriteOutcome<StoreResult>, StoreError> {
        if self.read_only {
            return Err(operational("read-only store"));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let digest = command.digest().map_err(StoreError::Validation)?;
        let prior: Option<String> = tx
            .query_row(
                "SELECT digest FROM commands WHERE command_id=?1",
                [command.data().command_id.as_str()],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql_error)?;
        if let Some(old) = prior {
            if old != digest.as_str() {
                return Err(StoreError::Integrity);
            }
            let raw = read_blob(
                &tx,
                "SELECT result FROM commands WHERE command_id=?1",
                command.data().command_id.as_str(),
            )?
            .ok_or(StoreError::Integrity)?;
            return Ok(WriteOutcome::Committed(StoreReply {
                result: decode!(&raw, StoreResult),
                disposition: Disposition::Duplicate,
            }));
        }
        let operation = &command.data().operation;
        let task_id = match operation {
            StoreOperation::CreateTask { task, .. } => Some(&task.data().task_id),
            StoreOperation::CommitTransition { task_id, .. }
            | StoreOperation::PrepareIntent { task_id, .. }
            | StoreOperation::Claim { task_id, .. }
            | StoreOperation::Deny { task_id, .. }
            | StoreOperation::CreateApprovalRequest { task_id, .. }
            | StoreOperation::RecordUncertainty { task_id, .. }
            | StoreOperation::ClaimReconciliation { task_id, .. }
            | StoreOperation::SettleReconciliation { task_id, .. }
            | StoreOperation::RecordVerification { task_id, .. }
            | StoreOperation::RecordCodingVerification { task_id, .. } => Some(task_id),
            StoreOperation::Admit { check } => Some(&check.prepared.action.task_id),
            StoreOperation::RecordReceipt { receipt, .. } => Some(&receipt.data().task_id),
            StoreOperation::AdvanceTick { .. }
            | StoreOperation::RecordApproval { .. }
            | StoreOperation::RevokeGrant { .. } => None,
        };
        let task_version = match operation {
            StoreOperation::CreateTask { task, .. } => Some(task.data().schema_version),
            StoreOperation::RecordApproval { request_id, .. } => {
                let mut statement = tx
                    .prepare("SELECT value FROM intents ORDER BY row_id")
                    .map_err(sql_error)?;
                let mut rows = statement.query([]).map_err(sql_error)?;
                let mut owner: Option<TaskId> = None;
                while let Some(row) = rows.next().map_err(sql_error)? {
                    let raw: Vec<u8> = bounded_blob(row, 0).map_err(sql_error)?;
                    let intent: IntentSnapshot = decode!(&raw, IntentSnapshot);
                    if intent
                        .approval
                        .as_ref()
                        .is_some_and(|approval| &approval.request_id == request_id)
                        && owner.replace(intent.intent.task_id).is_some()
                    {
                        return Err(StoreError::Integrity);
                    }
                }
                drop(rows);
                drop(statement);
                owner
                    .as_ref()
                    .map(|id| load_task(&tx, id).map(|task| task.data().schema_version))
                    .transpose()?
            }
            _ => task_id
                .map(|id| load_task(&tx, id).map(|task| task.data().schema_version))
                .transpose()?,
        };
        if task_version.is_some_and(|version| version != command.data().schema_version) {
            return Err(StoreError::Validation(ValidationError::Version));
        }
        let previously = match &command.data().operation {
            StoreOperation::CreateTask { task, genesis } => {
                if let Some(raw) = read_blob(
                    &tx,
                    "SELECT task FROM tasks WHERE task_id=?1",
                    task.data().task_id.as_str(),
                )? {
                    let prior: Task = decode!(&raw, Task);
                    let raw = read_blob(
                        &tx,
                        "SELECT genesis FROM tasks WHERE task_id=?1",
                        task.data().task_id.as_str(),
                    )?
                    .ok_or(StoreError::Integrity)?;
                    let old: Genesis = decode!(&raw, Genesis);
                    if prior != *task || old != *genesis {
                        return Err(StoreError::Integrity);
                    }
                    Some(StoreResult::TaskCreated {
                        genesis_digest: journal_genesis(task, genesis)
                            .map_err(StoreError::Validation)?,
                    })
                } else {
                    None
                }
            }
            StoreOperation::Admit { check } => {
                let intent = load_intent(&tx, &check.intent_id)?;
                if matches!(
                    intent.status,
                    DispatchStatus::Dispatched
                        | DispatchStatus::Uncertain
                        | DispatchStatus::Settled
                ) && intent.prepared.as_ref() == Some(&check.prepared)
                    && intent
                        .lease
                        .as_ref()
                        .is_some_and(|l| l.worker == check.worker && l.epoch == check.lease_epoch)
                    && intent
                        .reservation
                        .as_ref()
                        .is_some_and(|r| r.id == check.reservation_id)
                    && intent.dispatch_id.as_ref() == Some(&check.prepared.action.dispatch_id)
                {
                    Some(StoreResult::Admitted {
                        dispatch_id: check.prepared.action.dispatch_id.clone(),
                    })
                } else {
                    None
                }
            }
            StoreOperation::RecordVerification {
                task_id,
                selection,
                accepted,
                reason,
            } => prior_verification(&tx, task_id, selection, *accepted, reason)?,
            StoreOperation::RecordCodingVerification {
                task_id,
                selection,
                accepted,
                reason,
            } => prior_coding_verification(&tx, task_id, selection, *accepted, reason)?,
            StoreOperation::CreateApprovalRequest {
                task_id,
                request_id,
                intent_id,
                action_digest,
            } => {
                let intent = load_intent(&tx, intent_id)?;
                if &intent.intent.task_id != task_id {
                    return Err(StoreError::Integrity);
                }
                if let Some(old) = intent.approval {
                    if old.request_id == *request_id {
                        if old.action_digest != *action_digest {
                            return Err(StoreError::Integrity);
                        }
                        Some(StoreResult::ApprovalRequestCreated)
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            StoreOperation::RecordApproval {
                request_id,
                action_digest,
                decision,
            } => prior_approval(&tx, request_id, action_digest, *decision)?,
            StoreOperation::PrepareIntent { task_id, prepared } => {
                let raw = read_blob(
                    &tx,
                    "SELECT value FROM intents WHERE intent_id=?1",
                    prepared.intent_id.as_str(),
                )?
                .ok_or(StoreError::Integrity)?;
                let intent: IntentSnapshot = decode!(&raw, IntentSnapshot);
                if &intent.intent.task_id != task_id {
                    return Err(StoreError::Integrity);
                }
                match intent.prepared {
                    Some(old) if old == *prepared => Some(StoreResult::Prepared {
                        intent_id: prepared.intent_id.clone(),
                    }),
                    Some(_) => return Err(StoreError::Integrity),
                    None => None,
                }
            }
            StoreOperation::RecordReceipt { receipt, .. } => {
                let id = receipt.evidence_id().map_err(StoreError::Validation)?;
                if let Some(raw) = read_blob(
                    &tx,
                    "SELECT value FROM evidence WHERE evidence_id=?1",
                    id.as_str(),
                )? {
                    let evidence: EvidenceSnapshot = decode!(&raw, EvidenceSnapshot);
                    if evidence.receipt != *receipt {
                        return Err(StoreError::Integrity);
                    }
                    let intent = load_intent(&tx, &receipt.data().intent_id)?;
                    Some(StoreResult::ReceiptRecorded {
                        evidence_id: id,
                        evidence_revision: intent.evidence_revision,
                        settled: evidence.selected,
                        conflict: evidence.conflict,
                    })
                } else {
                    None
                }
            }
            StoreOperation::RecordUncertainty {
                task_id,
                intent_id,
                dispatch_id,
                lease_epoch,
                ..
            } => {
                let intent = load_intent(&tx, intent_id)?;
                if &intent.intent.task_id != task_id
                    || intent.dispatch_id.as_ref() != Some(dispatch_id)
                    || intent.lease_epoch != *lease_epoch
                {
                    None
                } else if intent.status == DispatchStatus::Uncertain {
                    Some(StoreResult::UncertaintyRecorded)
                } else {
                    None
                }
            }
            _ => None,
        };
        let disposition = if previously.is_some() {
            Disposition::PreviouslyCommitted
        } else {
            Disposition::FirstCommit
        };
        let last: Option<i64> = tx
            .query_row("SELECT MAX(seq) FROM journal", [], |r| r.get(0))
            .map_err(sql_error)?;
        let last = SqlU64::new(
            last.unwrap_or(0)
                .try_into()
                .map_err(|_| StoreError::Integrity)?,
        )
        .map_err(StoreError::Validation)?;
        let seq = last.checked_add(1).map_err(StoreError::Validation)?;
        let seq_i = seq.get() as i64;
        let result = if let Some(result) = previously {
            result
        } else {
            match &command.data().operation {
                StoreOperation::CreateTask { task, genesis } => {
                    create_task(&tx, task, genesis, seq)?
                }
                StoreOperation::CommitTransition {
                    task_id,
                    expected_revision,
                    inbox_id,
                    inbox_digest,
                    transition,
                } => {
                    match commit_transition(
                        &tx,
                        task_id,
                        *expected_revision,
                        inbox_id,
                        inbox_digest,
                        transition,
                    )? {
                        Some(result) => result,
                        None => {
                            return Ok(WriteOutcome::NotCommitted(NotCommitted::RevisionConflict));
                        }
                    }
                }
                StoreOperation::PrepareIntent { task_id, prepared } => {
                    prepare(&tx, task_id, prepared)?
                }
                StoreOperation::Claim {
                    task_id,
                    intent_id,
                    worker,
                    expected_epoch,
                } => match claim(&tx, task_id, intent_id, worker, *expected_epoch)? {
                    Ok(result) => result,
                    Err(reason) => return Ok(WriteOutcome::NotCommitted(reason)),
                },
                StoreOperation::Admit { check } => match admit(&tx, check, seq)? {
                    Ok(result) => result,
                    Err(reason) => return Ok(WriteOutcome::NotCommitted(reason)),
                },
                StoreOperation::RecordReceipt {
                    receipt,
                    lease_epoch,
                } => record_receipt(&tx, receipt, *lease_epoch, seq, false, true)?,
                StoreOperation::Deny {
                    task_id,
                    intent_id,
                    reason,
                } => deny(&tx, task_id, intent_id, reason, seq)?,
                StoreOperation::CreateApprovalRequest {
                    task_id,
                    request_id,
                    intent_id,
                    action_digest,
                } => create_approval(&tx, task_id, request_id, intent_id, action_digest)?,
                StoreOperation::RecordApproval {
                    request_id,
                    action_digest,
                    decision,
                } => match record_approval(&tx, request_id, action_digest, *decision)? {
                    Some(result) => result,
                    None => return Ok(WriteOutcome::NotCommitted(NotCommitted::RecheckAuthority)),
                },
                StoreOperation::RevokeGrant {
                    principal_id,
                    expected_revision,
                } => match revoke(&tx, principal_id, *expected_revision)? {
                    Some(result) => result,
                    None => return Ok(WriteOutcome::NotCommitted(NotCommitted::RevisionConflict)),
                },
                StoreOperation::RecordUncertainty {
                    task_id,
                    intent_id,
                    dispatch_id,
                    lease_epoch,
                    reason: _,
                } => match uncertainty(&tx, task_id, intent_id, dispatch_id, *lease_epoch, seq)? {
                    Some(result) => result,
                    None => return Ok(WriteOutcome::NotCommitted(NotCommitted::LeaseConflict)),
                },
                StoreOperation::ClaimReconciliation {
                    task_id,
                    intent_id,
                    worker,
                    expected_epoch,
                    expected_evidence_revision,
                } => match claim_reconciliation(
                    &tx,
                    task_id,
                    intent_id,
                    worker,
                    *expected_epoch,
                    *expected_evidence_revision,
                )? {
                    Ok(value) => value,
                    Err(reason) => return Ok(WriteOutcome::NotCommitted(reason)),
                },
                StoreOperation::SettleReconciliation {
                    task_id,
                    intent_id,
                    worker,
                    lease_epoch,
                    expected_evidence_revision,
                    evidence_id,
                    reconciled_receipt,
                } => match settle_reconciliation(
                    &tx,
                    task_id,
                    intent_id,
                    worker,
                    *lease_epoch,
                    *expected_evidence_revision,
                    evidence_id,
                    reconciled_receipt.as_ref(),
                    seq,
                )? {
                    Ok(value) => value,
                    Err(reason) => return Ok(WriteOutcome::NotCommitted(reason)),
                },
                StoreOperation::RecordVerification {
                    task_id,
                    selection,
                    accepted,
                    reason,
                } => match verify(&tx, task_id, selection, *accepted, reason, seq)? {
                    Some(result) => result,
                    None => return Ok(WriteOutcome::NotCommitted(NotCommitted::EvidenceChanged)),
                },
                StoreOperation::RecordCodingVerification {
                    task_id,
                    selection,
                    accepted,
                    reason,
                } => match verify_coding(&tx, task_id, selection, *accepted, reason, seq)? {
                    Some(result) => result,
                    None => return Ok(WriteOutcome::NotCommitted(NotCommitted::EvidenceChanged)),
                },
                StoreOperation::AdvanceTick {
                    expected_tick,
                    new_tick,
                } => {
                    if tick(&tx)? != *expected_tick {
                        return Ok(WriteOutcome::NotCommitted(NotCommitted::RevisionConflict));
                    }
                    if new_tick <= expected_tick {
                        return Err(StoreError::Validation(ValidationError::Binding));
                    }
                    tx.execute("INSERT INTO metadata(key,value) VALUES('tick',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [new_tick.get() as i64]).map_err(sql_error)?;
                    StoreResult::TickAdvanced { tick: *new_tick }
                }
            }
        };
        let previous: Digest =
            match read_blob(&tx, "SELECT value FROM metadata WHERE key=?1", "head")? {
                Some(head) => decode!(&head, Digest),
                None => match &command.data().operation {
                    StoreOperation::CreateTask { task, genesis } => {
                        journal_genesis(task, genesis).map_err(StoreError::Validation)?
                    }
                    _ => return Err(StoreError::Integrity),
                },
            };
        let event = JournalEvent::CommandCommitted {
            command: command.clone(),
            result: result.clone(),
        };
        let head = journal_entry(&previous, seq, &event).map_err(StoreError::Validation)?;
        let record = JournalRecord::new(JournalRecordData {
            seq,
            previous,
            event,
            digest: head.clone(),
        })
        .map_err(StoreError::Validation)?;
        tx.execute(
            "INSERT INTO journal(seq,payload) VALUES(?1,?2)",
            params![seq_i, bytes!(&record)],
        )
        .map_err(sql_error)?;
        tx.execute(
            "INSERT INTO commands(command_id,digest,result) VALUES(?1,?2,?3)",
            params![
                command.data().command_id.as_str(),
                digest.as_str(),
                bytes!(&result)
            ],
        )
        .map_err(sql_error)?;
        tx.execute("INSERT INTO metadata(key,value) VALUES('head',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [bytes!(&head)]).map_err(sql_error)?;
        tx.commit().map_err(operational)?;
        Ok(WriteOutcome::Committed(StoreReply {
            result,
            disposition,
        }))
    }
}

fn create_task(
    tx: &Transaction<'_>,
    task: &Task,
    genesis: &Genesis,
    seq: SqlU64,
) -> Result<StoreResult, StoreError> {
    let data = task.data();
    if read_blob(
        tx,
        "SELECT task FROM tasks WHERE task_id=?1",
        data.task_id.as_str(),
    )?
    .is_some()
    {
        return Err(StoreError::Integrity);
    }
    let digest = journal_genesis(task, genesis).map_err(StoreError::Validation)?;
    let zero = SqlU64::new(0).map_err(StoreError::Validation)?;
    let counters = Counters {
        model_requests: zero,
        tool_calls: zero,
        spend_micros_usd: zero,
    };
    let limit = Counters {
        model_requests: data.budget.max_model_requests,
        tool_calls: data.budget.max_tool_calls,
        spend_micros_usd: data.budget.max_spend_micros_usd,
    };
    let account = AccountSnapshot {
        limit,
        posted: counters.clone(),
        reserved: counters,
        breached: false,
        incomplete: false,
        integrity_hold: false,
    };
    let grant = GrantSnapshot {
        principal_id: data.principal_id.clone(),
        revision: zero,
        revoked: false,
    };
    let state = AgentState {
        coding_context: None,
        task_id: data.task_id.clone(),
        genesis_digest: digest.clone(),
        max_decision_steps: data.max_decision_steps,
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
    tx.execute(
        "INSERT INTO tasks(task_id,task,genesis,state,verification) VALUES(?1,?2,?3,?4,?5)",
        params![
            data.task_id.as_str(),
            bytes!(task),
            bytes!(genesis),
            bytes!(&state),
            bytes!(&verification)
        ],
    )
    .map_err(sql_error)?;
    tx.execute(
        "INSERT OR IGNORE INTO principals(principal_id,grant,account) VALUES(?1,?2,?3)",
        params![data.principal_id.as_str(), bytes!(&grant), bytes!(&account)],
    )
    .map_err(sql_error)?;
    tx.execute(
        "INSERT INTO task_accounts(task_id,account) VALUES(?1,?2)",
        params![data.task_id.as_str(), bytes!(&account)],
    )
    .map_err(sql_error)?;
    let mut input = InboxInput {
        id: inbox_id(&data.task_id, seq, 0).map_err(StoreError::Validation)?,
        task_id: data.task_id.clone(),
        principal_id: data.principal_id.clone(),
        source_seq: seq,
        output_ordinal: 0,
        source: InboxSource::TaskAccepted,
        evidence_id: None,
        event: AgentEvent::TaskAccepted,
        digest: content_digest(&[]),
    };
    input.digest = inbox_digest(&input).map_err(StoreError::Validation)?;
    let item = InboxSnapshot {
        input,
        status: InboxStatus::Pending,
        transition: None,
    };
    tx.execute(
        "INSERT INTO inbox(task_id,inbox_id,value) VALUES(?1,?2,?3)",
        params![data.task_id.as_str(), item.input.id.as_str(), bytes!(&item)],
    )
    .map_err(sql_error)?;
    Ok(StoreResult::TaskCreated {
        genesis_digest: digest,
    })
}

/// Coherent SQLite read transaction used for all audit and runtime views.
pub struct Snapshot<'a> {
    tx: Transaction<'a>,
}

impl StorePort for SqliteStore {
    type Audit<'a> = Snapshot<'a>;
    fn apply(&mut self, command: StoreCommand) -> Result<WriteOutcome<StoreResult>, StoreError> {
        self.apply_command(command)
    }
    fn audit(&mut self) -> Result<Self::Audit<'_>, StoreError> {
        let read_only = self.read_only;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(sql_error)?;
        // Establish the SQLite snapshot now, not on the first later page.
        tx.query_row("SELECT COUNT(*) FROM sqlite_schema", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(sql_error)?;
        if read_only {
            validate_representation(&tx)?;
        }
        Ok(Snapshot { tx })
    }
}

macro_rules! snapshot_page {
    ($self:expr, $sql:expr, $key:expr, $after:expr, $table:literal, $column:literal, $ty:ty) => {{
        let after = $after.map_or(0, |v| v.get());
        let mut statement = $self.tx.prepare($sql).map_err(sql_error)?;
        let mut rows = statement
            .query(params![$key, after as i64])
            .map_err(sql_error)?;
        let mut ids = Vec::new();
        let mut total = 0_usize;
        let mut more = false;
        while let Some(row) = rows.next().map_err(sql_error)? {
            let id: i64 = row.get(0).map_err(sql_error)?;
            let len: i64 = row.get(1).map_err(sql_error)?;
            let len: usize = len.try_into().map_err(|_| StoreError::Integrity)?;
            if len > MAX_INTERNAL {
                return Err(StoreError::Integrity);
            }
            if ids.len() == MAX_PAGE_RECORDS || total + len > MAX_PAGE_BYTES {
                more = true;
                break;
            }
            total += len;
            ids.push(id);
        }
        drop(rows);
        drop(statement);
        let mut records = Vec::with_capacity(ids.len());
        for id in &ids {
            let raw: Vec<u8> = $self
                .tx
                .query_row(
                    concat!("SELECT ", $column, " FROM ", $table, " WHERE row_id=?1"),
                    [id],
                    |r| bounded_blob(r, 0),
                )
                .map_err(sql_error)?;
            records.push(decode!(&raw, $ty));
        }
        let next = if more {
            Some(
                SqlU64::new(
                    (*ids.last().ok_or(StoreError::Integrity)?)
                        .try_into()
                        .map_err(|_| StoreError::Integrity)?,
                )
                .map_err(StoreError::Validation)?,
            )
        } else {
            None
        };
        SnapshotPage { records, next }
    }};
}

impl AuditSnapshot for Snapshot<'_> {
    fn projection(&mut self, task: &TaskId) -> Result<Projection, StoreError> {
        let (task_blob, genesis, state, verification): (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) = self
            .tx
            .query_row(
                "SELECT task,genesis,state,verification FROM tasks WHERE task_id=?1",
                [task.as_str()],
                |r| {
                    Ok((
                        bounded_blob(r, 0)?,
                        bounded_blob(r, 1)?,
                        bounded_blob(r, 2)?,
                        bounded_blob(r, 3)?,
                    ))
                },
            )
            .map_err(sql_error)?;
        let decoded_task = decode!(&task_blob, Task);
        if decoded_task.data().task_id != *task {
            return Err(StoreError::Integrity);
        }
        let task = decoded_task;
        let account = read_blob(
            &self.tx,
            "SELECT account FROM task_accounts WHERE task_id=?1",
            task.data().task_id.as_str(),
        )?
        .ok_or(StoreError::Integrity)?;
        let principal = read_blob(
            &self.tx,
            "SELECT account FROM principals WHERE principal_id=?1",
            task.data().principal_id.as_str(),
        )?
        .ok_or(StoreError::Integrity)?;
        let head = read_blob(&self.tx, "SELECT value FROM metadata WHERE key=?1", "head")?
            .ok_or(StoreError::Integrity)?;
        Ok(Projection {
            genesis: decode!(&genesis, Genesis),
            task,
            state: decode!(&state, AgentState),
            tick: tick(&self.tx)?,
            task_account: decode!(&account, AccountSnapshot),
            principal_account: decode!(&principal, AccountSnapshot),
            verification: decode!(&verification, VerificationSnapshot),
            replay_head: decode!(&head, Digest),
        })
    }
    fn journal_page(&mut self, after: Option<SqlU64>) -> Result<JournalPage, StoreError> {
        let start = after.map_or(0, |v| v.get());
        let mut statement = self
            .tx
            .prepare("SELECT seq,length(payload) FROM journal WHERE seq>?1 ORDER BY seq LIMIT 17")
            .map_err(sql_error)?;
        let mut rows = statement.query([start as i64]).map_err(sql_error)?;
        let mut ids = Vec::new();
        let mut size = 0_usize;
        let mut more = false;
        while let Some(row) = rows.next().map_err(sql_error)? {
            let id: i64 = row.get(0).map_err(sql_error)?;
            let len: usize = row
                .get::<_, i64>(1)
                .map_err(sql_error)?
                .try_into()
                .map_err(|_| StoreError::Integrity)?;
            if len > MAX_INTERNAL {
                return Err(StoreError::Integrity);
            }
            if ids.len() == MAX_PAGE_RECORDS || size + len > MAX_PAGE_BYTES {
                more = true;
                break;
            }
            ids.push(id);
            size += len;
        }
        drop(rows);
        drop(statement);
        let mut records = Vec::with_capacity(ids.len());
        for id in &ids {
            let raw: Vec<u8> = self
                .tx
                .query_row("SELECT payload FROM journal WHERE seq=?1", [id], |r| {
                    bounded_blob(r, 0)
                })
                .map_err(sql_error)?;
            records.push(decode!(&raw, JournalRecord));
        }
        let next = if more {
            Some(
                SqlU64::new(
                    (*ids.last().ok_or(StoreError::Integrity)?)
                        .try_into()
                        .map_err(|_| StoreError::Integrity)?,
                )
                .map_err(StoreError::Validation)?,
            )
        } else {
            None
        };
        Ok(JournalPage { records, next })
    }
    fn pending_inbox(&mut self, task: &TaskId) -> Result<Option<InboxInput>, StoreError> {
        let mut cursor = None;
        loop {
            let page = self.inbox(task, cursor)?;
            for entry in page.records {
                if entry.status == InboxStatus::Pending {
                    return Ok(Some(entry.input));
                }
            }
            match page.next {
                Some(next) => cursor = Some(next),
                None => return Ok(None),
            }
        }
    }
}
impl RuntimeSnapshot for Snapshot<'_> {
    fn intent(&mut self, id: &Digest) -> Result<Option<IntentSnapshot>, StoreError> {
        let blob = read_blob(
            &self.tx,
            "SELECT value FROM intents WHERE intent_id=?1",
            id.as_str(),
        )?;
        let value: Option<IntentSnapshot> = blob
            .map(|b| parse_internal(&b).map_err(|_| StoreError::Integrity))
            .transpose()?;
        if let Some(item) = &value {
            check_intent_key(&self.tx, item)?;
            if &item.intent_id != id {
                return Err(StoreError::Integrity);
            }
        }
        Ok(value)
    }
    fn intents(
        &mut self,
        task: &TaskId,
        after: Option<SqlU64>,
    ) -> Result<SnapshotPage<IntentSnapshot>, StoreError> {
        let page = snapshot_page!(
            self,
            "SELECT row_id,length(value) FROM intents WHERE task_id=?1 AND row_id>?2 ORDER BY row_id LIMIT 17",
            task.as_str(),
            after,
            "intents",
            "value",
            IntentSnapshot
        );
        for item in &page.records {
            check_intent_key(&self.tx, item)?;
        }
        Ok(page)
    }
    fn grant(&mut self, principal: &PrincipalId) -> Result<GrantSnapshot, StoreError> {
        let raw = read_blob(
            &self.tx,
            "SELECT grant FROM principals WHERE principal_id=?1",
            principal.as_str(),
        )?
        .ok_or(StoreError::Integrity)?;
        Ok(decode!(&raw, GrantSnapshot))
    }
    fn evidence(
        &mut self,
        intent: &Digest,
        after: Option<SqlU64>,
    ) -> Result<SnapshotPage<EvidenceSnapshot>, StoreError> {
        let page = snapshot_page!(
            self,
            "SELECT row_id,length(value) FROM evidence WHERE intent_id=?1 AND row_id>?2 ORDER BY row_id LIMIT 17",
            intent.as_str(),
            after,
            "evidence",
            "value",
            EvidenceSnapshot
        );
        for item in &page.records {
            check_evidence_key(&self.tx, item)?;
        }
        Ok(page)
    }
    fn inbox(
        &mut self,
        task: &TaskId,
        after: Option<SqlU64>,
    ) -> Result<SnapshotPage<InboxSnapshot>, StoreError> {
        let page = snapshot_page!(
            self,
            "SELECT row_id,length(value) FROM inbox WHERE task_id=?1 AND row_id>?2 ORDER BY row_id LIMIT 17",
            task.as_str(),
            after,
            "inbox",
            "value",
            InboxSnapshot
        );
        for item in &page.records {
            check_inbox_key(&self.tx, item)?;
        }
        Ok(page)
    }
    fn unposted_debits(
        &mut self,
        task: &TaskId,
        after: Option<SqlU64>,
    ) -> Result<SnapshotPage<UnpostedDebit>, StoreError> {
        Ok(snapshot_page!(
            self,
            "SELECT row_id,length(value) FROM unposted_debits WHERE task_id=?1 AND row_id>?2 ORDER BY row_id LIMIT 17",
            task.as_str(),
            after,
            "unposted_debits",
            "value",
            UnpostedDebit
        ))
    }
}

fn commit_transition(
    tx: &Transaction<'_>,
    task_id: &TaskId,
    revision: SqlU64,
    input_id: &Digest,
    expected_digest: &Digest,
    transition: &Transition,
) -> Result<Option<StoreResult>, StoreError> {
    let raw = read_blob(
        tx,
        "SELECT state FROM tasks WHERE task_id=?1",
        task_id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    let state: AgentState = decode!(&raw, AgentState);
    if state.revision != revision {
        return Ok(None);
    }
    let mut statement = tx
        .prepare("SELECT row_id,value FROM inbox WHERE task_id=?1 ORDER BY row_id")
        .map_err(sql_error)?;
    let mut rows = statement.query([task_id.as_str()]).map_err(sql_error)?;
    let mut pending = None;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let value: Vec<u8> = bounded_blob(row, 1).map_err(sql_error)?;
        let item: InboxSnapshot = decode!(&value, InboxSnapshot);
        if item.status == InboxStatus::Pending {
            pending = Some((row.get::<_, i64>(0).map_err(sql_error)?, item));
            break;
        }
    }
    drop(rows);
    drop(statement);
    let Some((row_id, mut item)) = pending else {
        return Ok(None);
    };
    if &item.input.id != input_id || &item.input.digest != expected_digest {
        return Ok(None);
    }
    if inbox_digest(&item.input).map_err(StoreError::Validation)? != item.input.digest
        || inbox_id(task_id, item.input.source_seq, item.input.output_ordinal)
            .map_err(StoreError::Validation)?
            != item.input.id
    {
        return Err(StoreError::Integrity);
    }
    validate_input_source(tx, &item.input)?;
    transition
        .validate_from(&state)
        .map_err(StoreError::Validation)?;
    let raw = read_blob(
        tx,
        "SELECT task FROM tasks WHERE task_id=?1",
        task_id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    let task: Task = decode!(&raw, Task);
    if item.input.principal_id != task.data().principal_id {
        return Err(StoreError::Integrity);
    }
    if let AgentEvent::EffectSettled {
        output: ReceiptOutput::Fixture { bytes, digest },
        ..
    } = &item.input.event
        && (content_digest(bytes.as_str().as_bytes()) != *digest
            || parse_sum_input(bytes.as_str().as_bytes()).is_err())
    {
        return Err(StoreError::Integrity);
    }
    let (next, new_intent) = match transition {
        Transition::Decision { next, intent } => (next, Some(intent)),
        Transition::Finalization { next } => (next, None),
    };
    if new_intent.is_some_and(|intent| intent.schema_version != task.data().schema_version) {
        return Err(StoreError::Validation(ValidationError::Version));
    }
    let valid = if let Some(coding) = task.coding() {
        let genesis = load_genesis(tx, task_id)?;
        let policy = genesis
            .coding_policy
            .as_ref()
            .ok_or(StoreError::Integrity)?;
        let controller = match genesis.engine_version.as_str() {
            "coding-agent/v3" | "coding-harness/v3" => {
                metron_protocol::coding_runtime::coding_transition
            }
            "coding-agent/v3.1" | "coding-harness/v3.1" => {
                metron_protocol::coding_runtime::coding_transition_phased
            }
            "coding-agent/v3.2" | "coding-harness/v3.2" => {
                metron_protocol::coding_runtime::coding_transition_catalog
            }
            "coding-agent/v3.3" | "coding-harness/v3.3" => {
                metron_protocol::coding_runtime::coding_transition_focused
            }
            "coding-agent/v3.4" | "coding-harness/v3.4" => {
                metron_protocol::coding_runtime::coding_transition_native
            }
            "coding-agent/v3.5" | "coding-harness/v3.5" => {
                metron_protocol::coding_runtime::coding_transition_recovering
            }
            "coding-agent/v3.6" | "coding-harness/v3.6" => {
                metron_protocol::coding_runtime::coding_transition_constrained
            }
            "coding-agent/v3.7" | "coding-harness/v3.7" => {
                metron_protocol::coding_runtime::coding_transition_format_neutral
            }
            "coding-agent/v3.9" | "coding-harness/v3.9" => {
                metron_protocol::coding_runtime::coding_transition_paired
            }
            "coding-agent/v3.8" | "coding-harness/v3.8" => {
                metron_protocol::coding_runtime::coding_transition_memory
            }
            _ => return Err(StoreError::Integrity),
        };
        controller(coding, policy, &state, &item.input.event).as_ref() == Ok(transition)
    } else {
        state.coding_context.is_none()
            && next.coding_context.is_none()
            && match (&state.phase, &item.input.event, transition) {
                (
                    Phase::New,
                    AgentEvent::TaskAccepted,
                    Transition::Decision {
                        intent:
                            Intent {
                                kind: IntentKind::ReadFixture,
                                ..
                            },
                        next,
                    },
                ) => next.phase == Phase::WaitingRead,
                (
                    Phase::WaitingRead,
                    AgentEvent::EffectSettled {
                        intent_id,
                        output: ReceiptOutput::Fixture { bytes, digest },
                        ..
                    },
                    Transition::Decision {
                        intent:
                            Intent {
                                kind:
                                    IntentKind::RequestModel {
                                        input_digest,
                                        context,
                                    },
                                ..
                            },
                        next,
                    },
                ) => {
                    state.pending_intent.as_ref() == Some(intent_id)
                        && next.phase == Phase::WaitingModel
                        && input_digest == digest
                        && context.as_str() == bytes.as_str()
                }
                (
                    Phase::WaitingRead,
                    AgentEvent::EffectSettled {
                        intent_id,
                        output: ReceiptOutput::Fixture { bytes, digest },
                        ..
                    },
                    Transition::Decision {
                        intent:
                            Intent {
                                kind:
                                    IntentKind::RequestLocalModel {
                                        input_digest,
                                        context,
                                        ..
                                    },
                                ..
                            },
                        next,
                    },
                ) => {
                    state.pending_intent.as_ref() == Some(intent_id)
                        && next.phase == Phase::WaitingModel
                        && input_digest == digest
                        && context.as_str() == bytes.as_str()
                }
                (
                    Phase::WaitingModel,
                    AgentEvent::EffectSettled {
                        intent_id,
                        output:
                            ReceiptOutput::Model {
                                candidate: observed,
                            },
                        ..
                    },
                    Transition::Decision {
                        intent:
                            Intent {
                                kind: IntentKind::PublishArtifact { candidate },
                                ..
                            },
                        next,
                    },
                ) => {
                    state.pending_intent.as_ref() == Some(intent_id)
                        && next.phase == Phase::WaitingArtifact
                        && candidate == observed
                }
                (
                    Phase::WaitingModel,
                    AgentEvent::EffectSettled {
                        intent_id,
                        output: ReceiptOutput::LocalModel { observation },
                        ..
                    },
                    Transition::Decision {
                        intent:
                            Intent {
                                kind: IntentKind::PublishArtifact { candidate },
                                ..
                            },
                        next,
                    },
                ) => {
                    state.pending_intent.as_ref() == Some(intent_id)
                        && next.phase == Phase::WaitingArtifact
                        && matches!(&observation.response, Some(metron_protocol::LocalResponse::Sum(sum)) if candidate.count == sum.count && candidate.sum == sum.sum && candidate.input_digest == observation.input_digest)
                }
                (
                    Phase::WaitingModel,
                    AgentEvent::EffectSettled {
                        intent_id,
                        output: ReceiptOutput::LocalModel { observation },
                        ..
                    },
                    Transition::Finalization { next },
                ) => {
                    state.pending_intent.as_ref() == Some(intent_id)
                        && next.phase == Phase::Rejected
                        && next.pending_intent.is_none()
                        && !matches!(
                            observation.response,
                            Some(metron_protocol::LocalResponse::Sum(_))
                        )
                }
                (
                    Phase::WaitingArtifact,
                    AgentEvent::EffectSettled {
                        intent_id,
                        output: ReceiptOutput::Published { .. },
                        ..
                    },
                    Transition::Finalization { next },
                ) => {
                    state.pending_intent.as_ref() == Some(intent_id)
                        && next.phase == Phase::AwaitingVerification
                        && next.pending_intent.is_none()
                }
                (
                    _,
                    AgentEvent::EffectUncertain { intent_id },
                    Transition::Finalization { next },
                ) => {
                    state.pending_intent.as_ref() == Some(intent_id)
                        && next.phase == state.phase
                        && next.pending_intent == state.pending_intent
                }
                (_, AgentEvent::Denied { intent_id, .. }, Transition::Finalization { next }) => {
                    state.pending_intent.as_ref() == Some(intent_id)
                        && next.phase == Phase::Rejected
                        && next.pending_intent.is_none()
                }
                (
                    _,
                    AgentEvent::EffectSettled {
                        intent_id,
                        output: ReceiptOutput::Failure { .. },
                        ..
                    },
                    Transition::Finalization { next },
                ) => {
                    state.pending_intent.as_ref() == Some(intent_id)
                        && next.phase == Phase::Rejected
                }
                (
                    Phase::AwaitingVerification,
                    AgentEvent::Verified { accepted, .. },
                    Transition::Finalization { next },
                ) => {
                    next.phase
                        == if *accepted {
                            Phase::Completed
                        } else {
                            Phase::Rejected
                        }
                }
                _ => false,
            }
    };
    if !valid {
        return Err(StoreError::Integrity);
    }
    let intent_id = if let Some(intent) = new_intent {
        let prepared =
            PreparedIntent::materialize(intent, &task.data().principal_id, &state.genesis_digest)
                .map_err(StoreError::Validation)?;
        prepared
            .validate_against_genesis(
                intent,
                &task.data().principal_id,
                &state.genesis_digest,
                &load_genesis(tx, task_id)?,
            )
            .map_err(StoreError::Validation)?;
        if next.pending_intent.as_ref() != Some(&prepared.intent_id)
            || intent.ordinal != u32::from(state.decision_steps)
        {
            return Err(StoreError::Integrity);
        }
        let id = prepared.intent_id;
        let zero = SqlU64::new(0).map_err(StoreError::Validation)?;
        let snapshot = IntentSnapshot {
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
        tx.execute(
            "INSERT INTO intents(task_id,intent_id,value) VALUES(?1,?2,?3)",
            params![task_id.as_str(), id.as_str(), bytes!(&snapshot)],
        )
        .map_err(sql_error)?;
        Some(id)
    } else {
        None
    };
    item.status = InboxStatus::Consumed;
    item.transition = Some(transition.clone());
    tx.execute(
        "UPDATE inbox SET value=?1 WHERE row_id=?2",
        params![bytes!(&item), row_id],
    )
    .map_err(sql_error)?;
    tx.execute(
        "UPDATE tasks SET state=?1 WHERE task_id=?2",
        params![bytes!(next), task_id.as_str()],
    )
    .map_err(sql_error)?;
    Ok(Some(StoreResult::TransitionCommitted {
        revision: next.revision,
        intent_id,
    }))
}

fn load_intent(tx: &Transaction<'_>, id: &Digest) -> Result<IntentSnapshot, StoreError> {
    let value = read_blob(
        tx,
        "SELECT value FROM intents WHERE intent_id=?1",
        id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    let intent: IntentSnapshot = decode!(&value, IntentSnapshot);
    if &intent.intent_id != id {
        return Err(StoreError::Integrity);
    }
    Ok(intent)
}
fn save_intent(tx: &Transaction<'_>, intent: &IntentSnapshot) -> Result<(), StoreError> {
    tx.execute(
        "UPDATE intents SET value=?1 WHERE intent_id=?2",
        params![bytes!(intent), intent.intent_id.as_str()],
    )
    .map_err(sql_error)?;
    Ok(())
}
fn load_task(tx: &Transaction<'_>, id: &TaskId) -> Result<Task, StoreError> {
    let value = read_blob(tx, "SELECT task FROM tasks WHERE task_id=?1", id.as_str())?
        .ok_or(StoreError::Integrity)?;
    Ok(decode!(&value, Task))
}
fn load_genesis(tx: &Transaction<'_>, id: &TaskId) -> Result<Genesis, StoreError> {
    let value = read_blob(
        tx,
        "SELECT genesis FROM tasks WHERE task_id=?1",
        id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    Ok(decode!(&value, Genesis))
}
fn prepare(
    tx: &Transaction<'_>,
    task_id: &TaskId,
    prepared: &PreparedIntent,
) -> Result<StoreResult, StoreError> {
    let mut intent = load_intent(tx, &prepared.intent_id)?;
    if &intent.intent.task_id != task_id {
        return Err(StoreError::Integrity);
    }
    let task = load_task(tx, task_id)?;
    let state = read_blob(
        tx,
        "SELECT state FROM tasks WHERE task_id=?1",
        task_id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    let state: AgentState = decode!(&state, AgentState);
    prepared
        .validate_against_genesis(
            &intent.intent,
            &task.data().principal_id,
            &state.genesis_digest,
            &load_genesis(tx, task_id)?,
        )
        .map_err(StoreError::Validation)?;
    if let Some(old) = &intent.prepared {
        if old != prepared {
            return Err(StoreError::Integrity);
        }
    } else {
        intent.prepared = Some(prepared.clone());
        save_intent(tx, &intent)?;
    }
    Ok(StoreResult::Prepared {
        intent_id: prepared.intent_id.clone(),
    })
}
fn claim(
    tx: &Transaction<'_>,
    task_id: &TaskId,
    id: &Digest,
    worker: &WorkerId,
    expected: SqlU64,
) -> Result<Result<StoreResult, NotCommitted>, StoreError> {
    let mut intent = load_intent(tx, id)?;
    let current = tick(tx)?;
    if &intent.intent.task_id != task_id {
        return Err(StoreError::Integrity);
    }
    if intent.lease_epoch != expected {
        return Ok(Err(NotCommitted::LeaseConflict));
    }
    if intent.prepared.is_none() {
        return Ok(Err(NotCommitted::NoEligible));
    }
    let eligible = match intent.status {
        DispatchStatus::Ready => true,
        DispatchStatus::Leased => intent
            .lease
            .as_ref()
            .is_some_and(|lease| lease.expires_at <= current),
        _ => false,
    };
    if !eligible {
        return Ok(Err(if intent.status == DispatchStatus::Leased {
            NotCommitted::LeaseConflict
        } else {
            NotCommitted::NoEligible
        }));
    }
    let epoch = expected.checked_add(1).map_err(StoreError::Validation)?;
    let expires_at = current.checked_add(30).map_err(StoreError::Validation)?;
    intent.lease_epoch = epoch;
    intent.lease = Some(LeaseSnapshot {
        worker: worker.clone(),
        epoch,
        expires_at,
    });
    intent.status = DispatchStatus::Leased;
    save_intent(tx, &intent)?;
    Ok(Ok(StoreResult::Claimed {
        lease_epoch: epoch,
        expires_at,
    }))
}
fn add_counters(a: &Counters, b: &Counters) -> Option<Counters> {
    Some(Counters {
        model_requests: a.model_requests.checked_add(b.model_requests.get()).ok()?,
        tool_calls: a.tool_calls.checked_add(b.tool_calls.get()).ok()?,
        spend_micros_usd: a
            .spend_micros_usd
            .checked_add(b.spend_micros_usd.get())
            .ok()?,
    })
}
fn within(a: &Counters, b: &Counters) -> bool {
    a.model_requests <= b.model_requests
        && a.tool_calls <= b.tool_calls
        && a.spend_micros_usd <= b.spend_micros_usd
}
fn admit(
    tx: &Transaction<'_>,
    check: &AdmissionCheck,
    seq: SqlU64,
) -> Result<Result<StoreResult, NotCommitted>, StoreError> {
    let mut intent = load_intent(tx, &check.intent_id)?;
    let now = tick(tx)?;
    if now != check.expected_tick {
        return Ok(Err(NotCommitted::RecheckAuthority));
    }
    if intent.status != DispatchStatus::Leased
        || !intent.lease.as_ref().is_some_and(|lease| {
            lease.worker == check.worker
                && lease.epoch == check.lease_epoch
                && lease.expires_at > now
        })
    {
        return Ok(Err(NotCommitted::LeaseConflict));
    }
    if intent.prepared.as_ref() != Some(&check.prepared) {
        return Err(StoreError::Integrity);
    }
    let task = load_task(tx, &intent.intent.task_id)?;
    let state = read_blob(
        tx,
        "SELECT state FROM tasks WHERE task_id=?1",
        task.data().task_id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    let state: AgentState = decode!(&state, AgentState);
    check
        .prepared
        .validate_against_genesis(
            &intent.intent,
            &task.data().principal_id,
            &state.genesis_digest,
            &load_genesis(tx, &task.data().task_id)?,
        )
        .map_err(StoreError::Validation)?;
    let grant = read_blob(
        tx,
        "SELECT grant FROM principals WHERE principal_id=?1",
        task.data().principal_id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    let grant: GrantSnapshot = decode!(&grant, GrantSnapshot);
    if grant.revision != check.grant_revision {
        return Ok(Err(NotCommitted::RecheckAuthority));
    }
    if grant.revoked {
        return Ok(Ok(deny(
            tx,
            &task.data().task_id,
            &check.intent_id,
            &BoundedText::new("grant revoked").map_err(StoreError::Validation)?,
            seq,
        )?));
    }
    let genesis = read_blob(
        tx,
        "SELECT genesis FROM tasks WHERE task_id=?1",
        task.data().task_id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    let genesis: Genesis = decode!(&genesis, Genesis);
    if let ApprovedRequest::Coding { step, .. } = &check.prepared.request {
        let policy = genesis
            .coding_policy
            .as_ref()
            .ok_or(StoreError::Integrity)?;
        if !step.permitted(policy) {
            return Ok(Ok(deny(
                tx,
                &task.data().task_id,
                &check.intent_id,
                &BoundedText::new("coding action outside grant").map_err(StoreError::Validation)?,
                seq,
            )?));
        }
    }
    if match &check.prepared.request {
        ApprovedRequest::Coding { step, .. } => genesis
            .coding_policy
            .as_ref()
            .is_some_and(|p| step.requires_approval(p)),
        ApprovedRequest::PublishArtifact { .. } => genesis.require_publication_approval,
        _ => false,
    } {
        let approved = intent.approval.as_ref().is_some_and(|approval| {
            Some(&approval.request_id) == check.approval_id.as_ref()
                && approval.action_digest == check.prepared.action_digest
                && approval.decision == Some(ApprovalDecision::Approve)
                && approval.expires_at > now
        });
        if !approved {
            return Ok(Err(NotCommitted::RecheckAuthority));
        }
    }
    let tariff = match &check.prepared.request {
        ApprovedRequest::ReadFixture { tariff, .. }
        | ApprovedRequest::RequestModel { tariff, .. }
        | ApprovedRequest::RequestLocalModel { tariff, .. }
        | ApprovedRequest::Coding { tariff, .. } => tariff,
        ApprovedRequest::PublishArtifact { plan } => &plan.tariff,
    };
    let amount = Counters {
        model_requests: tariff.model_requests,
        tool_calls: tariff.tool_calls,
        spend_micros_usd: tariff.spend_micros_usd,
    };
    let t = read_blob(
        tx,
        "SELECT account FROM task_accounts WHERE task_id=?1",
        task.data().task_id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    let p = read_blob(
        tx,
        "SELECT account FROM principals WHERE principal_id=?1",
        task.data().principal_id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    let mut t: AccountSnapshot = decode!(&t, AccountSnapshot);
    let mut p: AccountSnapshot = decode!(&p, AccountSnapshot);
    for account in [&t, &p] {
        if account.breached || account.incomplete || account.integrity_hold {
            return Ok(Ok(deny(
                tx,
                &task.data().task_id,
                &check.intent_id,
                &BoundedText::new("account unavailable").map_err(StoreError::Validation)?,
                seq,
            )?));
        }
        let Some(total) = add_counters(&account.posted, &account.reserved)
            .and_then(|v| add_counters(&v, &amount))
        else {
            return Ok(Ok(deny(
                tx,
                &task.data().task_id,
                &check.intent_id,
                &BoundedText::new("budget exceeded").map_err(StoreError::Validation)?,
                seq,
            )?));
        };
        if !within(&total, &account.limit) {
            return Ok(Ok(deny(
                tx,
                &task.data().task_id,
                &check.intent_id,
                &BoundedText::new("budget exceeded").map_err(StoreError::Validation)?,
                seq,
            )?));
        }
    }
    t.reserved = add_counters(&t.reserved, &amount).ok_or(StoreError::Integrity)?;
    p.reserved = add_counters(&p.reserved, &amount).ok_or(StoreError::Integrity)?;
    tx.execute(
        "UPDATE task_accounts SET account=?1 WHERE task_id=?2",
        params![bytes!(&t), task.data().task_id.as_str()],
    )
    .map_err(sql_error)?;
    tx.execute(
        "UPDATE principals SET account=?1 WHERE principal_id=?2",
        params![bytes!(&p), task.data().principal_id.as_str()],
    )
    .map_err(sql_error)?;
    let mut statement = tx
        .prepare("SELECT value FROM intents ORDER BY row_id")
        .map_err(sql_error)?;
    let mut rows = statement.query([]).map_err(sql_error)?;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let raw = bounded_blob(row, 0).map_err(sql_error)?;
        let other: IntentSnapshot = decode!(&raw, IntentSnapshot);
        if other
            .reservation
            .as_ref()
            .is_some_and(|r| r.id == check.reservation_id)
        {
            return Err(StoreError::Integrity);
        }
    }
    drop(rows);
    drop(statement);
    intent.reservation = Some(ReservationSnapshot {
        id: check.reservation_id.clone(),
        amount,
        status: ReservationStatus::Held,
    });
    intent.dispatch_id = Some(check.prepared.action.dispatch_id.clone());
    intent.status = DispatchStatus::Dispatched;
    save_intent(tx, &intent)?;
    refresh_unresolved(tx, &task.data().task_id)?;
    Ok(Ok(StoreResult::Admitted {
        dispatch_id: check.prepared.action.dispatch_id.clone(),
    }))
}
fn insert_input(
    tx: &Transaction<'_>,
    task: &Task,
    seq: SqlU64,
    source: InboxSource,
    evidence: Option<Digest>,
    event: AgentEvent,
) -> Result<(), StoreError> {
    let data = task.data();
    let mut input = InboxInput {
        id: inbox_id(&data.task_id, seq, 0).map_err(StoreError::Validation)?,
        task_id: data.task_id.clone(),
        principal_id: data.principal_id.clone(),
        source_seq: seq,
        output_ordinal: 0,
        source,
        evidence_id: evidence,
        event,
        digest: content_digest(&[]),
    };
    input.digest = inbox_digest(&input).map_err(StoreError::Validation)?;
    let snapshot = InboxSnapshot {
        input,
        status: InboxStatus::Pending,
        transition: None,
    };
    tx.execute(
        "INSERT INTO inbox(task_id,inbox_id,value) VALUES(?1,?2,?3)",
        params![
            data.task_id.as_str(),
            snapshot.input.id.as_str(),
            bytes!(&snapshot)
        ],
    )
    .map_err(sql_error)?;
    Ok(())
}
fn subtract_counters(a: &Counters, b: &Counters) -> Option<Counters> {
    Some(Counters {
        model_requests: SqlU64::new(a.model_requests.get().checked_sub(b.model_requests.get())?)
            .ok()?,
        tool_calls: SqlU64::new(a.tool_calls.get().checked_sub(b.tool_calls.get())?).ok()?,
        spend_micros_usd: SqlU64::new(
            a.spend_micros_usd
                .get()
                .checked_sub(b.spend_micros_usd.get())?,
        )
        .ok()?,
    })
}
fn usage_counters(usage: &UsageVector) -> Option<Counters> {
    Some(Counters {
        model_requests: SqlU64::new(usage.model_requests.as_u64()?).ok()?,
        tool_calls: SqlU64::new(usage.tool_calls.as_u64()?).ok()?,
        spend_micros_usd: SqlU64::new(usage.spend_micros_usd.as_u64()?).ok()?,
    })
}
fn record_receipt(
    tx: &Transaction<'_>,
    receipt: &Receipt,
    epoch: SqlU64,
    seq: SqlU64,
    reconciliation: bool,
    settlement_authorized: bool,
) -> Result<StoreResult, StoreError> {
    let data = receipt.data();
    let mut intent = load_intent(tx, &data.intent_id)?;
    let task = load_task(tx, &data.task_id)?;
    let prepared = intent.prepared.clone().ok_or(StoreError::Integrity)?;
    if intent.intent.task_id != data.task_id
        || data.schema_version != task.data().schema_version
        || intent.dispatch_id.as_ref() != Some(&data.dispatch_id)
        || prepared.action_digest != data.action_digest
    {
        return Err(StoreError::Integrity);
    }
    if matches!(prepared.request, ApprovedRequest::Coding { .. }) {
        metron_protocol::coding_runtime::validate_coding_receipt(
            &load_genesis(tx, &data.task_id)?,
            &prepared,
            receipt,
        )
        .map_err(StoreError::Validation)?;
    }
    let shape = matches!(prepared.request, ApprovedRequest::Coding { .. })
        || matches!(
            (&prepared.request, &data.output),
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
        );
    if !shape {
        return Err(StoreError::Integrity);
    }
    if let (
        ApprovedRequest::RequestLocalModel {
            output_schema,
            input_digest,
            ..
        },
        ReceiptOutput::LocalModel { observation },
    ) = (&prepared.request, &data.output)
    {
        if observation.input_digest != *input_digest {
            return Err(StoreError::Integrity);
        }
        if let ActualUsage::Known { usage } = &data.usage
            && (usage.model_requests.as_str() == "0" || usage.spend_micros_usd.as_str() != "0")
        {
            return Err(StoreError::Integrity);
        }
        let genesis = load_genesis(tx, &data.task_id)?;
        let profile = genesis
            .local_profile
            .as_ref()
            .ok_or(StoreError::Integrity)?;
        observation
            .validate(profile, &prepared.request_digest, *output_schema)
            .map_err(StoreError::Validation)?;
    }
    if let ReceiptOutput::Fixture { bytes, digest } = &data.output
        && content_digest(bytes.as_str().as_bytes()) != *digest
    {
        return Err(StoreError::Integrity);
    }
    if let (
        ApprovedRequest::PublishArtifact { plan },
        ReceiptOutput::Published {
            artifact_id,
            content_digest,
            content_length,
        },
    ) = (&prepared.request, &data.output)
        && (*artifact_id != plan.artifact_id
            || *content_digest != plan.content_digest
            || *content_length != plan.content_length)
    {
        return Err(StoreError::Integrity);
    }
    let id = receipt.evidence_id().map_err(StoreError::Validation)?;
    let existing = read_blob(
        tx,
        "SELECT value FROM evidence WHERE evidence_id=?1",
        id.as_str(),
    )?;
    if existing.is_some() && !reconciliation {
        return Err(StoreError::Integrity);
    }
    if let Some(raw) = &existing {
        let old: EvidenceSnapshot = decode!(raw, EvidenceSnapshot);
        if old.receipt != *receipt || old.selected || old.conflict {
            return Err(StoreError::Integrity);
        }
    }
    let collision: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM evidence WHERE producer_id=?1 AND evidence_id<>?2)",
            params![data.producer_receipt_id.as_str(), id.as_str()],
            |r| r.get(0),
        )
        .map_err(sql_error)?;
    let mut prior_conflict = false;
    let mut prior_unselected = false;
    let mut conflict_stmt = tx
        .prepare("SELECT value FROM evidence WHERE intent_id=?1")
        .map_err(sql_error)?;
    let mut conflict_rows = conflict_stmt
        .query([data.intent_id.as_str()])
        .map_err(sql_error)?;
    while let Some(row) = conflict_rows.next().map_err(sql_error)? {
        let raw = bounded_blob(row, 0).map_err(sql_error)?;
        let prior: EvidenceSnapshot = decode!(&raw, EvidenceSnapshot);
        prior_conflict |= prior.conflict;
        prior_unselected |= !prior.selected;
    }
    drop(conflict_rows);
    drop(conflict_stmt);
    let conflict = collision || prior_conflict;
    if existing.is_none() {
        intent.evidence_revision = intent
            .evidence_revision
            .checked_add(1)
            .map_err(StoreError::Validation)?;
    }
    let raw = read_blob(
        tx,
        "SELECT verification FROM tasks WHERE task_id=?1",
        data.task_id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    let mut verification: VerificationSnapshot = decode!(&raw, VerificationSnapshot);
    if existing.is_none() {
        verification.evidence_revision = verification
            .evidence_revision
            .checked_add(1)
            .map_err(StoreError::Validation)?;
    }
    let ta = read_blob(
        tx,
        "SELECT account FROM task_accounts WHERE task_id=?1",
        data.task_id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    let pa = read_blob(
        tx,
        "SELECT account FROM principals WHERE principal_id=?1",
        task.data().principal_id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    let mut ta: AccountSnapshot = decode!(&ta, AccountSnapshot);
    let mut pa: AccountSnapshot = decode!(&pa, AccountSnapshot);
    if existing.is_none() {
        supersede_verification(tx, &task, &mut ta, &mut pa)?;
    }
    if collision {
        flag_collisions(
            tx,
            &data.producer_receipt_id,
            &data.task_id,
            &data.intent_id,
        )?;
    }
    let now = tick(tx)?;
    let current = settlement_authorized
        && if reconciliation {
            intent.status == DispatchStatus::Uncertain
                && intent
                    .reconciliation_lease
                    .as_ref()
                    .is_some_and(|lease| lease.epoch == epoch && lease.expires_at > now)
        } else {
            intent.status == DispatchStatus::Dispatched
                && intent
                    .lease
                    .as_ref()
                    .is_some_and(|lease| lease.epoch == epoch && lease.expires_at > now)
        };
    let mut settled = false;
    if conflict || !current || (prior_unselected && !reconciliation) {
        ta.integrity_hold = true;
        pa.integrity_hold = true;
        verification.conflict |= conflict;
        verification.unresolved = true;
    } else if let Some(reservation) = intent.reservation.as_mut() {
        if reservation.status != ReservationStatus::Held {
            return Err(StoreError::Integrity);
        }
        match &data.usage {
            ActualUsage::Known { usage } => {
                let calculated = usage_counters(usage).and_then(|actual| {
                    let t = add_counters(&ta.posted, &actual)?;
                    let p = add_counters(&pa.posted, &actual)?;
                    Some((actual, t, p))
                });
                if let Some((actual, task_posted, principal_posted)) = calculated {
                    ta.posted = task_posted;
                    pa.posted = principal_posted;
                    ta.reserved = subtract_counters(&ta.reserved, &reservation.amount)
                        .ok_or(StoreError::Integrity)?;
                    pa.reserved = subtract_counters(&pa.reserved, &reservation.amount)
                        .ok_or(StoreError::Integrity)?;
                    if !within(&actual, &reservation.amount)
                        || !within(&ta.posted, &ta.limit)
                        || !within(&pa.posted, &pa.limit)
                    {
                        ta.breached = true;
                        pa.breached = true;
                    }
                    reservation.status = ReservationStatus::Settled;
                    intent.status = DispatchStatus::Settled;
                    intent.selected_evidence = Some(id.clone());
                    settled = true;
                    match &data.output {
                        ReceiptOutput::Fixture { .. } => {
                            verification.read = Some(receipt.clone());
                            verification.read_evidence = Some(id.clone());
                        }
                        ReceiptOutput::Model { .. } | ReceiptOutput::LocalModel { .. } => {
                            verification.model = Some(receipt.clone());
                            verification.model_evidence = Some(id.clone());
                        }
                        ReceiptOutput::Published { .. } => {
                            verification.publication = Some(receipt.clone());
                            verification.publication_evidence = Some(id.clone());
                            if let ApprovedRequest::PublishArtifact { plan } = &prepared.request {
                                verification.plan = Some(plan.clone());
                            }
                        }
                        ReceiptOutput::Coding { observation } => match observation.as_ref() {
                            metron_protocol::coding_runtime::CodingObservation::ScriptedModel {
                                ..
                            } => {
                                verification.model = Some(receipt.clone());
                                verification.model_evidence = Some(id.clone());
                            }
                            metron_protocol::coding_runtime::CodingObservation::Artifact {
                                ..
                            } => {
                                verification.publication = Some(receipt.clone());
                                verification.publication_evidence = Some(id.clone());
                            }
                            metron_protocol::coding_runtime::CodingObservation::Tool { .. } => {}
                        },
                        ReceiptOutput::Failure { .. } => {}
                    }
                    insert_input(
                        tx,
                        &task,
                        seq,
                        InboxSource::Settlement,
                        Some(id.clone()),
                        AgentEvent::EffectSettled {
                            intent_id: data.intent_id.clone(),
                            evidence_id: id.clone(),
                            output: data.output.clone(),
                        },
                    )?;
                } else {
                    ta.incomplete = true;
                    pa.incomplete = true;
                    verification.unresolved = true;
                    intent.status = DispatchStatus::Uncertain;
                    let debit = UnpostedDebit {
                        dispatch_id: data.dispatch_id.clone(),
                        usage: usage.clone(),
                    };
                    tx.execute(
                        "INSERT INTO unposted_debits(task_id,dispatch_id,value) VALUES(?1,?2,?3)",
                        params![
                            data.task_id.as_str(),
                            data.dispatch_id.as_str(),
                            bytes!(&debit)
                        ],
                    )
                    .map_err(sql_error)?;
                    if !reconciliation {
                        insert_input(
                            tx,
                            &task,
                            seq,
                            InboxSource::Uncertainty,
                            Some(id.clone()),
                            AgentEvent::EffectUncertain {
                                intent_id: data.intent_id.clone(),
                            },
                        )?;
                    }
                }
            }
            ActualUsage::Unknown { .. } => {
                ta.incomplete = true;
                pa.incomplete = true;
                verification.unresolved = true;
                intent.status = DispatchStatus::Uncertain;
                if !reconciliation {
                    insert_input(
                        tx,
                        &task,
                        seq,
                        InboxSource::Uncertainty,
                        Some(id.clone()),
                        AgentEvent::EffectUncertain {
                            intent_id: data.intent_id.clone(),
                        },
                    )?;
                }
            }
        }
    } else {
        return Err(StoreError::Integrity);
    }
    let snapshot = EvidenceSnapshot {
        evidence_id: id.clone(),
        receipt: receipt.clone(),
        selected: settled,
        conflict,
    };
    if existing.is_some() {
        tx.execute(
            "UPDATE evidence SET value=?1 WHERE evidence_id=?2",
            params![bytes!(&snapshot), id.as_str()],
        )
        .map_err(sql_error)?;
    } else {
        tx.execute(
            "INSERT INTO evidence(intent_id,evidence_id,producer_id,value) VALUES(?1,?2,?3,?4)",
            params![
                data.intent_id.as_str(),
                id.as_str(),
                data.producer_receipt_id.as_str(),
                bytes!(&snapshot)
            ],
        )
        .map_err(sql_error)?;
    }
    tx.execute(
        "UPDATE task_accounts SET account=?1 WHERE task_id=?2",
        params![bytes!(&ta), data.task_id.as_str()],
    )
    .map_err(sql_error)?;
    tx.execute(
        "UPDATE principals SET account=?1 WHERE principal_id=?2",
        params![bytes!(&pa), task.data().principal_id.as_str()],
    )
    .map_err(sql_error)?;
    tx.execute(
        "UPDATE tasks SET verification=?1 WHERE task_id=?2",
        params![bytes!(&verification), data.task_id.as_str()],
    )
    .map_err(sql_error)?;
    save_intent(tx, &intent)?;
    refresh_unresolved(tx, &data.task_id)?;
    Ok(StoreResult::ReceiptRecorded {
        evidence_id: id,
        evidence_revision: intent.evidence_revision,
        settled,
        conflict,
    })
}

fn deny(
    tx: &Transaction<'_>,
    task_id: &TaskId,
    id: &Digest,
    reason: &BoundedText<MAX_TEXT>,
    seq: SqlU64,
) -> Result<StoreResult, StoreError> {
    let mut intent = load_intent(tx, id)?;
    if &intent.intent.task_id != task_id
        || !matches!(
            intent.status,
            DispatchStatus::Ready | DispatchStatus::Leased | DispatchStatus::AwaitingApproval
        )
    {
        return Err(StoreError::Integrity);
    }
    let task = load_task(tx, task_id)?;
    intent.status = DispatchStatus::Denied;
    save_intent(tx, &intent)?;
    insert_input(
        tx,
        &task,
        seq,
        InboxSource::Denial,
        None,
        AgentEvent::Denied {
            intent_id: id.clone(),
            reason: reason.clone(),
        },
    )?;
    Ok(StoreResult::Denied)
}
fn create_approval(
    tx: &Transaction<'_>,
    task_id: &TaskId,
    request: &ApprovalId,
    id: &Digest,
    action: &Digest,
) -> Result<StoreResult, StoreError> {
    let mut stmt = tx
        .prepare("SELECT value FROM intents ORDER BY row_id")
        .map_err(sql_error)?;
    let mut rows = stmt.query([]).map_err(sql_error)?;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let raw = bounded_blob(row, 0).map_err(sql_error)?;
        let other: IntentSnapshot = decode!(&raw, IntentSnapshot);
        if other
            .approval
            .as_ref()
            .is_some_and(|a| &a.request_id == request)
        {
            return Err(StoreError::Integrity);
        }
    }
    drop(rows);
    drop(stmt);
    let mut intent = load_intent(tx, id)?;
    if &intent.intent.task_id != task_id
        || intent.status != DispatchStatus::Ready
        || intent.approval.is_some()
        || intent
            .prepared
            .as_ref()
            .is_none_or(|p| &p.action_digest != action)
    {
        return Err(StoreError::Integrity);
    }
    let expires_at = tick(tx)?
        .checked_add(APPROVAL_TTL_TICKS)
        .map_err(StoreError::Validation)?;
    intent.approval = Some(ApprovalSnapshot {
        task_id: task_id.clone(),
        intent_id: id.clone(),
        request_id: request.clone(),
        action_digest: action.clone(),
        expires_at,
        decision: None,
    });
    intent.status = DispatchStatus::AwaitingApproval;
    save_intent(tx, &intent)?;
    Ok(StoreResult::ApprovalRequestCreated)
}
fn record_approval(
    tx: &Transaction<'_>,
    request: &ApprovalId,
    action: &Digest,
    decision: ApprovalDecision,
) -> Result<Option<StoreResult>, StoreError> {
    let mut stmt = tx
        .prepare("SELECT value FROM intents ORDER BY row_id")
        .map_err(sql_error)?;
    let mut rows = stmt.query([]).map_err(sql_error)?;
    let mut found = None;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let raw: Vec<u8> = bounded_blob(row, 0).map_err(sql_error)?;
        let i: IntentSnapshot = decode!(&raw, IntentSnapshot);
        if i.approval
            .as_ref()
            .is_some_and(|a| &a.request_id == request)
        {
            if found.is_some() {
                return Err(StoreError::Integrity);
            }
            found = Some(i);
        }
    }
    drop(rows);
    drop(stmt);
    let Some(mut intent) = found else {
        return Ok(None);
    };
    let now = tick(tx)?;
    if intent.status != DispatchStatus::AwaitingApproval {
        return Ok(None);
    }
    let approval = intent.approval.as_mut().ok_or(StoreError::Integrity)?;
    if &approval.action_digest != action
        || approval.expires_at <= now
        || approval.decision.is_some()
    {
        return Ok(None);
    }
    approval.decision = Some(decision);
    if decision == ApprovalDecision::Approve {
        intent.status = DispatchStatus::Ready;
    }
    save_intent(tx, &intent)?;
    Ok(Some(StoreResult::ApprovalRecorded))
}
fn revoke(
    tx: &Transaction<'_>,
    id: &PrincipalId,
    expected: SqlU64,
) -> Result<Option<StoreResult>, StoreError> {
    let raw = read_blob(
        tx,
        "SELECT grant FROM principals WHERE principal_id=?1",
        id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    let mut grant: GrantSnapshot = decode!(&raw, GrantSnapshot);
    if grant.revision != expected {
        return Ok(None);
    }
    grant.revision = expected.checked_add(1).map_err(StoreError::Validation)?;
    grant.revoked = true;
    tx.execute(
        "UPDATE principals SET grant=?1 WHERE principal_id=?2",
        params![bytes!(&grant), id.as_str()],
    )
    .map_err(sql_error)?;
    Ok(Some(StoreResult::GrantRevoked {
        revision: grant.revision,
    }))
}
fn uncertainty(
    tx: &Transaction<'_>,
    task_id: &TaskId,
    id: &Digest,
    dispatch: &Digest,
    epoch: SqlU64,
    seq: SqlU64,
) -> Result<Option<StoreResult>, StoreError> {
    let mut intent = load_intent(tx, id)?;
    if &intent.intent.task_id != task_id
        || intent.dispatch_id.as_ref() != Some(dispatch)
        || intent.lease_epoch != epoch
        || intent.status != DispatchStatus::Dispatched
    {
        return Ok(None);
    }
    let task = load_task(tx, task_id)?;
    intent.status = DispatchStatus::Uncertain;
    save_intent(tx, &intent)?;
    let raw = read_blob(
        tx,
        "SELECT verification FROM tasks WHERE task_id=?1",
        task_id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    let mut v: VerificationSnapshot = decode!(&raw, VerificationSnapshot);
    v.unresolved = true;
    tx.execute(
        "UPDATE tasks SET verification=?1 WHERE task_id=?2",
        params![bytes!(&v), task_id.as_str()],
    )
    .map_err(sql_error)?;
    insert_input(
        tx,
        &task,
        seq,
        InboxSource::Uncertainty,
        None,
        AgentEvent::EffectUncertain {
            intent_id: id.clone(),
        },
    )?;
    Ok(Some(StoreResult::UncertaintyRecorded))
}

fn verify(
    tx: &Transaction<'_>,
    task_id: &TaskId,
    selection: &VerificationSelection,
    accepted: bool,
    reason: &BoundedText<MAX_TEXT>,
    seq: SqlU64,
) -> Result<Option<StoreResult>, StoreError> {
    let task = load_task(tx, task_id)?;
    if task.coding().is_some() {
        return Err(StoreError::Integrity);
    }
    let raw = read_blob(
        tx,
        "SELECT state FROM tasks WHERE task_id=?1",
        task_id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    let state: AgentState = decode!(&raw, AgentState);
    if state.phase != Phase::AwaitingVerification {
        return Ok(None);
    }
    let raw = read_blob(
        tx,
        "SELECT verification FROM tasks WHERE task_id=?1",
        task_id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    let v: VerificationSnapshot = decode!(&raw, VerificationSnapshot);
    if v.evidence_revision != selection.evidence_revision
        || v.read_evidence.as_ref() != Some(&selection.read_evidence)
        || v.model_evidence.as_ref() != Some(&selection.model_evidence)
        || v.publication_evidence.as_ref() != Some(&selection.publication_evidence)
        || v.unresolved
        || v.conflict
    {
        return Ok(None);
    }
    let publication = v.publication.as_ref().ok_or(StoreError::Integrity)?;
    let intent = load_intent(tx, &publication.data().intent_id)?;
    if intent
        .prepared
        .as_ref()
        .is_none_or(|p| p.request_digest != selection.plan_digest)
        || intent.status != DispatchStatus::Settled
    {
        return Ok(None);
    }
    finish_verification(tx, &task, accepted, reason, seq)
}

fn finish_verification(
    tx: &Transaction<'_>,
    task: &Task,
    accepted: bool,
    reason: &BoundedText<MAX_TEXT>,
    seq: SqlU64,
) -> Result<Option<StoreResult>, StoreError> {
    let task_id = &task.data().task_id;
    let raw = read_blob(
        tx,
        "SELECT account FROM task_accounts WHERE task_id=?1",
        task_id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    let ta: AccountSnapshot = decode!(&raw, AccountSnapshot);
    let raw = read_blob(
        tx,
        "SELECT account FROM principals WHERE principal_id=?1",
        task.data().principal_id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    let pa: AccountSnapshot = decode!(&raw, AccountSnapshot);
    if ta.breached
        || pa.breached
        || ta.incomplete
        || pa.incomplete
        || ta.integrity_hold
        || pa.integrity_hold
    {
        return Ok(None);
    }
    let mut stmt = tx
        .prepare("SELECT value FROM intents WHERE task_id=?1")
        .map_err(sql_error)?;
    let mut rows = stmt
        .query([task.data().task_id.as_str()])
        .map_err(sql_error)?;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let raw: Vec<u8> = bounded_blob(row, 0).map_err(sql_error)?;
        let item: IntentSnapshot = decode!(&raw, IntentSnapshot);
        if item.status != DispatchStatus::Settled
            || item
                .reservation
                .as_ref()
                .is_some_and(|r| r.status == ReservationStatus::Held)
        {
            return Ok(None);
        }
    }
    drop(rows);
    drop(stmt);
    insert_input(
        tx,
        task,
        seq,
        InboxSource::Verification,
        None,
        AgentEvent::Verified {
            accepted,
            reason: reason.clone(),
        },
    )?;
    Ok(Some(StoreResult::VerificationRecorded))
}

fn verify_coding(
    tx: &Transaction<'_>,
    task_id: &TaskId,
    selection: &CodingVerificationSelection,
    accepted: bool,
    reason: &BoundedText<MAX_TEXT>,
    seq: SqlU64,
) -> Result<Option<StoreResult>, StoreError> {
    let task = load_task(tx, task_id)?;
    let raw = read_blob(
        tx,
        "SELECT state FROM tasks WHERE task_id=?1",
        task_id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    let state: AgentState = decode!(&raw, AgentState);
    let raw = read_blob(
        tx,
        "SELECT verification FROM tasks WHERE task_id=?1",
        task_id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    let snapshot: VerificationSnapshot = decode!(&raw, VerificationSnapshot);
    if !metron_protocol::coding_runtime::coding_selection_matches(
        &task, &state, &snapshot, selection,
    ) {
        return Ok(None);
    }
    for id in [
        &selection.plan_evidence,
        &selection.model_evidence,
        &selection.artifact_evidence,
    ] {
        let raw = read_blob(
            tx,
            "SELECT value FROM evidence WHERE evidence_id=?1",
            id.as_str(),
        )?
        .ok_or(StoreError::Integrity)?;
        let evidence: EvidenceSnapshot = decode!(&raw, EvidenceSnapshot);
        if !evidence.selected
            || evidence.conflict
            || evidence.receipt.data().task_id != *task_id
            || evidence
                .receipt
                .evidence_id()
                .map_err(StoreError::Validation)?
                != *id
        {
            return Ok(None);
        }
        if id == &selection.plan_evidence
            && !metron_protocol::coding_runtime::model_content(&evidence.receipt.data().output)
                .is_some_and(|content| {
                    matches!(
                        metron_protocol::coding::parse_coding_action(content.as_bytes()),
                        Ok(metron_protocol::coding::CodingAction::Plan { .. })
                    )
                })
        {
            return Ok(None);
        }
    }
    finish_verification(tx, &task, accepted, reason, seq)
}

fn claim_reconciliation(
    tx: &Transaction<'_>,
    task_id: &TaskId,
    id: &Digest,
    worker: &WorkerId,
    expected: SqlU64,
    evidence: SqlU64,
) -> Result<Result<StoreResult, NotCommitted>, StoreError> {
    let mut intent = load_intent(tx, id)?;
    let now = tick(tx)?;
    if &intent.intent.task_id != task_id {
        return Err(StoreError::Integrity);
    }
    if intent.status != DispatchStatus::Uncertain {
        return Ok(Err(NotCommitted::NoEligible));
    }
    if intent.evidence_revision != evidence {
        return Ok(Err(NotCommitted::EvidenceChanged));
    }
    if intent.reconciliation_epoch != expected
        || intent
            .reconciliation_lease
            .as_ref()
            .is_some_and(|l| l.expires_at > now)
    {
        return Ok(Err(NotCommitted::LeaseConflict));
    }
    let epoch = expected.checked_add(1).map_err(StoreError::Validation)?;
    let expires_at = now.checked_add(30).map_err(StoreError::Validation)?;
    intent.reconciliation_epoch = epoch;
    intent.reconciliation_lease = Some(LeaseSnapshot {
        worker: worker.clone(),
        epoch,
        expires_at,
    });
    save_intent(tx, &intent)?;
    Ok(Ok(StoreResult::ReconciliationClaimed {
        lease_epoch: epoch,
        expires_at,
    }))
}
#[allow(clippy::too_many_arguments)]
fn settle_reconciliation(
    tx: &Transaction<'_>,
    task_id: &TaskId,
    id: &Digest,
    worker: &WorkerId,
    epoch: SqlU64,
    expected: SqlU64,
    evidence: &Digest,
    supplied: Option<&Receipt>,
    seq: SqlU64,
) -> Result<Result<StoreResult, NotCommitted>, StoreError> {
    let intent = load_intent(tx, id)?;
    let now = tick(tx)?;
    if &intent.intent.task_id != task_id {
        return Err(StoreError::Integrity);
    }
    if let Some(receipt) = supplied
        && (receipt.evidence_id().map_err(StoreError::Validation)? != *evidence
            || receipt.data().intent_id != *id
            || receipt.data().task_id != *task_id)
    {
        return Err(StoreError::Integrity);
    }
    let valid_lease = intent
        .reconciliation_lease
        .as_ref()
        .is_some_and(|l| &l.worker == worker && l.epoch == epoch && l.expires_at > now);
    if !valid_lease || intent.evidence_revision != expected {
        if let Some(receipt) = supplied
            && read_blob(
                tx,
                "SELECT value FROM evidence WHERE evidence_id=?1",
                evidence.as_str(),
            )?
            .is_none()
        {
            return Ok(Ok(record_receipt(tx, receipt, epoch, seq, false, false)?));
        }
        return Ok(Err(if valid_lease {
            NotCommitted::EvidenceChanged
        } else {
            NotCommitted::LeaseConflict
        }));
    }
    let owned;
    let receipt = if let Some(receipt) = supplied {
        receipt
    } else {
        let raw = read_blob(
            tx,
            "SELECT value FROM evidence WHERE evidence_id=?1",
            evidence.as_str(),
        )?
        .ok_or(StoreError::Integrity)?;
        let snapshot: EvidenceSnapshot = decode!(&raw, EvidenceSnapshot);
        owned = snapshot.receipt;
        &owned
    };
    if receipt.data().intent_id != *id
        || receipt.data().task_id != *task_id
        || receipt.evidence_id().map_err(StoreError::Validation)? != *evidence
    {
        return Err(StoreError::Integrity);
    }
    if !compatible_reconciliation(tx, id, receipt)? {
        if supplied.is_some()
            && read_blob(
                tx,
                "SELECT value FROM evidence WHERE evidence_id=?1",
                evidence.as_str(),
            )?
            .is_none()
        {
            return Ok(Ok(record_receipt(tx, receipt, epoch, seq, false, false)?));
        }
        return Ok(Err(NotCommitted::EvidenceChanged));
    }
    let result = record_receipt(tx, receipt, epoch, seq, true, true)?;
    match result {
        StoreResult::ReceiptRecorded { settled: true, .. } => {
            clear_reconciled_hold(tx, task_id, id, seq)?;
            refresh_unresolved(tx, task_id)?;
            Ok(Ok(StoreResult::ReconciliationSettled {
                evidence_id: evidence.clone(),
            }))
        }
        other => Ok(Ok(other)),
    }
}

fn refresh_unresolved(tx: &Transaction<'_>, task_id: &TaskId) -> Result<(), StoreError> {
    let raw = read_blob(
        tx,
        "SELECT verification FROM tasks WHERE task_id=?1",
        task_id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    let mut v: VerificationSnapshot = decode!(&raw, VerificationSnapshot);
    let mut stmt = tx
        .prepare("SELECT value FROM intents WHERE task_id=?1")
        .map_err(sql_error)?;
    let mut rows = stmt.query([task_id.as_str()]).map_err(sql_error)?;
    let mut unresolved = false;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let raw: Vec<u8> = bounded_blob(row, 0).map_err(sql_error)?;
        let intent: IntentSnapshot = decode!(&raw, IntentSnapshot);
        unresolved |= matches!(
            intent.status,
            DispatchStatus::Dispatched | DispatchStatus::Uncertain
        ) || intent
            .reservation
            .as_ref()
            .is_some_and(|r| r.status == ReservationStatus::Held);
    }
    drop(rows);
    drop(stmt);
    let raw = read_blob(
        tx,
        "SELECT account FROM task_accounts WHERE task_id=?1",
        task_id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    let account: AccountSnapshot = decode!(&raw, AccountSnapshot);
    v.unresolved = unresolved || account.incomplete || account.integrity_hold;
    tx.execute(
        "UPDATE tasks SET verification=?1 WHERE task_id=?2",
        params![bytes!(&v), task_id.as_str()],
    )
    .map_err(sql_error)?;
    Ok(())
}
fn supersede_verification(
    tx: &Transaction<'_>,
    task: &Task,
    ta: &mut AccountSnapshot,
    pa: &mut AccountSnapshot,
) -> Result<(), StoreError> {
    let mut stmt = tx
        .prepare("SELECT row_id,value FROM inbox WHERE task_id=?1")
        .map_err(sql_error)?;
    let mut rows = stmt
        .query([task.data().task_id.as_str()])
        .map_err(sql_error)?;
    let mut updates = Vec::new();
    while let Some(row) = rows.next().map_err(sql_error)? {
        let row_id: i64 = row.get(0).map_err(sql_error)?;
        let raw: Vec<u8> = bounded_blob(row, 1).map_err(sql_error)?;
        let mut item: InboxSnapshot = decode!(&raw, InboxSnapshot);
        if matches!(item.input.event, AgentEvent::Verified { .. }) {
            if item.status == InboxStatus::Pending {
                item.status = InboxStatus::Superseded;
                updates.push((row_id, item));
            } else if item.status == InboxStatus::Consumed {
                ta.integrity_hold = true;
                pa.integrity_hold = true;
                tx.execute(
                    "UPDATE tasks SET invalidated_verification=1 WHERE task_id=?1",
                    [task.data().task_id.as_str()],
                )
                .map_err(sql_error)?;
            }
        }
    }
    drop(rows);
    drop(stmt);
    for (row_id, item) in updates {
        tx.execute(
            "UPDATE inbox SET value=?1 WHERE row_id=?2",
            params![bytes!(&item), row_id],
        )
        .map_err(sql_error)?;
    }
    Ok(())
}
fn flag_collisions(
    tx: &Transaction<'_>,
    producer: &ProducerReceiptId,
    current_task: &TaskId,
    current_intent: &Digest,
) -> Result<(), StoreError> {
    let mut stmt = tx
        .prepare("SELECT row_id,value FROM evidence WHERE producer_id=?1")
        .map_err(sql_error)?;
    let mut rows = stmt.query([producer.as_str()]).map_err(sql_error)?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().map_err(sql_error)? {
        let raw = bounded_blob(row, 1).map_err(sql_error)?;
        let mut evidence: EvidenceSnapshot = decode!(&raw, EvidenceSnapshot);
        evidence.conflict = true;
        items.push((row.get::<_, i64>(0).map_err(sql_error)?, evidence));
    }
    drop(rows);
    drop(stmt);
    let mut tasks = std::collections::BTreeSet::new();
    let mut intents = std::collections::BTreeSet::new();
    for (row_id, evidence) in items {
        tx.execute(
            "UPDATE evidence SET value=?1 WHERE row_id=?2",
            params![bytes!(&evidence), row_id],
        )
        .map_err(sql_error)?;
        let data = evidence.receipt.data();
        if &data.intent_id != current_intent && intents.insert(data.intent_id.clone()) {
            let mut intent = load_intent(tx, &data.intent_id)?;
            intent.evidence_revision = intent
                .evidence_revision
                .checked_add(1)
                .map_err(StoreError::Validation)?;
            save_intent(tx, &intent)?;
        }
        if &data.task_id == current_task || !tasks.insert(data.task_id.clone()) {
            continue;
        }
        let task = load_task(tx, &data.task_id)?;
        let raw = read_blob(
            tx,
            "SELECT account FROM task_accounts WHERE task_id=?1",
            data.task_id.as_str(),
        )?
        .ok_or(StoreError::Integrity)?;
        let mut ta: AccountSnapshot = decode!(&raw, AccountSnapshot);
        let raw = read_blob(
            tx,
            "SELECT account FROM principals WHERE principal_id=?1",
            task.data().principal_id.as_str(),
        )?
        .ok_or(StoreError::Integrity)?;
        let mut pa: AccountSnapshot = decode!(&raw, AccountSnapshot);
        ta.integrity_hold = true;
        pa.integrity_hold = true;
        supersede_verification(tx, &task, &mut ta, &mut pa)?;
        tx.execute(
            "UPDATE task_accounts SET account=?1 WHERE task_id=?2",
            params![bytes!(&ta), data.task_id.as_str()],
        )
        .map_err(sql_error)?;
        tx.execute(
            "UPDATE principals SET account=?1 WHERE principal_id=?2",
            params![bytes!(&pa), task.data().principal_id.as_str()],
        )
        .map_err(sql_error)?;
        let raw = read_blob(
            tx,
            "SELECT verification FROM tasks WHERE task_id=?1",
            data.task_id.as_str(),
        )?
        .ok_or(StoreError::Integrity)?;
        let mut verification: VerificationSnapshot = decode!(&raw, VerificationSnapshot);
        verification.conflict = true;
        verification.unresolved = true;
        verification.evidence_revision = verification
            .evidence_revision
            .checked_add(1)
            .map_err(StoreError::Validation)?;
        tx.execute(
            "UPDATE tasks SET verification=?1 WHERE task_id=?2",
            params![bytes!(&verification), data.task_id.as_str()],
        )
        .map_err(sql_error)?;
    }
    Ok(())
}

fn validate_input_source(tx: &Transaction<'_>, input: &InboxInput) -> Result<(), StoreError> {
    if input.output_ordinal != 0 {
        return Err(StoreError::Integrity);
    }
    let raw: Vec<u8> = tx
        .query_row(
            "SELECT payload FROM journal WHERE seq=?1",
            [input.source_seq.get() as i64],
            |r| bounded_blob(r, 0),
        )
        .map_err(sql_error)?;
    let record: JournalRecord = decode!(&raw, JournalRecord);
    if record.data().seq != input.source_seq {
        return Err(StoreError::Integrity);
    }
    let JournalEvent::CommandCommitted { command, result } = &record.data().event;
    let matches = match (
        &input.source,
        &input.event,
        &command.data().operation,
        result,
    ) {
        (
            InboxSource::TaskAccepted,
            AgentEvent::TaskAccepted,
            StoreOperation::CreateTask { task, .. },
            StoreResult::TaskCreated { .. },
        ) => task.data().task_id == input.task_id && input.evidence_id.is_none(),
        (
            InboxSource::Denial,
            AgentEvent::Denied { intent_id, reason },
            StoreOperation::Deny {
                task_id,
                intent_id: source,
                reason: source_reason,
            },
            StoreResult::Denied,
        ) => {
            task_id == &input.task_id
                && source == intent_id
                && source_reason == reason
                && input.evidence_id.is_none()
        }
        (
            InboxSource::Denial,
            AgentEvent::Denied { intent_id, reason },
            StoreOperation::Admit { check },
            StoreResult::Denied,
        ) => {
            &check.intent_id == intent_id
                && matches!(
                    reason.as_str(),
                    "grant revoked"
                        | "account unavailable"
                        | "budget exceeded"
                        | "coding action outside grant"
                )
                && input.evidence_id.is_none()
        }
        (
            InboxSource::Uncertainty,
            AgentEvent::EffectUncertain { intent_id },
            StoreOperation::RecordUncertainty {
                intent_id: source, ..
            },
            StoreResult::UncertaintyRecorded,
        ) => source == intent_id && input.evidence_id.is_none(),
        (
            InboxSource::Uncertainty,
            AgentEvent::EffectUncertain { intent_id },
            StoreOperation::RecordReceipt { receipt, .. },
            StoreResult::ReceiptRecorded {
                evidence_id,
                settled: false,
                ..
            },
        ) => {
            &receipt.data().intent_id == intent_id
                && input.evidence_id.as_ref() == Some(evidence_id)
        }
        (
            InboxSource::Settlement,
            AgentEvent::EffectSettled {
                intent_id,
                evidence_id,
                output,
            },
            StoreOperation::RecordReceipt { receipt, .. },
            StoreResult::ReceiptRecorded {
                evidence_id: source,
                settled: true,
                ..
            },
        ) => {
            &receipt.data().intent_id == intent_id
                && &receipt.data().output == output
                && source == evidence_id
                && input.evidence_id.as_ref() == Some(evidence_id)
        }
        (
            InboxSource::Settlement,
            AgentEvent::EffectSettled {
                intent_id,
                evidence_id,
                output,
            },
            StoreOperation::SettleReconciliation {
                intent_id: source, ..
            },
            StoreResult::ReconciliationSettled {
                evidence_id: chosen,
            },
        ) => {
            let raw = read_blob(
                tx,
                "SELECT value FROM evidence WHERE evidence_id=?1",
                evidence_id.as_str(),
            )?
            .ok_or(StoreError::Integrity)?;
            let evidence: EvidenceSnapshot = decode!(&raw, EvidenceSnapshot);
            source == intent_id
                && chosen == evidence_id
                && evidence.selected
                && &evidence.receipt.data().output == output
                && input.evidence_id.as_ref() == Some(evidence_id)
        }
        (
            InboxSource::Verification,
            AgentEvent::Verified { accepted, reason },
            StoreOperation::RecordVerification {
                task_id,
                accepted: a,
                reason: r,
                ..
            }
            | StoreOperation::RecordCodingVerification {
                task_id,
                accepted: a,
                reason: r,
                ..
            },
            StoreResult::VerificationRecorded,
        ) => {
            task_id == &input.task_id && a == accepted && r == reason && input.evidence_id.is_none()
        }
        _ => false,
    };
    if matches {
        Ok(())
    } else {
        Err(StoreError::Integrity)
    }
}

fn compatible_reconciliation(
    tx: &Transaction<'_>,
    id: &Digest,
    candidate: &Receipt,
) -> Result<bool, StoreError> {
    let unposted: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM unposted_debits WHERE dispatch_id=?1)",
            [candidate.data().dispatch_id.as_str()],
            |r| r.get(0),
        )
        .map_err(sql_error)?;
    if unposted {
        return Ok(false);
    }
    if !matches!(candidate.data().usage, ActualUsage::Known { .. }) {
        return Ok(false);
    }
    let mut stmt = tx
        .prepare("SELECT value FROM evidence WHERE intent_id=?1")
        .map_err(sql_error)?;
    let mut rows = stmt.query([id.as_str()]).map_err(sql_error)?;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let raw = bounded_blob(row, 0).map_err(sql_error)?;
        let prior: EvidenceSnapshot = decode!(&raw, EvidenceSnapshot);
        if prior.conflict
            || prior.receipt.data().usage != candidate.data().usage
            || prior.receipt.data().output != candidate.data().output
        {
            return Ok(false);
        }
    }
    Ok(true)
}

// Derive first evidence origins and per-intent resolution frontiers in one
// streaming pass; later evidence cannot inherit an earlier resolution.
fn evidence_resolved_at(
    tx: &Transaction<'_>,
    pending: &std::collections::BTreeMap<Digest, Digest>,
    current_intent: &Digest,
    current_seq: SqlU64,
) -> Result<bool, StoreError> {
    if pending.is_empty() {
        return Ok(true);
    }
    let mut origins: std::collections::BTreeMap<Digest, Option<SqlU64>> =
        pending.keys().cloned().map(|id| (id, None)).collect();
    let mut frontiers: std::collections::BTreeMap<Digest, Option<SqlU64>> =
        pending.values().cloned().map(|id| (id, None)).collect();
    if let Some(frontier) = frontiers.get_mut(current_intent) {
        *frontier = Some(current_seq);
    }
    let mut stmt = tx
        .prepare("SELECT seq,payload FROM journal ORDER BY seq")
        .map_err(sql_error)?;
    let mut rows = stmt.query([]).map_err(sql_error)?;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let seq: i64 = row.get(0).map_err(sql_error)?;
        let seq = SqlU64::new(seq.try_into().map_err(|_| StoreError::Integrity)?)
            .map_err(StoreError::Validation)?;
        let raw = bounded_blob(row, 1).map_err(sql_error)?;
        let record: JournalRecord = decode!(&raw, JournalRecord);
        let JournalEvent::CommandCommitted { command, result } = &record.data().event;
        if let (
            StoreOperation::SettleReconciliation { intent_id, .. },
            StoreResult::ReconciliationSettled { .. },
        ) = (&command.data().operation, result)
            && let Some(frontier) = frontiers.get_mut(intent_id)
        {
            *frontier = Some(frontier.map_or(seq, |old| old.max(seq)));
        }
        let candidate = match &command.data().operation {
            StoreOperation::RecordReceipt { receipt, .. } => Some(receipt),
            StoreOperation::SettleReconciliation {
                reconciled_receipt: Some(receipt),
                ..
            } => Some(receipt),
            _ => None,
        };
        if let Some(receipt) = candidate {
            let id = receipt.evidence_id().map_err(StoreError::Validation)?;
            if let Some(origin) = origins.get_mut(&id)
                && origin.is_none()
            {
                *origin = Some(seq);
            }
        }
    }
    Ok(pending.iter().all(|(evidence,intent)| matches!((origins.get(evidence).copied().flatten(),frontiers.get(intent).copied().flatten()),(Some(origin),Some(frontier)) if origin<=frontier)))
}
fn clear_reconciled_hold(
    tx: &Transaction<'_>,
    task_id: &TaskId,
    reconciled_id: &Digest,
    reconciled_seq: SqlU64,
) -> Result<(), StoreError> {
    let task = load_task(tx, task_id)?;
    let invalidated: i64 = tx
        .query_row(
            "SELECT invalidated_verification FROM tasks WHERE task_id=?1",
            [task_id.as_str()],
            |r| r.get(0),
        )
        .map_err(sql_error)?;
    let raw = read_blob(
        tx,
        "SELECT verification FROM tasks WHERE task_id=?1",
        task_id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    let verification: VerificationSnapshot = decode!(&raw, VerificationSnapshot);
    let raw = read_blob(
        tx,
        "SELECT account FROM task_accounts WHERE task_id=?1",
        task_id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    let mut ta: AccountSnapshot = decode!(&raw, AccountSnapshot);
    if invalidated != 0 || verification.conflict || ta.incomplete {
        return Ok(());
    }
    let unposted: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM unposted_debits WHERE task_id=?1)",
            [task_id.as_str()],
            |r| r.get(0),
        )
        .map_err(sql_error)?;
    if unposted {
        return Ok(());
    }
    let mut evidence_stmt=tx.prepare("SELECT e.value FROM evidence e JOIN intents i ON i.intent_id=e.intent_id WHERE i.task_id=?1").map_err(sql_error)?;
    let mut evidence_rows = evidence_stmt.query([task_id.as_str()]).map_err(sql_error)?;
    let mut pending = std::collections::BTreeMap::new();
    while let Some(row) = evidence_rows.next().map_err(sql_error)? {
        let raw = bounded_blob(row, 0).map_err(sql_error)?;
        let evidence: EvidenceSnapshot = decode!(&raw, EvidenceSnapshot);
        if evidence.conflict {
            return Ok(());
        }
        if !evidence.selected {
            pending.insert(
                evidence.evidence_id,
                evidence.receipt.data().intent_id.clone(),
            );
        }
    }
    drop(evidence_rows);
    drop(evidence_stmt);
    if !evidence_resolved_at(tx, &pending, reconciled_id, reconciled_seq)? {
        return Ok(());
    }
    let mut stmt = tx
        .prepare("SELECT value FROM intents WHERE task_id=?1")
        .map_err(sql_error)?;
    let mut rows = stmt.query([task_id.as_str()]).map_err(sql_error)?;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let raw = bounded_blob(row, 0).map_err(sql_error)?;
        let intent: IntentSnapshot = decode!(&raw, IntentSnapshot);
        if matches!(
            intent.status,
            DispatchStatus::Dispatched | DispatchStatus::Uncertain
        ) || intent
            .reservation
            .as_ref()
            .is_some_and(|r| r.status == ReservationStatus::Held)
        {
            return Ok(());
        }
    }
    drop(rows);
    drop(stmt);
    ta.integrity_hold = false;
    tx.execute(
        "UPDATE task_accounts SET account=?1 WHERE task_id=?2",
        params![bytes!(&ta), task_id.as_str()],
    )
    .map_err(sql_error)?;
    let mut stmt = tx
        .prepare("SELECT t.task FROM tasks t JOIN task_accounts a ON a.task_id=t.task_id")
        .map_err(sql_error)?;
    let mut rows = stmt.query([]).map_err(sql_error)?;
    let mut ids = Vec::new();
    while let Some(row) = rows.next().map_err(sql_error)? {
        let raw = bounded_blob(row, 0).map_err(sql_error)?;
        let t: Task = decode!(&raw, Task);
        if t.data().principal_id == task.data().principal_id {
            ids.push(t.data().task_id.clone());
        }
    }
    drop(rows);
    drop(stmt);
    let mut held = false;
    for id in ids {
        let raw = read_blob(
            tx,
            "SELECT account FROM task_accounts WHERE task_id=?1",
            id.as_str(),
        )?
        .ok_or(StoreError::Integrity)?;
        let account: AccountSnapshot = decode!(&raw, AccountSnapshot);
        held |= account.integrity_hold;
    }
    let raw = read_blob(
        tx,
        "SELECT account FROM principals WHERE principal_id=?1",
        task.data().principal_id.as_str(),
    )?
    .ok_or(StoreError::Integrity)?;
    let mut pa: AccountSnapshot = decode!(&raw, AccountSnapshot);
    pa.integrity_hold = held;
    tx.execute(
        "UPDATE principals SET account=?1 WHERE principal_id=?2",
        params![bytes!(&pa), task.data().principal_id.as_str()],
    )
    .map_err(sql_error)?;
    Ok(())
}

fn prior_verification(
    tx: &Transaction<'_>,
    task_id: &TaskId,
    selection: &VerificationSelection,
    accepted: bool,
    reason: &BoundedText<MAX_TEXT>,
) -> Result<Option<StoreResult>, StoreError> {
    let mut stmt = tx
        .prepare("SELECT payload FROM journal ORDER BY seq")
        .map_err(sql_error)?;
    let mut rows = stmt.query([]).map_err(sql_error)?;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let raw = bounded_blob(row, 0).map_err(sql_error)?;
        let record: JournalRecord = decode!(&raw, JournalRecord);
        let JournalEvent::CommandCommitted { command, result } = &record.data().event;
        if let (
            StoreOperation::RecordVerification {
                task_id: t,
                selection: s,
                accepted: a,
                reason: r,
            },
            StoreResult::VerificationRecorded,
        ) = (&command.data().operation, result)
            && t == task_id
            && s == selection
        {
            if *a != accepted || r != reason {
                return Err(StoreError::Integrity);
            }
            return Ok(Some(StoreResult::VerificationRecorded));
        }
    }
    Ok(None)
}
fn prior_coding_verification(
    tx: &Transaction<'_>,
    task_id: &TaskId,
    selection: &CodingVerificationSelection,
    accepted: bool,
    reason: &BoundedText<MAX_TEXT>,
) -> Result<Option<StoreResult>, StoreError> {
    let mut stmt = tx
        .prepare("SELECT payload FROM journal ORDER BY seq")
        .map_err(sql_error)?;
    let mut rows = stmt.query([]).map_err(sql_error)?;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let raw = bounded_blob(row, 0).map_err(sql_error)?;
        let record: JournalRecord = decode!(&raw, JournalRecord);
        let JournalEvent::CommandCommitted { command, result } = &record.data().event;
        if let (
            StoreOperation::RecordCodingVerification {
                task_id: t,
                selection: s,
                accepted: a,
                reason: r,
            },
            StoreResult::VerificationRecorded,
        ) = (&command.data().operation, result)
            && t == task_id
            && s == selection
        {
            if *a != accepted || r != reason {
                return Err(StoreError::Integrity);
            }
            return Ok(Some(StoreResult::VerificationRecorded));
        }
    }
    Ok(None)
}
fn prior_approval(
    tx: &Transaction<'_>,
    request: &ApprovalId,
    action: &Digest,
    decision: ApprovalDecision,
) -> Result<Option<StoreResult>, StoreError> {
    let mut stmt = tx
        .prepare("SELECT value FROM intents ORDER BY row_id")
        .map_err(sql_error)?;
    let mut rows = stmt.query([]).map_err(sql_error)?;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let raw = bounded_blob(row, 0).map_err(sql_error)?;
        let intent: IntentSnapshot = decode!(&raw, IntentSnapshot);
        if let Some(approval) = intent.approval
            && &approval.request_id == request
        {
            if &approval.action_digest != action {
                return Err(StoreError::Integrity);
            }
            return match approval.decision {
                Some(old) if old == decision => Ok(Some(StoreResult::ApprovalRecorded)),
                Some(_) => Err(StoreError::Integrity),
                None => Ok(None),
            };
        }
    }
    Ok(None)
}

fn check_intent_key(tx: &Transaction<'_>, item: &IntentSnapshot) -> Result<(), StoreError> {
    let key: Option<String> = tx
        .query_row(
            "SELECT task_id FROM intents WHERE intent_id=?1",
            [item.intent_id.as_str()],
            |r| r.get(0),
        )
        .optional()
        .map_err(sql_error)?;
    if key.as_deref() != Some(item.intent.task_id.as_str()) {
        return Err(StoreError::Integrity);
    }
    Ok(())
}
fn check_inbox_key(tx: &Transaction<'_>, item: &InboxSnapshot) -> Result<(), StoreError> {
    let key: Option<String> = tx
        .query_row(
            "SELECT task_id FROM inbox WHERE inbox_id=?1",
            [item.input.id.as_str()],
            |r| r.get(0),
        )
        .optional()
        .map_err(sql_error)?;
    if key.as_deref() != Some(item.input.task_id.as_str())
        || inbox_digest(&item.input).map_err(StoreError::Validation)? != item.input.digest
        || inbox_id(
            &item.input.task_id,
            item.input.source_seq,
            item.input.output_ordinal,
        )
        .map_err(StoreError::Validation)?
            != item.input.id
    {
        return Err(StoreError::Integrity);
    }
    Ok(())
}
fn check_evidence_key(tx: &Transaction<'_>, item: &EvidenceSnapshot) -> Result<(), StoreError> {
    let key: Option<(String, String)> = tx
        .query_row(
            "SELECT intent_id,producer_id FROM evidence WHERE evidence_id=?1",
            [item.evidence_id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(sql_error)?;
    if key.as_ref().is_none_or(|(intent, producer)| {
        intent != item.receipt.data().intent_id.as_str()
            || producer != item.receipt.data().producer_receipt_id.as_str()
    }) || item.receipt.evidence_id().map_err(StoreError::Validation)? != item.evidence_id
    {
        return Err(StoreError::Integrity);
    }
    Ok(())
}
fn validate_representation(tx: &Transaction<'_>) -> Result<(), StoreError> {
    // Alpha.0 audit is deliberately bounded; the persisted format remains unrestricted.
    let query = format!(
        "SELECT length(payload) FROM journal ORDER BY seq LIMIT {}",
        MAX_AUDIT_RECORDS + 1
    );
    let mut budget_stmt = tx.prepare(&query).map_err(sql_error)?;
    let mut budget_rows = budget_stmt.query([]).map_err(sql_error)?;
    let mut count = 0_usize;
    let mut size = 0_usize;
    while let Some(row) = budget_rows.next().map_err(sql_error)? {
        count += 1;
        let len: usize = row
            .get::<_, i64>(0)
            .map_err(sql_error)?
            .try_into()
            .map_err(|_| StoreError::Integrity)?;
        size = size
            .checked_add(len)
            .ok_or_else(|| operational("audit resource limit exceeded"))?;
        if count > MAX_AUDIT_RECORDS || size > MAX_AUDIT_JOURNAL_BYTES {
            return Err(operational("audit resource limit exceeded"));
        }
    }
    drop(budget_rows);
    drop(budget_stmt);
    // Pagination starts after zero; invalid keys must not hide durable rows.
    for table in ["intents", "inbox", "evidence", "unposted_debits"] {
        let invalid: bool = tx
            .query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE row_id<=0)"),
                [],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        if invalid {
            return Err(StoreError::Integrity);
        }
    }
    let commands: i64 = tx
        .query_row("SELECT count(*) FROM commands", [], |r| r.get(0))
        .map_err(sql_error)?;
    let journal: i64 = tx
        .query_row("SELECT count(*) FROM journal", [], |r| r.get(0))
        .map_err(sql_error)?;
    if commands != journal {
        return Err(StoreError::Integrity);
    }
    for table in [
        "tasks",
        "principals",
        "task_accounts",
        "intents",
        "inbox",
        "evidence",
        "unposted_debits",
    ] {
        let count: i64 = tx
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .map_err(sql_error)?;
        if count > journal {
            return Err(StoreError::Integrity);
        }
    }

    let mut stmt=tx.prepare("SELECT c.rowid,c.command_id,c.digest,c.result,j.seq,j.payload FROM commands c LEFT JOIN journal j ON j.seq=c.rowid ORDER BY c.rowid").map_err(sql_error)?;
    let mut rows = stmt.query([]).map_err(sql_error)?;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let row_id: i64 = row.get(0).map_err(sql_error)?;
        let id: String = row.get(1).map_err(sql_error)?;
        let digest: String = row.get(2).map_err(sql_error)?;
        let result_raw = bounded_blob(row, 3).map_err(|_| StoreError::Integrity)?;
        let seq: Option<i64> = row.get(4).map_err(sql_error)?;
        if seq != Some(row_id) {
            return Err(StoreError::Integrity);
        }
        let record_raw = bounded_blob(row, 5).map_err(|_| StoreError::Integrity)?;
        let record: JournalRecord = decode!(&record_raw, JournalRecord);
        let saved: StoreResult = decode!(&result_raw, StoreResult);
        let JournalEvent::CommandCommitted { command, result } = &record.data().event;
        if record.data().seq.get() != row_id as u64
            || command.data().command_id.as_str() != id
            || command.digest().map_err(StoreError::Validation)?.as_str() != digest
            || result != &saved
        {
            return Err(StoreError::Integrity);
        }
    }
    drop(rows);
    drop(stmt);
    let mut stmt = tx
        .prepare("SELECT task_id,intent_id,value FROM intents ORDER BY row_id")
        .map_err(sql_error)?;
    let mut rows = stmt.query([]).map_err(sql_error)?;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let task: String = row.get(0).map_err(sql_error)?;
        let id: String = row.get(1).map_err(sql_error)?;
        let raw = bounded_blob(row, 2).map_err(|_| StoreError::Integrity)?;
        let item: IntentSnapshot = decode!(&raw, IntentSnapshot);
        if item.intent.task_id.as_str() != task || item.intent_id.as_str() != id {
            return Err(StoreError::Integrity);
        }
    }
    drop(rows);
    drop(stmt);
    let mut stmt = tx
        .prepare("SELECT task_id,inbox_id,value FROM inbox ORDER BY row_id")
        .map_err(sql_error)?;
    let mut rows = stmt.query([]).map_err(sql_error)?;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let task: String = row.get(0).map_err(sql_error)?;
        let id: String = row.get(1).map_err(sql_error)?;
        let raw = bounded_blob(row, 2).map_err(|_| StoreError::Integrity)?;
        let item: InboxSnapshot = decode!(&raw, InboxSnapshot);
        if item.input.task_id.as_str() != task
            || item.input.id.as_str() != id
            || inbox_digest(&item.input).map_err(StoreError::Validation)? != item.input.digest
            || inbox_id(
                &item.input.task_id,
                item.input.source_seq,
                item.input.output_ordinal,
            )
            .map_err(StoreError::Validation)?
                != item.input.id
        {
            return Err(StoreError::Integrity);
        }
    }
    drop(rows);
    drop(stmt);
    let mut stmt = tx
        .prepare("SELECT intent_id,evidence_id,producer_id,value FROM evidence ORDER BY row_id")
        .map_err(sql_error)?;
    let mut rows = stmt.query([]).map_err(sql_error)?;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let intent: String = row.get(0).map_err(sql_error)?;
        let id: String = row.get(1).map_err(sql_error)?;
        let producer: String = row.get(2).map_err(sql_error)?;
        let raw = bounded_blob(row, 3).map_err(|_| StoreError::Integrity)?;
        let item: EvidenceSnapshot = decode!(&raw, EvidenceSnapshot);
        if item.receipt.data().intent_id.as_str() != intent
            || item.evidence_id.as_str() != id
            || item.receipt.data().producer_receipt_id.as_str() != producer
            || item.receipt.evidence_id().map_err(StoreError::Validation)? != item.evidence_id
        {
            return Err(StoreError::Integrity);
        }
    }
    drop(rows);
    drop(stmt);
    let mut stmt = tx
        .prepare("SELECT task_id,task,state FROM tasks ORDER BY task_id")
        .map_err(sql_error)?;
    let mut rows = stmt.query([]).map_err(sql_error)?;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let id: String = row.get(0).map_err(sql_error)?;
        let raw = bounded_blob(row, 1).map_err(|_| StoreError::Integrity)?;
        let task: Task = decode!(&raw, Task);
        let raw = bounded_blob(row, 2).map_err(|_| StoreError::Integrity)?;
        let state: AgentState = decode!(&raw, AgentState);
        if task.data().task_id.as_str() != id || state.task_id.as_str() != id {
            return Err(StoreError::Integrity);
        }
    }
    drop(rows);
    drop(stmt);
    let mut stmt = tx
        .prepare("SELECT principal_id,grant FROM principals ORDER BY principal_id")
        .map_err(sql_error)?;
    let mut rows = stmt.query([]).map_err(sql_error)?;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let id: String = row.get(0).map_err(sql_error)?;
        let raw = bounded_blob(row, 1).map_err(|_| StoreError::Integrity)?;
        let grant: GrantSnapshot = decode!(&raw, GrantSnapshot);
        if grant.principal_id.as_str() != id {
            return Err(StoreError::Integrity);
        }
    }
    drop(rows);
    drop(stmt);
    let mut stmt = tx
        .prepare("SELECT dispatch_id,value FROM unposted_debits ORDER BY row_id")
        .map_err(sql_error)?;
    let mut rows = stmt.query([]).map_err(sql_error)?;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let id: String = row.get(0).map_err(sql_error)?;
        let raw = bounded_blob(row, 1).map_err(|_| StoreError::Integrity)?;
        let debit: UnpostedDebit = decode!(&raw, UnpostedDebit);
        if debit.dispatch_id.as_str() != id {
            return Err(StoreError::Integrity);
        }
    }

    Ok(())
}
#[cfg(test)]
mod authority_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn num(n: u64) -> SqlU64 {
        SqlU64::new(n).unwrap()
    }
    fn cmd(id: &str, operation: StoreOperation) -> StoreCommand {
        StoreCommand::new(CommandData {
            schema_version: SchemaVersion::V1,
            command_id: CommandId::new(id).unwrap(),
            operation,
        })
        .unwrap()
    }
    fn make_task(id: &str) -> Task {
        Task::new(TaskData {
            schema_version: SchemaVersion::V1,
            task_id: TaskId::new(id).unwrap(),
            principal_id: PrincipalId::new("shared").unwrap(),
            objective: BoundedText::new("test").unwrap(),
            budget: BudgetRequest {
                max_model_requests: num(2),
                max_tool_calls: num(2),
                max_spend_micros_usd: num(0),
                max_elapsed_ms: None,
            },
            max_decision_steps: 8,
        })
        .unwrap()
    }
    fn genesis() -> Genesis {
        Genesis {
            coding_policy: None,
            mode: EngineMode::Agent,
            engine_version: BoundedText::new("sum-agent/v1").unwrap(),
            require_publication_approval: true,
            policy_digest: content_digest(b"policy"),
            local_profile: None,
        }
    }
    fn store() -> (SqliteStore, std::path::PathBuf) {
        let p = std::env::temp_dir().join(format!(
            "metron-authority-{}-{}.db",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        (SqliteStore::create(&p).unwrap(), p)
    }
    use metron_protocol::*;
    fn local_profile() -> LocalProfile {
        LocalProfile {
            schema_version: SchemaVersion::V2,
            auxiliary_repository: BoundedText::new("LiquidAI/LFM3-1.7B").unwrap(),
            auxiliary_revision: BoundedText::new("1da06bec6a819da10b54c67e5c90972b46d81871")
                .unwrap(),
            artifact_repository: BoundedText::new("LiquidAI/LFM3-1.7B-GGUF").unwrap(),
            artifact_revision: BoundedText::new("cd0c74c575b5c416f3011b5b4c298077f26ca8c2")
                .unwrap(),
            artifact_name: BoundedText::new("LFM3-1.7B-Q4_K_M.gguf").unwrap(),
            artifact_digest: Digest::new(
                "3793e1e6e2266919d6e015287221b41100175a98eddaba92e111aed3b05c93c4",
            )
            .unwrap(),
            artifact_bytes: SqlU64::new(1099627808).unwrap(),
            embedded_tokenizer_digest: gguf_embedded_tokenizer_digest(
                &Digest::new("3793e1e6e2266919d6e015287221b41100175a98eddaba92e111aed3b05c93c4")
                    .unwrap(),
            )
            .unwrap(),
            template_digest: content_digest(b"template"),
            prompt_recipe_digest: content_digest(b"recipe"),
            backend_revision: BoundedText::new("fb4cdc3730dc64b366724e7637a205a8b7747499").unwrap(),
            backend_build_digest: content_digest(b"build"),
            cpu_identity: BoundedText::new("test-cpu").unwrap(),
            cpu_features_digest: content_digest(b"cpu-features"),
            device: LocalDevice::Cpu,
            cpu_threads: 2,
            context_tokens: 2048,
            max_input_tokens: 1536,
            max_output_tokens: 512,
            temperature_milli: 100,
            top_k: 50,
            repeat_penalty_milli: 1100,
            max_sequences: 1,
            queue_capacity: 8,
            queue_wait_ms: 5000,
            deadline_ms: 60000,
            chunked_prefill: false,
            scoped_prefix_cache: false,
            cache_target_bytes: None,
            strict_memory_cap_bytes: None,
        }
    }

    #[test]
    fn local_task_rejects_mixed_versions_before_mutation() {
        let (mut store, path) = store();
        let mut data = make_task("local").data().clone();
        data.schema_version = SchemaVersion::V2;
        let task = Task::new(data).unwrap();
        let mut g = genesis();
        g.local_profile = Some(local_profile());
        g.engine_version = BoundedText::new("sum-local-agent/v2").unwrap();
        let command = |id: &str, version, operation| {
            StoreCommand::new(CommandData {
                schema_version: version,
                command_id: CommandId::new(id).unwrap(),
                operation,
            })
            .unwrap()
        };
        let create = StoreOperation::CreateTask {
            task: task.clone(),
            genesis: g.clone(),
        };
        assert!(matches!(
            store.apply(command("bad-create", SchemaVersion::V1, create.clone())),
            Err(StoreError::Validation(ValidationError::Version))
        ));
        assert!(matches!(
            store
                .apply(command("create-v2", SchemaVersion::V2, create))
                .unwrap(),
            WriteOutcome::Committed(_)
        ));
        let (before, input) = {
            let mut snap = store.audit().unwrap();
            (
                snap.projection(&task.data().task_id).unwrap(),
                snap.pending_inbox(&task.data().task_id).unwrap().unwrap(),
            )
        };
        let mut next = before.state.clone();
        next.revision = num(1);
        next.decision_steps = 1;
        next.phase = Phase::WaitingRead;
        let intent = Intent {
            schema_version: SchemaVersion::V1,
            task_id: task.data().task_id.clone(),
            ordinal: 0,
            kind: IntentKind::ReadFixture,
        };
        let prepared = PreparedIntent::materialize(
            &intent,
            &task.data().principal_id,
            &before.state.genesis_digest,
        )
        .unwrap();
        next.pending_intent = Some(prepared.intent_id);
        let op = StoreOperation::CommitTransition {
            task_id: task.data().task_id.clone(),
            expected_revision: num(0),
            inbox_id: input.id.clone(),
            inbox_digest: input.digest.clone(),
            transition: Transition::Decision { next, intent },
        };
        assert!(matches!(
            store.apply(command(
                "wrong-command-version",
                SchemaVersion::V1,
                op.clone()
            )),
            Err(StoreError::Validation(ValidationError::Version))
        ));
        assert!(matches!(
            store.apply(command("wrong-intent-version", SchemaVersion::V2, op)),
            Err(StoreError::Validation(ValidationError::Version))
        ));
        let mut snap = store.audit().unwrap();
        assert_eq!(snap.projection(&task.data().task_id).unwrap(), before);
        assert_eq!(
            snap.pending_inbox(&task.data().task_id).unwrap(),
            Some(input)
        );
        drop(snap);
        drop(store);
        let _ = std::fs::remove_file(path);
    }
    #[test]
    fn local_read_to_model_binds_profile_and_rejects_changed_observation() {
        let (mut store, path) = store();
        let mut data = make_task("local-path").data().clone();
        data.schema_version = SchemaVersion::V2;
        let task = Task::new(data).unwrap();
        let mut genesis = genesis();
        genesis.require_publication_approval = false;
        genesis.local_profile = Some(local_profile());
        genesis.engine_version = BoundedText::new("sum-local-agent/v2").unwrap();
        let command = |id: &str, operation| {
            StoreCommand::new(CommandData {
                schema_version: SchemaVersion::V2,
                command_id: CommandId::new(id).unwrap(),
                operation,
            })
            .unwrap()
        };
        let apply = |s: &mut SqliteStore, id: &str, operation| {
            assert!(matches!(
                s.apply(command(id, operation)).unwrap(),
                WriteOutcome::Committed(_)
            ));
        };
        apply(
            &mut store,
            "create-local",
            StoreOperation::CreateTask {
                task: task.clone(),
                genesis: genesis.clone(),
            },
        );
        let (mut state, input) = {
            let mut snap = store.audit().unwrap();
            (
                snap.projection(&task.data().task_id).unwrap().state,
                snap.pending_inbox(&task.data().task_id).unwrap().unwrap(),
            )
        };
        let read = Intent {
            schema_version: SchemaVersion::V2,
            task_id: task.data().task_id.clone(),
            ordinal: 0,
            kind: IntentKind::ReadFixture,
        };
        let prepared =
            PreparedIntent::materialize(&read, &task.data().principal_id, &state.genesis_digest)
                .unwrap();
        state.revision = num(1);
        state.decision_steps = 1;
        state.phase = Phase::WaitingRead;
        state.pending_intent = Some(prepared.intent_id.clone());
        apply(
            &mut store,
            "read-transition",
            StoreOperation::CommitTransition {
                task_id: task.data().task_id.clone(),
                expected_revision: num(0),
                inbox_id: input.id,
                inbox_digest: input.digest,
                transition: Transition::Decision {
                    next: state,
                    intent: read,
                },
            },
        );
        apply(
            &mut store,
            "read-prepare",
            StoreOperation::PrepareIntent {
                task_id: task.data().task_id.clone(),
                prepared: prepared.clone(),
            },
        );
        let worker = WorkerId::new("w").unwrap();
        apply(
            &mut store,
            "read-claim",
            StoreOperation::Claim {
                task_id: task.data().task_id.clone(),
                intent_id: prepared.intent_id.clone(),
                worker: worker.clone(),
                expected_epoch: num(0),
            },
        );
        apply(
            &mut store,
            "read-admit",
            StoreOperation::Admit {
                check: AdmissionCheck {
                    intent_id: prepared.intent_id.clone(),
                    prepared: prepared.clone(),
                    worker: worker.clone(),
                    lease_epoch: num(1),
                    expected_tick: num(0),
                    grant_revision: num(0),
                    approval_id: None,
                    reservation_id: content_digest(b"read-reserve"),
                },
            },
        );
        let text = "{\"values\":[17,-4,9]}\n";
        let fixture_digest = content_digest(text.as_bytes());
        let receipt = Receipt::new(ReceiptData {
            schema_version: SchemaVersion::V2,
            producer_receipt_id: ProducerReceiptId::new("read").unwrap(),
            task_id: task.data().task_id.clone(),
            intent_id: prepared.intent_id,
            dispatch_id: prepared.action.dispatch_id,
            action_digest: prepared.action_digest,
            usage: known(),
            output: ReceiptOutput::Fixture {
                bytes: BoundedText::new(text).unwrap(),
                digest: fixture_digest.clone(),
            },
        })
        .unwrap();
        apply(
            &mut store,
            "read-receipt",
            StoreOperation::RecordReceipt {
                receipt,
                lease_epoch: num(1),
            },
        );
        let (mut state, input) = {
            let mut snap = store.audit().unwrap();
            (
                snap.projection(&task.data().task_id).unwrap().state,
                snap.pending_inbox(&task.data().task_id).unwrap().unwrap(),
            )
        };
        let mut model = Intent {
            schema_version: SchemaVersion::V2,
            task_id: task.data().task_id.clone(),
            ordinal: 1,
            kind: IntentKind::RequestLocalModel {
                input_digest: fixture_digest.clone(),
                context: BoundedText::new(text).unwrap(),
                profile_digest: content_digest(b"wrong-profile"),
                role: LocalRole::Solver,
                output_schema: LocalOutputSchema::Sum,
            },
        };
        let invalid =
            PreparedIntent::materialize(&model, &task.data().principal_id, &state.genesis_digest)
                .unwrap();
        state.revision = num(2);
        state.decision_steps = 2;
        state.phase = Phase::WaitingModel;
        state.pending_intent = Some(invalid.intent_id.clone());
        let transition = |intent: Intent, next: AgentState| StoreOperation::CommitTransition {
            task_id: task.data().task_id.clone(),
            expected_revision: num(1),
            inbox_id: input.id.clone(),
            inbox_digest: input.digest.clone(),
            transition: Transition::Decision { next, intent },
        };
        assert!(matches!(
            store.apply(command(
                "wrong-profile",
                transition(model.clone(), state.clone())
            )),
            Err(StoreError::Validation(ValidationError::Binding))
        ));
        if let IntentKind::RequestLocalModel { profile_digest, .. } = &mut model.kind {
            *profile_digest = genesis.local_profile.as_ref().unwrap().digest().unwrap();
        }
        let local =
            PreparedIntent::materialize(&model, &task.data().principal_id, &state.genesis_digest)
                .unwrap();
        apply(&mut store, "model-transition", transition(model, state));
        apply(
            &mut store,
            "model-prepare",
            StoreOperation::PrepareIntent {
                task_id: task.data().task_id.clone(),
                prepared: local.clone(),
            },
        );
        apply(
            &mut store,
            "model-claim",
            StoreOperation::Claim {
                task_id: task.data().task_id.clone(),
                intent_id: local.intent_id.clone(),
                worker: worker.clone(),
                expected_epoch: num(0),
            },
        );
        apply(
            &mut store,
            "model-admit",
            StoreOperation::Admit {
                check: AdmissionCheck {
                    intent_id: local.intent_id.clone(),
                    prepared: local.clone(),
                    worker,
                    lease_epoch: num(1),
                    expected_tick: num(0),
                    grant_revision: num(0),
                    approval_id: None,
                    reservation_id: content_digest(b"model-reserve"),
                },
            },
        );
        let observation = LocalObservation {
            schema_version: SchemaVersion::V2,
            profile_digest: genesis.local_profile.as_ref().unwrap().digest().unwrap(),
            request_digest: local.request_digest.clone(),
            input_digest: fixture_digest,
            content: BoundedText::new("{\"count\":3,\"sum\":22}").unwrap(),
            reasoning: None,
            native_calls: None,
            prompt_tokens: 20,
            completion_tokens: 10,
            submission_cohort: SubmissionCohort {
                id: BoundedText::new("cohort").unwrap(),
                size: 1,
                position: 0,
            },
            backend_batch: BatchProvenance::Unknown,
            response: Some(LocalResponse::Sum(SumResponse { count: 3, sum: 22 })),
        };
        let make_receipt = |observation| {
            Receipt::new(ReceiptData {
                schema_version: SchemaVersion::V2,
                producer_receipt_id: ProducerReceiptId::new("model").unwrap(),
                task_id: task.data().task_id.clone(),
                intent_id: local.intent_id.clone(),
                dispatch_id: local.action.dispatch_id.clone(),
                action_digest: local.action_digest.clone(),
                usage: ActualUsage::Known {
                    usage: UsageVector {
                        model_requests: DecimalUsage::new("1").unwrap(),
                        tool_calls: DecimalUsage::new("0").unwrap(),
                        spend_micros_usd: DecimalUsage::new("0").unwrap(),
                    },
                },
                output: ReceiptOutput::LocalModel {
                    observation: Box::new(observation),
                },
            })
            .unwrap()
        };
        let mut bad = observation.clone();
        bad.request_digest = content_digest(b"wrong-request");
        assert!(matches!(
            store.apply(command(
                "bad-observation",
                StoreOperation::RecordReceipt {
                    receipt: make_receipt(bad),
                    lease_epoch: num(1)
                }
            )),
            Err(StoreError::Validation(ValidationError::Binding))
        ));
        apply(
            &mut store,
            "model-receipt",
            StoreOperation::RecordReceipt {
                receipt: make_receipt(observation),
                lease_epoch: num(1),
            },
        );
        let mut snapshot = store.audit().unwrap();
        let projection = snapshot.projection(&task.data().task_id).unwrap();
        assert_eq!(projection.task_account.posted.model_requests, num(1));
        assert!(matches!(
            projection.verification.model.unwrap().data().output,
            ReceiptOutput::LocalModel { .. }
        ));
        drop(snapshot);
        drop(store);
        let _ = std::fs::remove_file(path);
    }
    #[test]
    fn task_creation_does_not_revive_revoked_principal() {
        let (mut s, p) = store();
        let a = make_task("a");
        let b = make_task("b");
        s.apply(cmd(
            "a",
            StoreOperation::CreateTask {
                task: a,
                genesis: genesis(),
            },
        ))
        .unwrap();
        s.apply(cmd(
            "revoke",
            StoreOperation::RevokeGrant {
                principal_id: b.data().principal_id.clone(),
                expected_revision: num(0),
            },
        ))
        .unwrap();
        s.apply(cmd(
            "b",
            StoreOperation::CreateTask {
                task: b.clone(),
                genesis: genesis(),
            },
        ))
        .unwrap();
        let mut snap = s.audit().unwrap();
        let grant = snap.grant(&b.data().principal_id).unwrap();
        assert!(grant.revoked);
        assert_eq!(grant.revision, num(1));
        drop(snap);
        drop(s);
        let _ = std::fs::remove_file(p);
    }
    #[test]
    fn expired_approval_cannot_be_rescued_by_fresh_lease() {
        let (mut s, p) = store();
        let task = make_task("t");
        let g = genesis();
        s.apply(cmd(
            "create",
            StoreOperation::CreateTask {
                task: task.clone(),
                genesis: g.clone(),
            },
        ))
        .unwrap();
        let intent = Intent {
            schema_version: SchemaVersion::V1,
            task_id: task.data().task_id.clone(),
            ordinal: 0,
            kind: IntentKind::PublishArtifact {
                candidate: CandidateResult {
                    count: 3,
                    sum: 22,
                    input_digest: content_digest(b"input"),
                },
            },
        };
        let material = PreparedIntent::materialize(
            &intent,
            &task.data().principal_id,
            &journal_genesis(&task, &g).unwrap(),
        )
        .unwrap();
        let snapshot = IntentSnapshot {
            intent_id: material.intent_id.clone(),
            intent,
            prepared: Some(material.clone()),
            status: DispatchStatus::Ready,
            dispatch_id: None,
            lease_epoch: num(0),
            lease: None,
            reconciliation_epoch: num(0),
            reconciliation_lease: None,
            reservation: None,
            approval: None,
            evidence_revision: num(0),
            selected_evidence: None,
        };
        s.connection
            .execute(
                "INSERT INTO intents(task_id,intent_id,value) VALUES(?1,?2,?3)",
                params![
                    task.data().task_id.as_str(),
                    material.intent_id.as_str(),
                    canonical_bytes(&snapshot, MAX_INTERNAL).unwrap()
                ],
            )
            .unwrap();
        let approval = ApprovalId::new("approval").unwrap();
        s.apply(cmd(
            "request",
            StoreOperation::CreateApprovalRequest {
                task_id: task.data().task_id.clone(),
                request_id: approval.clone(),
                intent_id: material.intent_id.clone(),
                action_digest: material.action_digest.clone(),
            },
        ))
        .unwrap();
        s.apply(cmd(
            "approve",
            StoreOperation::RecordApproval {
                request_id: approval.clone(),
                action_digest: material.action_digest.clone(),
                decision: ApprovalDecision::Approve,
            },
        ))
        .unwrap();
        s.apply(cmd(
            "tick",
            StoreOperation::AdvanceTick {
                expected_tick: num(0),
                new_tick: num(30),
            },
        ))
        .unwrap();
        let worker = WorkerId::new("fresh").unwrap();
        s.apply(cmd(
            "claim",
            StoreOperation::Claim {
                task_id: task.data().task_id.clone(),
                intent_id: material.intent_id.clone(),
                worker: worker.clone(),
                expected_epoch: num(0),
            },
        ))
        .unwrap();
        let outcome = s
            .apply(cmd(
                "admit",
                StoreOperation::Admit {
                    check: AdmissionCheck {
                        intent_id: material.intent_id.clone(),
                        prepared: material,
                        worker,
                        lease_epoch: num(1),
                        expected_tick: num(30),
                        grant_revision: num(0),
                        approval_id: Some(approval),
                        reservation_id: content_digest(b"r"),
                    },
                },
            ))
            .unwrap();
        assert_eq!(
            outcome,
            WriteOutcome::NotCommitted(NotCommitted::RecheckAuthority)
        );
        drop(s);
        let _ = std::fs::remove_file(p);
    }
    #[test]
    fn unrepresentable_usage_posts_no_partial_vector() {
        let (mut s, p) = store();
        let task = make_task("overflow");
        let g = genesis();
        s.apply(cmd(
            "create",
            StoreOperation::CreateTask {
                task: task.clone(),
                genesis: g.clone(),
            },
        ))
        .unwrap();
        let intent = Intent {
            schema_version: SchemaVersion::V1,
            task_id: task.data().task_id.clone(),
            ordinal: 0,
            kind: IntentKind::ReadFixture,
        };
        let prepared = PreparedIntent::materialize(
            &intent,
            &task.data().principal_id,
            &journal_genesis(&task, &g).unwrap(),
        )
        .unwrap();
        let value = IntentSnapshot {
            intent_id: prepared.intent_id.clone(),
            intent,
            prepared: Some(prepared.clone()),
            status: DispatchStatus::Ready,
            dispatch_id: None,
            lease_epoch: num(0),
            lease: None,
            reconciliation_epoch: num(0),
            reconciliation_lease: None,
            reservation: None,
            approval: None,
            evidence_revision: num(0),
            selected_evidence: None,
        };
        s.connection
            .execute(
                "INSERT INTO intents(task_id,intent_id,value) VALUES(?1,?2,?3)",
                params![
                    task.data().task_id.as_str(),
                    prepared.intent_id.as_str(),
                    canonical_bytes(&value, MAX_INTERNAL).unwrap()
                ],
            )
            .unwrap();
        let worker = WorkerId::new("w").unwrap();
        s.apply(cmd(
            "claim",
            StoreOperation::Claim {
                task_id: task.data().task_id.clone(),
                intent_id: prepared.intent_id.clone(),
                worker: worker.clone(),
                expected_epoch: num(0),
            },
        ))
        .unwrap();
        s.apply(cmd(
            "admit",
            StoreOperation::Admit {
                check: AdmissionCheck {
                    intent_id: prepared.intent_id.clone(),
                    prepared: prepared.clone(),
                    worker,
                    lease_epoch: num(1),
                    expected_tick: num(0),
                    grant_revision: num(0),
                    approval_id: None,
                    reservation_id: content_digest(b"reservation"),
                },
            },
        ))
        .unwrap();
        let usage = UsageVector {
            model_requests: DecimalUsage::new("18446744073709551616").unwrap(),
            tool_calls: DecimalUsage::new("1").unwrap(),
            spend_micros_usd: DecimalUsage::new("2").unwrap(),
        };
        let text = "{\"values\":[17,-4,9]}\n";
        let receipt = Receipt::new(ReceiptData {
            schema_version: SchemaVersion::V1,
            producer_receipt_id: ProducerReceiptId::new("receipt").unwrap(),
            task_id: task.data().task_id.clone(),
            intent_id: prepared.intent_id.clone(),
            dispatch_id: prepared.action.dispatch_id.clone(),
            action_digest: prepared.action_digest.clone(),
            usage: ActualUsage::Known {
                usage: usage.clone(),
            },
            output: ReceiptOutput::Fixture {
                bytes: BoundedText::new(text).unwrap(),
                digest: content_digest(text.as_bytes()),
            },
        })
        .unwrap();
        let result = s
            .apply(cmd(
                "receipt",
                StoreOperation::RecordReceipt {
                    receipt,
                    lease_epoch: num(1),
                },
            ))
            .unwrap();
        assert!(matches!(
            result,
            WriteOutcome::Committed(StoreReply {
                result: StoreResult::ReceiptRecorded { settled: false, .. },
                ..
            })
        ));
        let mut snap = s.audit().unwrap();
        let state = snap.projection(&task.data().task_id).unwrap();
        for account in [&state.task_account, &state.principal_account] {
            assert_eq!(account.posted.tool_calls, num(0));
            assert_eq!(account.posted.spend_micros_usd, num(0));
            assert_eq!(account.reserved.tool_calls, num(1));
            assert!(account.incomplete);
        }
        let debit = snap.unposted_debits(&task.data().task_id, None).unwrap();
        assert_eq!(debit.records.len(), 1);
        assert_eq!(debit.records[0].usage, usage);
        drop(snap);
        drop(s);
        let _ = std::fs::remove_file(p);
    }
    #[test]
    fn committed_noop_remembers_command_and_checks_reuse() {
        let (mut s, p) = store();
        let task = make_task("dedup");
        let g = genesis();
        s.apply(cmd(
            "create",
            StoreOperation::CreateTask {
                task: task.clone(),
                genesis: g.clone(),
            },
        ))
        .unwrap();
        let intent = Intent {
            schema_version: SchemaVersion::V1,
            task_id: task.data().task_id.clone(),
            ordinal: 0,
            kind: IntentKind::ReadFixture,
        };
        let prepared = PreparedIntent::materialize(
            &intent,
            &task.data().principal_id,
            &journal_genesis(&task, &g).unwrap(),
        )
        .unwrap();
        let value = IntentSnapshot {
            intent_id: prepared.intent_id.clone(),
            intent,
            prepared: Some(prepared.clone()),
            status: DispatchStatus::Ready,
            dispatch_id: None,
            lease_epoch: num(0),
            lease: None,
            reconciliation_epoch: num(0),
            reconciliation_lease: None,
            reservation: None,
            approval: None,
            evidence_revision: num(0),
            selected_evidence: None,
        };
        s.connection
            .execute(
                "INSERT INTO intents(task_id,intent_id,value) VALUES(?1,?2,?3)",
                params![
                    task.data().task_id.as_str(),
                    prepared.intent_id.as_str(),
                    canonical_bytes(&value, MAX_INTERNAL).unwrap()
                ],
            )
            .unwrap();
        let command = cmd(
            "repeat",
            StoreOperation::PrepareIntent {
                task_id: task.data().task_id.clone(),
                prepared,
            },
        );
        assert!(matches!(
            s.apply(command.clone()).unwrap(),
            WriteOutcome::Committed(StoreReply {
                disposition: Disposition::PreviouslyCommitted,
                ..
            })
        ));
        assert!(matches!(
            s.apply(command).unwrap(),
            WriteOutcome::Committed(StoreReply {
                disposition: Disposition::Duplicate,
                ..
            })
        ));
        assert!(matches!(
            s.apply(cmd(
                "repeat",
                StoreOperation::AdvanceTick {
                    expected_tick: num(0),
                    new_tick: num(1)
                }
            )),
            Err(StoreError::Integrity)
        ));
        let count: i64 = s
            .connection
            .query_row("SELECT count(*) FROM journal", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
        drop(s);
        let _ = std::fs::remove_file(p);
    }
    #[test]
    fn busy_and_sequence_overflow_do_not_mutate() {
        let (mut s, p) = store();
        let task = make_task("locking");
        s.apply(cmd(
            "create",
            StoreOperation::CreateTask {
                task,
                genesis: genesis(),
            },
        ))
        .unwrap();
        let blocker = rusqlite::Connection::open(&p).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert!(matches!(
            s.apply(cmd(
                "tick",
                StoreOperation::AdvanceTick {
                    expected_tick: num(0),
                    new_tick: num(1)
                }
            )),
            Err(StoreError::Busy)
        ));
        blocker.execute_batch("ROLLBACK").unwrap();
        drop(blocker);
        s.connection
            .execute(
                "INSERT INTO journal(seq,payload) VALUES(?1,x'7b7d')",
                [i64::MAX],
            )
            .unwrap();
        assert!(matches!(
            s.apply(cmd(
                "overflow",
                StoreOperation::AdvanceTick {
                    expected_tick: num(0),
                    new_tick: num(1)
                }
            )),
            Err(StoreError::Validation(ValidationError::Limit))
        ));
        let count: i64 = s
            .connection
            .query_row(
                "SELECT count(*) FROM commands WHERE command_id='overflow'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        drop(s);
        let _ = std::fs::remove_file(p);
    }
    #[test]
    fn concurrent_admission_cannot_reserve_same_principal_unit() {
        let (mut s, p) = store();
        let mut checks = Vec::new();
        for (n, id) in ["race-a", "race-b"].into_iter().enumerate() {
            let mut data = make_task(id).data().clone();
            data.budget.max_tool_calls = num(1);
            let task = Task::new(data).unwrap();
            let g = genesis();
            s.apply(cmd(
                &format!("create{n}"),
                StoreOperation::CreateTask {
                    task: task.clone(),
                    genesis: g.clone(),
                },
            ))
            .unwrap();
            let intent = Intent {
                schema_version: SchemaVersion::V1,
                task_id: task.data().task_id.clone(),
                ordinal: 0,
                kind: IntentKind::ReadFixture,
            };
            let prepared = PreparedIntent::materialize(
                &intent,
                &task.data().principal_id,
                &journal_genesis(&task, &g).unwrap(),
            )
            .unwrap();
            let value = IntentSnapshot {
                intent_id: prepared.intent_id.clone(),
                intent,
                prepared: Some(prepared.clone()),
                status: DispatchStatus::Ready,
                dispatch_id: None,
                lease_epoch: num(0),
                lease: None,
                reconciliation_epoch: num(0),
                reconciliation_lease: None,
                reservation: None,
                approval: None,
                evidence_revision: num(0),
                selected_evidence: None,
            };
            s.connection
                .execute(
                    "INSERT INTO intents(task_id,intent_id,value) VALUES(?1,?2,?3)",
                    params![
                        task.data().task_id.as_str(),
                        prepared.intent_id.as_str(),
                        canonical_bytes(&value, MAX_INTERNAL).unwrap()
                    ],
                )
                .unwrap();
            let worker = WorkerId::new(format!("worker{n}")).unwrap();
            s.apply(cmd(
                &format!("claim{n}"),
                StoreOperation::Claim {
                    task_id: task.data().task_id.clone(),
                    intent_id: prepared.intent_id.clone(),
                    worker: worker.clone(),
                    expected_epoch: num(0),
                },
            ))
            .unwrap();
            checks.push(AdmissionCheck {
                intent_id: prepared.intent_id.clone(),
                prepared,
                worker,
                lease_epoch: num(1),
                expected_tick: num(0),
                grant_revision: num(0),
                approval_id: None,
                reservation_id: content_digest(format!("reservation{n}").as_bytes()),
            });
        }
        drop(s);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let threads: Vec<_> = checks
            .into_iter()
            .enumerate()
            .map(|(n, check)| {
                let b = barrier.clone();
                let path = p.clone();
                std::thread::spawn(move || {
                    let mut s = SqliteStore::open_existing(path).unwrap();
                    b.wait();
                    s.apply(cmd(&format!("admit{n}"), StoreOperation::Admit { check }))
                        .unwrap()
                })
            })
            .collect();
        let outcomes: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(
            outcomes
                .iter()
                .filter(|v| matches!(
                    v,
                    WriteOutcome::Committed(StoreReply {
                        result: StoreResult::Admitted { .. },
                        ..
                    })
                ))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|v| matches!(
                    v,
                    WriteOutcome::Committed(StoreReply {
                        result: StoreResult::Denied,
                        ..
                    })
                ))
                .count(),
            1
        );
        let mut s = SqliteStore::open_existing(&p).unwrap();
        let mut snapshot = s.audit().unwrap();
        let account = snapshot
            .projection(&TaskId::new("race-a").unwrap())
            .unwrap()
            .principal_account;
        assert_eq!(account.reserved.tool_calls, num(1));
        drop(snapshot);
        drop(s);
        let _ = std::fs::remove_file(p);
    }
    #[test]
    fn late_conflicting_receipt_preserves_both_and_freezes_accounts() {
        let (mut s, p) = store();
        let task = make_task("collision");
        let g = genesis();
        s.apply(cmd(
            "create",
            StoreOperation::CreateTask {
                task: task.clone(),
                genesis: g.clone(),
            },
        ))
        .unwrap();
        let intent = Intent {
            schema_version: SchemaVersion::V1,
            task_id: task.data().task_id.clone(),
            ordinal: 0,
            kind: IntentKind::ReadFixture,
        };
        let prepared = PreparedIntent::materialize(
            &intent,
            &task.data().principal_id,
            &journal_genesis(&task, &g).unwrap(),
        )
        .unwrap();
        let value = IntentSnapshot {
            intent_id: prepared.intent_id.clone(),
            intent,
            prepared: Some(prepared.clone()),
            status: DispatchStatus::Ready,
            dispatch_id: None,
            lease_epoch: num(0),
            lease: None,
            reconciliation_epoch: num(0),
            reconciliation_lease: None,
            reservation: None,
            approval: None,
            evidence_revision: num(0),
            selected_evidence: None,
        };
        s.connection
            .execute(
                "INSERT INTO intents(task_id,intent_id,value) VALUES(?1,?2,?3)",
                params![
                    task.data().task_id.as_str(),
                    prepared.intent_id.as_str(),
                    canonical_bytes(&value, MAX_INTERNAL).unwrap()
                ],
            )
            .unwrap();
        let worker = WorkerId::new("w").unwrap();
        s.apply(cmd(
            "claim",
            StoreOperation::Claim {
                task_id: task.data().task_id.clone(),
                intent_id: prepared.intent_id.clone(),
                worker: worker.clone(),
                expected_epoch: num(0),
            },
        ))
        .unwrap();
        s.apply(cmd(
            "admit",
            StoreOperation::Admit {
                check: AdmissionCheck {
                    intent_id: prepared.intent_id.clone(),
                    prepared: prepared.clone(),
                    worker,
                    lease_epoch: num(1),
                    expected_tick: num(0),
                    grant_revision: num(0),
                    approval_id: None,
                    reservation_id: content_digest(b"reservation"),
                },
            },
        ))
        .unwrap();
        let make = |text: &str| {
            Receipt::new(ReceiptData {
                schema_version: SchemaVersion::V1,
                producer_receipt_id: ProducerReceiptId::new("same-producer-id").unwrap(),
                task_id: task.data().task_id.clone(),
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
                output: ReceiptOutput::Fixture {
                    bytes: BoundedText::new(text).unwrap(),
                    digest: content_digest(text.as_bytes()),
                },
            })
            .unwrap()
        };
        s.apply(cmd(
            "first",
            StoreOperation::RecordReceipt {
                receipt: make("first"),
                lease_epoch: num(1),
            },
        ))
        .unwrap();
        let result = s
            .apply(cmd(
                "late",
                StoreOperation::RecordReceipt {
                    receipt: make("different"),
                    lease_epoch: num(1),
                },
            ))
            .unwrap();
        assert!(matches!(
            result,
            WriteOutcome::Committed(StoreReply {
                result: StoreResult::ReceiptRecorded {
                    settled: false,
                    conflict: true,
                    ..
                },
                ..
            })
        ));
        let mut snap = s.audit().unwrap();
        let view = snap.projection(&task.data().task_id).unwrap();
        assert!(
            view.task_account.integrity_hold
                && view.principal_account.integrity_hold
                && view.verification.conflict
        );
        assert_eq!(view.verification.evidence_revision, num(2));
        let evidence = snap.evidence(&prepared.intent_id, None).unwrap();
        assert_eq!(evidence.records.len(), 2);
        assert!(evidence.records.iter().all(|e| e.conflict));
        assert_eq!(evidence.records.iter().filter(|e| e.selected).count(), 1);
        drop(snap);
        drop(s);
        let _ = std::fs::remove_file(p);
    }
    fn admitted_read(s: &mut SqliteStore, name: &str) -> (Task, PreparedIntent) {
        let task = make_task(name);
        let mut g = genesis();
        g.require_publication_approval = false;
        s.apply(cmd(
            &format!("create-{name}"),
            StoreOperation::CreateTask {
                task: task.clone(),
                genesis: g.clone(),
            },
        ))
        .unwrap();
        let intent = Intent {
            schema_version: SchemaVersion::V1,
            task_id: task.data().task_id.clone(),
            ordinal: 0,
            kind: IntentKind::ReadFixture,
        };
        let prepared = PreparedIntent::materialize(
            &intent,
            &task.data().principal_id,
            &journal_genesis(&task, &g).unwrap(),
        )
        .unwrap();
        let (mut state, input) = {
            let mut snapshot = s.audit().unwrap();
            (
                snapshot.projection(&task.data().task_id).unwrap().state,
                snapshot
                    .pending_inbox(&task.data().task_id)
                    .unwrap()
                    .unwrap(),
            )
        };
        state.revision = num(1);
        state.decision_steps = 1;
        state.phase = Phase::WaitingRead;
        state.pending_intent = Some(prepared.intent_id.clone());
        s.apply(cmd(
            &format!("transition-{name}"),
            StoreOperation::CommitTransition {
                task_id: task.data().task_id.clone(),
                expected_revision: num(0),
                inbox_id: input.id,
                inbox_digest: input.digest,
                transition: Transition::Decision {
                    next: state,
                    intent,
                },
            },
        ))
        .unwrap();
        s.apply(cmd(
            &format!("prepare-{name}"),
            StoreOperation::PrepareIntent {
                task_id: task.data().task_id.clone(),
                prepared: prepared.clone(),
            },
        ))
        .unwrap();
        let worker = WorkerId::new("w").unwrap();
        s.apply(cmd(
            &format!("claim-{name}"),
            StoreOperation::Claim {
                task_id: task.data().task_id.clone(),
                intent_id: prepared.intent_id.clone(),
                worker: worker.clone(),
                expected_epoch: num(0),
            },
        ))
        .unwrap();
        s.apply(cmd(
            &format!("admit-{name}"),
            StoreOperation::Admit {
                check: AdmissionCheck {
                    intent_id: prepared.intent_id.clone(),
                    prepared: prepared.clone(),
                    worker,
                    lease_epoch: num(1),
                    expected_tick: num(0),
                    grant_revision: num(0),
                    approval_id: None,
                    reservation_id: content_digest(name.as_bytes()),
                },
            },
        ))
        .unwrap();
        (task, prepared)
    }
    fn receipt(task: &Task, prepared: &PreparedIntent, id: &str, usage: ActualUsage) -> Receipt {
        let text = "{\"values\":[17,-4,9]}\n";
        Receipt::new(ReceiptData {
            schema_version: SchemaVersion::V1,
            producer_receipt_id: ProducerReceiptId::new(id).unwrap(),
            task_id: task.data().task_id.clone(),
            intent_id: prepared.intent_id.clone(),
            dispatch_id: prepared.action.dispatch_id.clone(),
            action_digest: prepared.action_digest.clone(),
            usage,
            output: ReceiptOutput::Fixture {
                bytes: BoundedText::new(text).unwrap(),
                digest: content_digest(text.as_bytes()),
            },
        })
        .unwrap()
    }
    fn known() -> ActualUsage {
        ActualUsage::Known {
            usage: UsageVector {
                model_requests: DecimalUsage::new("0").unwrap(),
                tool_calls: DecimalUsage::new("1").unwrap(),
                spend_micros_usd: DecimalUsage::new("0").unwrap(),
            },
        }
    }
    #[test]
    fn matching_stale_evidence_can_be_reconciled_and_unfrozen() {
        let (mut s, p) = store();
        let (task, prepared) = admitted_read(&mut s, "stale");
        s.apply(cmd(
            "tick",
            StoreOperation::AdvanceTick {
                expected_tick: num(0),
                new_tick: num(30),
            },
        ))
        .unwrap();
        let evidence = receipt(&task, &prepared, "late", known());
        let id = evidence.evidence_id().unwrap();
        s.apply(cmd(
            "receipt",
            StoreOperation::RecordReceipt {
                receipt: evidence,
                lease_epoch: num(1),
            },
        ))
        .unwrap();
        s.apply(cmd(
            "uncertain",
            StoreOperation::RecordUncertainty {
                task_id: task.data().task_id.clone(),
                intent_id: prepared.intent_id.clone(),
                dispatch_id: prepared.action.dispatch_id.clone(),
                lease_epoch: num(1),
                reason: BoundedText::new("expired").unwrap(),
            },
        ))
        .unwrap();
        let worker = WorkerId::new("reconciler").unwrap();
        s.apply(cmd(
            "reclaim",
            StoreOperation::ClaimReconciliation {
                task_id: task.data().task_id.clone(),
                intent_id: prepared.intent_id.clone(),
                worker: worker.clone(),
                expected_epoch: num(0),
                expected_evidence_revision: num(1),
            },
        ))
        .unwrap();
        let out = s
            .apply(cmd(
                "settle",
                StoreOperation::SettleReconciliation {
                    task_id: task.data().task_id.clone(),
                    intent_id: prepared.intent_id.clone(),
                    worker,
                    lease_epoch: num(1),
                    expected_evidence_revision: num(1),
                    evidence_id: id,
                    reconciled_receipt: None,
                },
            ))
            .unwrap();
        assert!(matches!(
            out,
            WriteOutcome::Committed(StoreReply {
                result: StoreResult::ReconciliationSettled { .. },
                ..
            })
        ));
        let mut snap = s.audit().unwrap();
        let v = snap.projection(&task.data().task_id).unwrap();
        assert!(
            !v.task_account.integrity_hold
                && !v.principal_account.integrity_hold
                && !v.verification.unresolved
        );
        assert_eq!(v.task_account.posted.tool_calls, num(1));
        assert_eq!(v.task_account.reserved.tool_calls, num(0));
        drop(snap);
        drop(s);
        let _ = std::fs::remove_file(p);
    }
    #[test]
    fn unknown_usage_cannot_be_repaired_by_new_known_receipt() {
        let (mut s, p) = store();
        let (task, prepared) = admitted_read(&mut s, "unknown");
        let original = receipt(
            &task,
            &prepared,
            "unknown-receipt",
            ActualUsage::Unknown {
                reason: BoundedText::new("unknown").unwrap(),
                evidence: None,
            },
        );
        s.apply(cmd(
            "receipt",
            StoreOperation::RecordReceipt {
                receipt: original,
                lease_epoch: num(1),
            },
        ))
        .unwrap();
        let worker = WorkerId::new("reconciler").unwrap();
        s.apply(cmd(
            "reclaim",
            StoreOperation::ClaimReconciliation {
                task_id: task.data().task_id.clone(),
                intent_id: prepared.intent_id.clone(),
                worker: worker.clone(),
                expected_epoch: num(0),
                expected_evidence_revision: num(1),
            },
        ))
        .unwrap();
        let replacement = receipt(&task, &prepared, "replacement", known());
        let id = replacement.evidence_id().unwrap();
        let out = s
            .apply(cmd(
                "settle",
                StoreOperation::SettleReconciliation {
                    task_id: task.data().task_id.clone(),
                    intent_id: prepared.intent_id.clone(),
                    worker,
                    lease_epoch: num(1),
                    expected_evidence_revision: num(1),
                    evidence_id: id,
                    reconciled_receipt: Some(replacement),
                },
            ))
            .unwrap();
        assert!(matches!(
            out,
            WriteOutcome::Committed(StoreReply {
                result: StoreResult::ReceiptRecorded { settled: false, .. },
                ..
            })
        ));
        let mut snap = s.audit().unwrap();
        let v = snap.projection(&task.data().task_id).unwrap();
        assert!(
            v.task_account.incomplete
                && v.principal_account.incomplete
                && v.verification.unresolved
        );
        assert_eq!(v.task_account.posted.tool_calls, num(0));
        assert_eq!(v.task_account.reserved.tool_calls, num(1));
        assert_eq!(
            snap.evidence(&prepared.intent_id, None)
                .unwrap()
                .records
                .len(),
            2
        );
        drop(snap);
        drop(s);
        let _ = std::fs::remove_file(p);
    }
    #[test]
    fn stale_overrun_reconciliation_posts_complete_vector_and_breaches() {
        let (mut s, p) = store();
        let (task, prepared) = admitted_read(&mut s, "overrun");
        s.apply(cmd(
            "tick",
            StoreOperation::AdvanceTick {
                expected_tick: num(0),
                new_tick: num(30),
            },
        ))
        .unwrap();
        let usage = UsageVector {
            model_requests: DecimalUsage::new("3").unwrap(),
            tool_calls: DecimalUsage::new("2").unwrap(),
            spend_micros_usd: DecimalUsage::new("1").unwrap(),
        };
        let evidence = receipt(
            &task,
            &prepared,
            "late-overrun",
            ActualUsage::Known { usage },
        );
        let id = evidence.evidence_id().unwrap();
        s.apply(cmd(
            "receipt",
            StoreOperation::RecordReceipt {
                receipt: evidence,
                lease_epoch: num(1),
            },
        ))
        .unwrap();
        s.apply(cmd(
            "uncertain",
            StoreOperation::RecordUncertainty {
                task_id: task.data().task_id.clone(),
                intent_id: prepared.intent_id.clone(),
                dispatch_id: prepared.action.dispatch_id.clone(),
                lease_epoch: num(1),
                reason: BoundedText::new("expired").unwrap(),
            },
        ))
        .unwrap();
        let worker = WorkerId::new("reconciler").unwrap();
        s.apply(cmd(
            "reclaim",
            StoreOperation::ClaimReconciliation {
                task_id: task.data().task_id.clone(),
                intent_id: prepared.intent_id.clone(),
                worker: worker.clone(),
                expected_epoch: num(0),
                expected_evidence_revision: num(1),
            },
        ))
        .unwrap();
        let result = s
            .apply(cmd(
                "settle",
                StoreOperation::SettleReconciliation {
                    task_id: task.data().task_id.clone(),
                    intent_id: prepared.intent_id.clone(),
                    worker,
                    lease_epoch: num(1),
                    expected_evidence_revision: num(1),
                    evidence_id: id,
                    reconciled_receipt: None,
                },
            ))
            .unwrap();
        assert!(matches!(
            result,
            WriteOutcome::Committed(StoreReply {
                result: StoreResult::ReconciliationSettled { .. },
                ..
            })
        ));
        let mut snap = s.audit().unwrap();
        let v = snap.projection(&task.data().task_id).unwrap();
        for account in [&v.task_account, &v.principal_account] {
            assert!(account.breached);
            assert_eq!(account.posted.model_requests, num(3));
            assert_eq!(account.posted.tool_calls, num(2));
            assert_eq!(account.posted.spend_micros_usd, num(1));
            assert_eq!(account.reserved.tool_calls, num(0));
        }
        drop(snap);
        drop(s);
        let _ = std::fs::remove_file(p);
    }
    fn submit_next(
        s: &mut SqliteStore,
        task: &Task,
        kind: IntentKind,
        phase: Phase,
        tag: &str,
    ) -> PreparedIntent {
        let (mut state, input) = {
            let mut snap = s.audit().unwrap();
            (
                snap.projection(&task.data().task_id).unwrap().state,
                snap.pending_inbox(&task.data().task_id).unwrap().unwrap(),
            )
        };
        let intent = Intent {
            schema_version: SchemaVersion::V1,
            task_id: task.data().task_id.clone(),
            ordinal: u32::from(state.decision_steps),
            kind,
        };
        let prepared =
            PreparedIntent::materialize(&intent, &task.data().principal_id, &state.genesis_digest)
                .unwrap();
        let expected = state.revision;
        state.revision = state.revision.checked_add(1).unwrap();
        state.decision_steps += 1;
        state.phase = phase;
        state.pending_intent = Some(prepared.intent_id.clone());
        s.apply(cmd(
            &format!("transition-{tag}"),
            StoreOperation::CommitTransition {
                task_id: task.data().task_id.clone(),
                expected_revision: expected,
                inbox_id: input.id,
                inbox_digest: input.digest,
                transition: Transition::Decision {
                    next: state,
                    intent,
                },
            },
        ))
        .unwrap();
        s.apply(cmd(
            &format!("prepare-{tag}"),
            StoreOperation::PrepareIntent {
                task_id: task.data().task_id.clone(),
                prepared: prepared.clone(),
            },
        ))
        .unwrap();
        let worker = WorkerId::new(format!("worker-{tag}")).unwrap();
        s.apply(cmd(
            &format!("claim-{tag}"),
            StoreOperation::Claim {
                task_id: task.data().task_id.clone(),
                intent_id: prepared.intent_id.clone(),
                worker: worker.clone(),
                expected_epoch: num(0),
            },
        ))
        .unwrap();
        let current_tick = {
            let mut snapshot = s.audit().unwrap();
            snapshot.projection(&task.data().task_id).unwrap().tick
        };
        s.apply(cmd(
            &format!("admit-{tag}"),
            StoreOperation::Admit {
                check: AdmissionCheck {
                    intent_id: prepared.intent_id.clone(),
                    prepared: prepared.clone(),
                    worker,
                    lease_epoch: num(1),
                    expected_tick: current_tick,
                    grant_revision: num(0),
                    approval_id: None,
                    reservation_id: content_digest(tag.as_bytes()),
                },
            },
        ))
        .unwrap();
        prepared
    }
    fn output_receipt(
        task: &Task,
        prepared: &PreparedIntent,
        id: &str,
        output: ReceiptOutput,
        model: bool,
    ) -> Receipt {
        Receipt::new(ReceiptData {
            schema_version: SchemaVersion::V1,
            producer_receipt_id: ProducerReceiptId::new(id).unwrap(),
            task_id: task.data().task_id.clone(),
            intent_id: prepared.intent_id.clone(),
            dispatch_id: prepared.action.dispatch_id.clone(),
            action_digest: prepared.action_digest.clone(),
            usage: ActualUsage::Known {
                usage: UsageVector {
                    model_requests: DecimalUsage::new(if model { "1" } else { "0" }).unwrap(),
                    tool_calls: DecimalUsage::new(if model { "0" } else { "1" }).unwrap(),
                    spend_micros_usd: DecimalUsage::new("0").unwrap(),
                },
            },
            output,
        })
        .unwrap()
    }
    fn prepare_verification(
        s: &mut SqliteStore,
        record: bool,
        overrun: bool,
    ) -> (Task, Receipt, VerificationSelection) {
        let (task, read) = admitted_read(s, "verify-task");
        let read_receipt = receipt(&task, &read, "read-receipt", known());
        let read_id = read_receipt.evidence_id().unwrap();
        s.apply(cmd(
            "read-receipt",
            StoreOperation::RecordReceipt {
                receipt: read_receipt.clone(),
                lease_epoch: num(1),
            },
        ))
        .unwrap();
        let fixture = "{\"values\":[17,-4,9]}\n";
        let model = submit_next(
            s,
            &task,
            IntentKind::RequestModel {
                input_digest: content_digest(fixture.as_bytes()),
                context: BoundedText::new(fixture).unwrap(),
            },
            Phase::WaitingModel,
            "model",
        );
        let candidate = CandidateResult {
            count: 3,
            sum: 22,
            input_digest: content_digest(fixture.as_bytes()),
        };
        let model_receipt = output_receipt(
            &task,
            &model,
            "model-receipt",
            ReceiptOutput::Model {
                candidate: candidate.clone(),
            },
            true,
        );
        let model_id = model_receipt.evidence_id().unwrap();
        s.apply(cmd(
            "model-receipt",
            StoreOperation::RecordReceipt {
                receipt: model_receipt,
                lease_epoch: num(1),
            },
        ))
        .unwrap();
        let publication = submit_next(
            s,
            &task,
            IntentKind::PublishArtifact { candidate },
            Phase::WaitingArtifact,
            "publish",
        );
        let ApprovedRequest::PublishArtifact { plan } = &publication.request else {
            panic!("publication plan")
        };
        let pub_receipt = output_receipt(
            &task,
            &publication,
            "publication-receipt",
            ReceiptOutput::Published {
                artifact_id: plan.artifact_id.clone(),
                content_digest: plan.content_digest.clone(),
                content_length: plan.content_length,
            },
            false,
        );
        let pub_receipt = if overrun {
            let mut data = pub_receipt.data().clone();
            data.usage = ActualUsage::Known {
                usage: UsageVector {
                    model_requests: DecimalUsage::new("0").unwrap(),
                    tool_calls: DecimalUsage::new("2").unwrap(),
                    spend_micros_usd: DecimalUsage::new("0").unwrap(),
                },
            };
            Receipt::new(data).unwrap()
        } else {
            pub_receipt
        };
        let pub_id = pub_receipt.evidence_id().unwrap();
        s.apply(cmd(
            "publication-receipt",
            StoreOperation::RecordReceipt {
                receipt: pub_receipt,
                lease_epoch: num(1),
            },
        ))
        .unwrap();
        let (mut state, input) = {
            let mut snap = s.audit().unwrap();
            (
                snap.projection(&task.data().task_id).unwrap().state,
                snap.pending_inbox(&task.data().task_id).unwrap().unwrap(),
            )
        };
        let revision = state.revision;
        state.revision = state.revision.checked_add(1).unwrap();
        state.phase = Phase::AwaitingVerification;
        state.pending_intent = None;
        s.apply(cmd(
            "publication-finalize",
            StoreOperation::CommitTransition {
                task_id: task.data().task_id.clone(),
                expected_revision: revision,
                inbox_id: input.id,
                inbox_digest: input.digest,
                transition: Transition::Finalization { next: state },
            },
        ))
        .unwrap();
        let selection = VerificationSelection {
            evidence_revision: num(3),
            read_evidence: read_id,
            model_evidence: model_id,
            publication_evidence: pub_id,
            plan_digest: publication.request_digest.clone(),
        };
        if record {
            s.apply(cmd(
                "verification",
                StoreOperation::RecordVerification {
                    task_id: task.data().task_id.clone(),
                    selection: selection.clone(),
                    accepted: true,
                    reason: BoundedText::new("accepted").unwrap(),
                },
            ))
            .unwrap();
        }
        (task, read_receipt, selection)
    }
    fn late_conflict(s: &mut SqliteStore, read: &Receipt) {
        let mut data = read.data().clone();
        data.usage = ActualUsage::Known {
            usage: UsageVector {
                model_requests: DecimalUsage::new("0").unwrap(),
                tool_calls: DecimalUsage::new("2").unwrap(),
                spend_micros_usd: DecimalUsage::new("0").unwrap(),
            },
        };
        let changed = Receipt::new(data).unwrap();
        s.apply(cmd(
            "late-conflict",
            StoreOperation::RecordReceipt {
                receipt: changed,
                lease_epoch: num(1),
            },
        ))
        .unwrap();
    }
    #[test]
    fn late_conflict_supersedes_unconsumed_verification_atomically() {
        let (mut s, p) = store();
        let (task, read, _) = prepare_verification(&mut s, true, false);
        let input = {
            let mut snap = s.audit().unwrap();
            snap.pending_inbox(&task.data().task_id).unwrap().unwrap()
        };
        late_conflict(&mut s, &read);
        let mut snap = s.audit().unwrap();
        let page = snap.inbox(&task.data().task_id, None).unwrap();
        let verified = page
            .records
            .iter()
            .find(|i| i.input.id == input.id)
            .unwrap();
        assert_eq!(verified.status, InboxStatus::Superseded);
        let v = snap.projection(&task.data().task_id).unwrap();
        assert!(
            v.task_account.integrity_hold
                && v.principal_account.integrity_hold
                && v.verification.conflict
        );
        assert_eq!(v.state.phase, Phase::AwaitingVerification);
        assert_eq!(v.verification.evidence_revision, num(4));
        drop(snap);
        drop(s);
        let _ = std::fs::remove_file(p);
    }
    #[test]
    fn consumed_completion_remains_historical_under_late_hold() {
        let (mut s, p) = store();
        let (task, read, _) = prepare_verification(&mut s, true, false);
        let (mut state, input) = {
            let mut snap = s.audit().unwrap();
            (
                snap.projection(&task.data().task_id).unwrap().state,
                snap.pending_inbox(&task.data().task_id).unwrap().unwrap(),
            )
        };
        let revision = state.revision;
        state.revision = state.revision.checked_add(1).unwrap();
        state.phase = Phase::Completed;
        s.apply(cmd(
            "complete",
            StoreOperation::CommitTransition {
                task_id: task.data().task_id.clone(),
                expected_revision: revision,
                inbox_id: input.id,
                inbox_digest: input.digest,
                transition: Transition::Finalization { next: state },
            },
        ))
        .unwrap();
        late_conflict(&mut s, &read);
        let mut snap = s.audit().unwrap();
        let v = snap.projection(&task.data().task_id).unwrap();
        assert_eq!(v.state.phase, Phase::Completed);
        assert!(
            v.task_account.integrity_hold
                && v.principal_account.integrity_hold
                && v.verification.conflict
                && v.verification.unresolved
        );
        let invalidated: i64 = snap
            .tx
            .query_row(
                "SELECT invalidated_verification FROM tasks WHERE task_id=?1",
                [task.data().task_id.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(invalidated, 1);
        drop(snap);
        drop(s);
        let _ = std::fs::remove_file(p);
    }
    #[test]
    fn cross_task_receipt_collision_invalidates_both_snapshots() {
        let (mut s, p) = store();
        let (a, ai) = admitted_read(&mut s, "cross-a");
        let (b, bi) = admitted_read(&mut s, "cross-b");
        let first = receipt(&a, &ai, "shared-receipt", known());
        let second = receipt(&b, &bi, "shared-receipt", known());
        s.apply(cmd(
            "first-cross",
            StoreOperation::RecordReceipt {
                receipt: first.clone(),
                lease_epoch: num(1),
            },
        ))
        .unwrap();
        s.apply(cmd(
            "second-cross",
            StoreOperation::RecordReceipt {
                receipt: second.clone(),
                lease_epoch: num(1),
            },
        ))
        .unwrap();
        let retry = cmd(
            "old-receipt-new-id",
            StoreOperation::RecordReceipt {
                receipt: first,
                lease_epoch: num(1),
            },
        );
        let first_reply = s.apply(retry.clone()).unwrap();
        assert!(
            matches!(first_reply,WriteOutcome::Committed(StoreReply { result:StoreResult::ReceiptRecorded { evidence_revision,settled:true,conflict:true,.. }, disposition:Disposition::PreviouslyCommitted }) if evidence_revision==num(2))
        );
        let duplicate = s.apply(retry).unwrap();
        assert!(
            matches!(duplicate,WriteOutcome::Committed(StoreReply { result:StoreResult::ReceiptRecorded { evidence_revision,settled:true,conflict:true,.. }, disposition:Disposition::Duplicate }) if evidence_revision==num(2))
        );
        let mut third_data = second.data().clone();
        third_data.producer_receipt_id = ProducerReceiptId::new("independent-producer").unwrap();
        let third = Receipt::new(third_data).unwrap();
        let outcome = s
            .apply(cmd(
                "third-cross",
                StoreOperation::RecordReceipt {
                    receipt: third,
                    lease_epoch: num(1),
                },
            ))
            .unwrap();
        assert!(matches!(
            outcome,
            WriteOutcome::Committed(StoreReply {
                result: StoreResult::ReceiptRecorded {
                    settled: false,
                    conflict: true,
                    ..
                },
                ..
            })
        ));
        let mut snap = s.audit().unwrap();
        let av = snap.projection(&a.data().task_id).unwrap();
        let bv = snap.projection(&b.data().task_id).unwrap();
        assert!(
            av.task_account.integrity_hold
                && bv.task_account.integrity_hold
                && av.principal_account.integrity_hold
        );
        assert!(av.verification.conflict && bv.verification.conflict);
        assert_eq!(av.verification.evidence_revision, num(2));
        assert_eq!(bv.verification.evidence_revision, num(2));
        assert_eq!(
            snap.intent(&ai.intent_id)
                .unwrap()
                .unwrap()
                .evidence_revision,
            num(2)
        );
        assert!(snap.evidence(&ai.intent_id, None).unwrap().records[0].conflict);
        assert!(snap.evidence(&bi.intent_id, None).unwrap().records[0].conflict);
        drop(snap);
        drop(s);
        let _ = std::fs::remove_file(p);
    }
    #[test]
    fn new_evidence_invalidates_reconciliation_snapshot_without_losing_receipt() {
        let (mut s, p) = store();
        let (task, prepared) = admitted_read(&mut s, "evidence-race");
        s.apply(cmd(
            "tick",
            StoreOperation::AdvanceTick {
                expected_tick: num(0),
                new_tick: num(30),
            },
        ))
        .unwrap();
        s.apply(cmd(
            "uncertain",
            StoreOperation::RecordUncertainty {
                task_id: task.data().task_id.clone(),
                intent_id: prepared.intent_id.clone(),
                dispatch_id: prepared.action.dispatch_id.clone(),
                lease_epoch: num(1),
                reason: BoundedText::new("expired").unwrap(),
            },
        ))
        .unwrap();
        let worker = WorkerId::new("reconciler").unwrap();
        s.apply(cmd(
            "reclaim",
            StoreOperation::ClaimReconciliation {
                task_id: task.data().task_id.clone(),
                intent_id: prepared.intent_id.clone(),
                worker: worker.clone(),
                expected_epoch: num(0),
                expected_evidence_revision: num(0),
            },
        ))
        .unwrap();
        let late = receipt(&task, &prepared, "late", known());
        let id = late.evidence_id().unwrap();
        s.apply(cmd(
            "late",
            StoreOperation::RecordReceipt {
                receipt: late,
                lease_epoch: num(1),
            },
        ))
        .unwrap();
        let result = s
            .apply(cmd(
                "stale-resolution",
                StoreOperation::SettleReconciliation {
                    task_id: task.data().task_id.clone(),
                    intent_id: prepared.intent_id.clone(),
                    worker,
                    lease_epoch: num(1),
                    expected_evidence_revision: num(0),
                    evidence_id: id,
                    reconciled_receipt: None,
                },
            ))
            .unwrap();
        assert_eq!(
            result,
            WriteOutcome::NotCommitted(NotCommitted::EvidenceChanged)
        );
        let mut snap = s.audit().unwrap();
        let projection = snap.projection(&task.data().task_id).unwrap();
        assert_eq!(projection.task_account.posted.tool_calls, num(0));
        assert_eq!(projection.task_account.reserved.tool_calls, num(1));
        assert!(projection.task_account.integrity_hold);
        assert_eq!(
            snap.evidence(&prepared.intent_id, None)
                .unwrap()
                .records
                .len(),
            1
        );
        drop(snap);
        drop(s);
        let _ = std::fs::remove_file(p);
    }
    #[test]
    fn readonly_audit_rejects_tampered_dedup_and_evidence_indexes() {
        for mode in ["digest", "result", "producer"] {
            let (mut s, p) = store();
            let (task, prepared) = admitted_read(&mut s, &format!("tamper-{mode}"));
            let r = receipt(&task, &prepared, "producer", known());
            s.apply(cmd(
                "receipt",
                StoreOperation::RecordReceipt {
                    receipt: r,
                    lease_epoch: num(1),
                },
            ))
            .unwrap();
            match mode {
                "digest" => {
                    s.connection
                        .execute(
                            "UPDATE commands SET digest=?1 WHERE rowid=1",
                            ["0".repeat(64)],
                        )
                        .unwrap();
                }
                "result" => {
                    let wrong = canonical_bytes(&StoreResult::Denied, MAX_INTERNAL).unwrap();
                    s.connection
                        .execute("UPDATE commands SET result=?1 WHERE rowid=1", [wrong])
                        .unwrap();
                }
                "producer" => {
                    s.connection
                        .execute("UPDATE evidence SET producer_id='forged'", [])
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let mut readonly = SqliteStore::open_read_only(&p).unwrap();
            assert!(
                matches!(readonly.audit(), Err(StoreError::Integrity)),
                "mode {mode}"
            );
            drop(readonly);
            drop(s);
            let _ = std::fs::remove_file(p);
        }
    }
    #[test]
    fn stale_reconciler_cannot_settle_live_dispatch_with_matching_epoch() {
        let (mut s, p) = store();
        let (task, prepared) = admitted_read(&mut s, "forged-resolver");
        let r = receipt(&task, &prepared, "untrusted-resolver", known());
        let id = r.evidence_id().unwrap();
        let out = s
            .apply(cmd(
                "bad-resolution",
                StoreOperation::SettleReconciliation {
                    task_id: task.data().task_id.clone(),
                    intent_id: prepared.intent_id.clone(),
                    worker: WorkerId::new("not-claimed").unwrap(),
                    lease_epoch: num(1),
                    expected_evidence_revision: num(0),
                    evidence_id: id,
                    reconciled_receipt: Some(r),
                },
            ))
            .unwrap();
        assert!(matches!(
            out,
            WriteOutcome::Committed(StoreReply {
                result: StoreResult::ReceiptRecorded { settled: false, .. },
                ..
            })
        ));
        let fresh = receipt(&task, &prepared, "later-original-worker", known());
        let result = s
            .apply(cmd(
                "later-receipt",
                StoreOperation::RecordReceipt {
                    receipt: fresh,
                    lease_epoch: num(1),
                },
            ))
            .unwrap();
        assert!(matches!(
            result,
            WriteOutcome::Committed(StoreReply {
                result: StoreResult::ReceiptRecorded { settled: false, .. },
                ..
            })
        ));
        let mut snap = s.audit().unwrap();
        let state = snap.projection(&task.data().task_id).unwrap();
        assert_eq!(state.task_account.posted.tool_calls, num(0));
        assert_eq!(state.task_account.reserved.tool_calls, num(1));
        assert!(state.task_account.integrity_hold);
        assert_eq!(
            snap.evidence(&prepared.intent_id, None)
                .unwrap()
                .records
                .len(),
            2
        );
        drop(snap);
        drop(s);
        let _ = std::fs::remove_file(p);
    }
    #[test]
    fn readonly_audit_reports_resource_limits_without_domain_mutation() {
        let (mut s, p) = store();
        s.connection.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<4097) INSERT INTO journal(seq,payload) SELECT x,x'7b7d' FROM n").unwrap();
        let mut ro = SqliteStore::open_read_only(&p).unwrap();
        assert!(
            matches!(ro.audit(),Err(StoreError::Operational(text)) if text.as_str()=="audit resource limit exceeded")
        );
        drop(ro);
        // Writer snapshots do not impose the audit history ceiling.
        assert!(s.audit().is_ok());
        s.connection
            .execute("DELETE FROM journal WHERE seq=4097", [])
            .unwrap();
        let mut boundary = SqliteStore::open_read_only(&p).unwrap();
        assert!(matches!(boundary.audit(), Err(StoreError::Integrity))); // 4096 reaches representation checks.
        drop(boundary);
        s.connection.execute_batch("DELETE FROM journal; INSERT INTO journal(seq,payload) VALUES(1,zeroblob(8388609));").unwrap();
        let mut ro = SqliteStore::open_read_only(&p).unwrap();
        assert!(
            matches!(ro.audit(),Err(StoreError::Operational(text)) if text.as_str()=="audit resource limit exceeded")
        );
        drop(ro);
        s.connection.execute_batch("DELETE FROM journal; INSERT INTO journal(seq,payload) VALUES(1,zeroblob(8388608));").unwrap();
        let mut boundary = SqliteStore::open_read_only(&p).unwrap();
        assert!(matches!(boundary.audit(), Err(StoreError::Integrity))); // 8 MiB reaches representation checks.
        drop(boundary);
        drop(s);
        let _ = std::fs::remove_file(p);
    }
    #[test]
    fn terminal_transition_with_pending_intent_is_rejected_without_mutation() {
        let (mut s, p) = store();
        let (task, _, _) = prepare_verification(&mut s, true, false);
        let (state, input, head) = {
            let mut snap = s.audit().unwrap();
            let view = snap.projection(&task.data().task_id).unwrap();
            (
                view.state,
                snap.pending_inbox(&task.data().task_id).unwrap().unwrap(),
                view.replay_head,
            )
        };
        let mut forged = state.clone();
        forged.revision = forged.revision.checked_add(1).unwrap();
        forged.phase = Phase::Completed;
        forged.pending_intent = Some(content_digest(b"forged"));
        let result = s.apply(cmd(
            "bad-terminal",
            StoreOperation::CommitTransition {
                task_id: task.data().task_id.clone(),
                expected_revision: state.revision,
                inbox_id: input.id.clone(),
                inbox_digest: input.digest.clone(),
                transition: Transition::Finalization { next: forged },
            },
        ));
        assert!(matches!(result, Err(StoreError::Validation(_))));
        let mut snap = s.audit().unwrap();
        let after = snap.projection(&task.data().task_id).unwrap();
        assert_eq!(after.state, state);
        assert_eq!(after.replay_head, head);
        assert_eq!(
            snap.pending_inbox(&task.data().task_id).unwrap().unwrap(),
            input
        );
        drop(snap);
        drop(s);
        let _ = std::fs::remove_file(p);
    }
    #[test]
    fn failed_effect_cannot_finalize_with_pending_intent() {
        let (mut s, p) = store();
        let (task, prepared) = admitted_read(&mut s, "bad-failure");
        let failed = output_receipt(
            &task,
            &prepared,
            "failed-receipt",
            ReceiptOutput::Failure {
                diagnostic: BoundedText::new("failed").unwrap(),
            },
            false,
        );
        s.apply(cmd(
            "failure",
            StoreOperation::RecordReceipt {
                receipt: failed,
                lease_epoch: num(1),
            },
        ))
        .unwrap();
        let (state, input, head) = {
            let mut snap = s.audit().unwrap();
            let view = snap.projection(&task.data().task_id).unwrap();
            (
                view.state,
                snap.pending_inbox(&task.data().task_id).unwrap().unwrap(),
                view.replay_head,
            )
        };
        let mut forged = state.clone();
        forged.revision = forged.revision.checked_add(1).unwrap();
        forged.phase = Phase::Rejected;
        let result = s.apply(cmd(
            "bad-failure-transition",
            StoreOperation::CommitTransition {
                task_id: task.data().task_id.clone(),
                expected_revision: state.revision,
                inbox_id: input.id.clone(),
                inbox_digest: input.digest.clone(),
                transition: Transition::Finalization { next: forged },
            },
        ));
        assert!(matches!(result, Err(StoreError::Validation(_))));
        let mut snap = s.audit().unwrap();
        let after = snap.projection(&task.data().task_id).unwrap();
        assert_eq!(after.state, state);
        assert_eq!(after.replay_head, head);
        assert_eq!(
            snap.pending_inbox(&task.data().task_id).unwrap().unwrap(),
            input
        );
        drop(snap);
        drop(s);
        let _ = std::fs::remove_file(p);
    }
    #[test]
    fn readonly_audit_rejects_nonpositive_pagination_keys() {
        for table in ["intents", "inbox", "evidence", "unposted_debits"] {
            for invalid in [0_i64, -1] {
                let (mut s, p) = store();
                let (task, prepared) = admitted_read(&mut s, &format!("hidden-{table}"));
                let r = receipt(&task, &prepared, "producer", known());
                s.apply(cmd(
                    "receipt",
                    StoreOperation::RecordReceipt {
                        receipt: r,
                        lease_epoch: num(1),
                    },
                ))
                .unwrap();
                s.connection
                    .pragma_update(None, "ignore_check_constraints", true)
                    .unwrap();
                if table == "unposted_debits" {
                    s.connection.execute("INSERT INTO unposted_debits(row_id,task_id,dispatch_id,value) VALUES(?1,?2,?3,x'7b7d')",params![invalid,task.data().task_id.as_str(),prepared.action.dispatch_id.as_str()]).unwrap();
                } else {
                    s.connection.execute(&format!("UPDATE {table} SET row_id=?1 WHERE row_id=(SELECT min(row_id) FROM {table})"),[invalid]).unwrap();
                }
                let mut ro = SqliteStore::open_read_only(&p).unwrap();
                assert!(
                    matches!(ro.audit(), Err(StoreError::Integrity)),
                    "table {table}, row {invalid}"
                );
                drop(ro);
                drop(s);
                let _ = std::fs::remove_file(p);
            }
        }
    }
    #[test]
    fn publication_overrun_cannot_record_verification() {
        let (mut s, p) = store();
        let (task, _, selection) = prepare_verification(&mut s, false, true);
        let (state, head) = {
            let mut snap = s.audit().unwrap();
            let v = snap.projection(&task.data().task_id).unwrap();
            assert!(v.task_account.breached && v.principal_account.breached);
            assert_eq!(v.state.phase, Phase::AwaitingVerification);
            assert!(snap.pending_inbox(&task.data().task_id).unwrap().is_none());
            (v.state, v.replay_head)
        };
        let result = s
            .apply(cmd(
                "verification-after-overrun",
                StoreOperation::RecordVerification {
                    task_id: task.data().task_id.clone(),
                    selection,
                    accepted: true,
                    reason: BoundedText::new("accepted").unwrap(),
                },
            ))
            .unwrap();
        assert_eq!(
            result,
            WriteOutcome::NotCommitted(NotCommitted::EvidenceChanged)
        );
        let mut snap = s.audit().unwrap();
        let after = snap.projection(&task.data().task_id).unwrap();
        assert_eq!(after.state, state);
        assert_eq!(after.replay_head, head);
        assert!(snap.pending_inbox(&task.data().task_id).unwrap().is_none());
        drop(snap);
        drop(s);
        let _ = std::fs::remove_file(p);
    }
    #[test]
    fn reconciliation_cannot_settle_another_intents_retained_receipt() {
        let (mut s, p) = store();
        let (a, ai) = admitted_read(&mut s, "lease-a");
        let (b, bi) = admitted_read(&mut s, "lease-b");
        s.apply(cmd(
            "tick",
            StoreOperation::AdvanceTick {
                expected_tick: num(0),
                new_tick: num(30),
            },
        ))
        .unwrap();
        for (task, intent, tag) in [(&a, &ai, "a"), (&b, &bi, "b")] {
            s.apply(cmd(
                &format!("uncertain-{tag}"),
                StoreOperation::RecordUncertainty {
                    task_id: task.data().task_id.clone(),
                    intent_id: intent.intent_id.clone(),
                    dispatch_id: intent.action.dispatch_id.clone(),
                    lease_epoch: num(1),
                    reason: BoundedText::new("expired").unwrap(),
                },
            ))
            .unwrap();
        }
        let b_receipt = receipt(&b, &bi, "receipt-b", known());
        let evidence = b_receipt.evidence_id().unwrap();
        s.apply(cmd(
            "receipt-b",
            StoreOperation::RecordReceipt {
                receipt: b_receipt,
                lease_epoch: num(1),
            },
        ))
        .unwrap();
        let wa = WorkerId::new("resolver-a").unwrap();
        let wb = WorkerId::new("resolver-b").unwrap();
        for (task, intent, worker, rev, tag) in
            [(&a, &ai, &wa, num(0), "a"), (&b, &bi, &wb, num(1), "b")]
        {
            s.apply(cmd(
                &format!("claim-recon-{tag}"),
                StoreOperation::ClaimReconciliation {
                    task_id: task.data().task_id.clone(),
                    intent_id: intent.intent_id.clone(),
                    worker: worker.clone(),
                    expected_epoch: num(0),
                    expected_evidence_revision: rev,
                },
            ))
            .unwrap();
        }
        let head = {
            let mut snap = s.audit().unwrap();
            snap.projection(&a.data().task_id).unwrap().replay_head
        };
        let result = s.apply(cmd(
            "cross-settle",
            StoreOperation::SettleReconciliation {
                task_id: a.data().task_id.clone(),
                intent_id: ai.intent_id.clone(),
                worker: wa,
                lease_epoch: num(1),
                expected_evidence_revision: num(0),
                evidence_id: evidence,
                reconciled_receipt: None,
            },
        ));
        assert!(matches!(result, Err(StoreError::Integrity)));
        let mut snap = s.audit().unwrap();
        let av = snap.projection(&a.data().task_id).unwrap();
        let bv = snap.projection(&b.data().task_id).unwrap();
        assert_eq!(av.replay_head, head);
        assert_eq!(av.task_account.reserved.tool_calls, num(1));
        assert_eq!(bv.task_account.reserved.tool_calls, num(1));
        assert_eq!(bv.task_account.posted.tool_calls, num(0));
        assert_eq!(
            snap.intent(&bi.intent_id).unwrap().unwrap().status,
            DispatchStatus::Uncertain
        );
        drop(snap);
        drop(s);
        let _ = std::fs::remove_file(p);
    }
    #[test]
    fn late_approval_cannot_revive_a_durably_denied_intent() {
        let (mut s, p) = store();
        let task = make_task("denied-approval");
        let g = genesis();
        s.apply(cmd(
            "create",
            StoreOperation::CreateTask {
                task: task.clone(),
                genesis: g.clone(),
            },
        ))
        .unwrap();
        let intent = Intent {
            schema_version: SchemaVersion::V1,
            task_id: task.data().task_id.clone(),
            ordinal: 0,
            kind: IntentKind::ReadFixture,
        };
        let prepared = PreparedIntent::materialize(
            &intent,
            &task.data().principal_id,
            &journal_genesis(&task, &g).unwrap(),
        )
        .unwrap();
        let (mut state, input) = {
            let mut snap = s.audit().unwrap();
            (
                snap.projection(&task.data().task_id).unwrap().state,
                snap.pending_inbox(&task.data().task_id).unwrap().unwrap(),
            )
        };
        state.revision = num(1);
        state.decision_steps = 1;
        state.phase = Phase::WaitingRead;
        state.pending_intent = Some(prepared.intent_id.clone());
        s.apply(cmd(
            "transition",
            StoreOperation::CommitTransition {
                task_id: task.data().task_id.clone(),
                expected_revision: num(0),
                inbox_id: input.id,
                inbox_digest: input.digest,
                transition: Transition::Decision {
                    next: state,
                    intent,
                },
            },
        ))
        .unwrap();
        s.apply(cmd(
            "prepare",
            StoreOperation::PrepareIntent {
                task_id: task.data().task_id.clone(),
                prepared: prepared.clone(),
            },
        ))
        .unwrap();
        let request = ApprovalId::new("request").unwrap();
        s.apply(cmd(
            "approval-request",
            StoreOperation::CreateApprovalRequest {
                task_id: task.data().task_id.clone(),
                request_id: request.clone(),
                intent_id: prepared.intent_id.clone(),
                action_digest: prepared.action_digest.clone(),
            },
        ))
        .unwrap();
        s.apply(cmd(
            "deny",
            StoreOperation::Deny {
                task_id: task.data().task_id.clone(),
                intent_id: prepared.intent_id.clone(),
                reason: BoundedText::new("denied").unwrap(),
            },
        ))
        .unwrap();
        let head = {
            let mut snap = s.audit().unwrap();
            snap.projection(&task.data().task_id).unwrap().replay_head
        };
        let result = s
            .apply(cmd(
                "late-approval",
                StoreOperation::RecordApproval {
                    request_id: request,
                    action_digest: prepared.action_digest.clone(),
                    decision: ApprovalDecision::Approve,
                },
            ))
            .unwrap();
        assert_eq!(
            result,
            WriteOutcome::NotCommitted(NotCommitted::RecheckAuthority)
        );
        let mut snap = s.audit().unwrap();
        assert_eq!(
            snap.projection(&task.data().task_id).unwrap().replay_head,
            head
        );
        assert_eq!(
            snap.intent(&prepared.intent_id).unwrap().unwrap().status,
            DispatchStatus::Denied
        );
        drop(snap);
        drop(s);
        let _ = std::fs::remove_file(p);
    }
    #[test]
    fn compatible_reconciliation_resolves_prior_receipt_without_reselecting_it() {
        let (mut s, p) = store();
        let (task, prepared) = admitted_read(&mut s, "compatible");
        s.apply(cmd(
            "tick",
            StoreOperation::AdvanceTick {
                expected_tick: num(0),
                new_tick: num(30),
            },
        ))
        .unwrap();
        let r1 = receipt(&task, &prepared, "r1", known());
        s.apply(cmd(
            "stale",
            StoreOperation::RecordReceipt {
                receipt: r1,
                lease_epoch: num(1),
            },
        ))
        .unwrap();
        s.apply(cmd(
            "uncertain",
            StoreOperation::RecordUncertainty {
                task_id: task.data().task_id.clone(),
                intent_id: prepared.intent_id.clone(),
                dispatch_id: prepared.action.dispatch_id.clone(),
                lease_epoch: num(1),
                reason: BoundedText::new("expired").unwrap(),
            },
        ))
        .unwrap();
        let worker = WorkerId::new("reconciler").unwrap();
        s.apply(cmd(
            "claim-recon",
            StoreOperation::ClaimReconciliation {
                task_id: task.data().task_id.clone(),
                intent_id: prepared.intent_id.clone(),
                worker: worker.clone(),
                expected_epoch: num(0),
                expected_evidence_revision: num(1),
            },
        ))
        .unwrap();
        let r2 = receipt(&task, &prepared, "r2", known());
        let evidence = r2.evidence_id().unwrap();
        let result = s
            .apply(cmd(
                "settle",
                StoreOperation::SettleReconciliation {
                    task_id: task.data().task_id.clone(),
                    intent_id: prepared.intent_id.clone(),
                    worker,
                    lease_epoch: num(1),
                    expected_evidence_revision: num(1),
                    evidence_id: evidence,
                    reconciled_receipt: Some(r2),
                },
            ))
            .unwrap();
        assert!(matches!(
            result,
            WriteOutcome::Committed(StoreReply {
                result: StoreResult::ReconciliationSettled { .. },
                ..
            })
        ));
        let mut snap = s.audit().unwrap();
        let v = snap.projection(&task.data().task_id).unwrap();
        assert!(
            !v.task_account.integrity_hold
                && !v.principal_account.integrity_hold
                && !v.verification.unresolved
        );
        assert_eq!(v.task_account.posted.tool_calls, num(1));
        assert_eq!(v.task_account.reserved.tool_calls, num(0));
        let page = snap.evidence(&prepared.intent_id, None).unwrap();
        assert_eq!(page.records.len(), 2);
        assert_eq!(page.records.iter().filter(|r| r.selected).count(), 1);
        drop(snap);
        drop(s);
        let _ = std::fs::remove_file(p);
    }
    #[test]
    fn sequential_reconciliations_resolve_only_evidence_before_each_frontier() {
        let (mut s, p) = store();
        let (task, read) = admitted_read(&mut s, "frontiers");
        s.apply(cmd(
            "tick30",
            StoreOperation::AdvanceTick {
                expected_tick: num(0),
                new_tick: num(30),
            },
        ))
        .unwrap();
        let a1 = receipt(&task, &read, "read-r1", known());
        s.apply(cmd(
            "read-stale",
            StoreOperation::RecordReceipt {
                receipt: a1.clone(),
                lease_epoch: num(1),
            },
        ))
        .unwrap();
        s.apply(cmd(
            "read-uncertain",
            StoreOperation::RecordUncertainty {
                task_id: task.data().task_id.clone(),
                intent_id: read.intent_id.clone(),
                dispatch_id: read.action.dispatch_id.clone(),
                lease_epoch: num(1),
                reason: BoundedText::new("expired").unwrap(),
            },
        ))
        .unwrap();
        let worker = WorkerId::new("resolver").unwrap();
        s.apply(cmd(
            "read-reclaim",
            StoreOperation::ClaimReconciliation {
                task_id: task.data().task_id.clone(),
                intent_id: read.intent_id.clone(),
                worker: worker.clone(),
                expected_epoch: num(0),
                expected_evidence_revision: num(1),
            },
        ))
        .unwrap();
        let a2 = receipt(&task, &read, "read-r2", known());
        let aid = a2.evidence_id().unwrap();
        s.apply(cmd(
            "read-settle",
            StoreOperation::SettleReconciliation {
                task_id: task.data().task_id.clone(),
                intent_id: read.intent_id.clone(),
                worker: worker.clone(),
                lease_epoch: num(1),
                expected_evidence_revision: num(1),
                evidence_id: aid,
                reconciled_receipt: Some(a2),
            },
        ))
        .unwrap();
        // A later semantic no-op must not replace R1's original evidence position.
        assert!(matches!(
            s.apply(cmd(
                "read-r1-again",
                StoreOperation::RecordReceipt {
                    receipt: a1,
                    lease_epoch: num(1)
                }
            ))
            .unwrap(),
            WriteOutcome::Committed(StoreReply {
                disposition: Disposition::PreviouslyCommitted,
                ..
            })
        ));
        let (mut state, input) = {
            let mut snap = s.audit().unwrap();
            (
                snap.projection(&task.data().task_id).unwrap().state,
                snap.pending_inbox(&task.data().task_id).unwrap().unwrap(),
            )
        };
        assert!(matches!(input.event, AgentEvent::EffectUncertain { .. }));
        let revision = state.revision;
        state.revision = state.revision.checked_add(1).unwrap();
        s.apply(cmd(
            "read-uncertainty-finalize",
            StoreOperation::CommitTransition {
                task_id: task.data().task_id.clone(),
                expected_revision: revision,
                inbox_id: input.id,
                inbox_digest: input.digest,
                transition: Transition::Finalization { next: state },
            },
        ))
        .unwrap();
        let fixture = "{\"values\":[17,-4,9]}\n";
        let model = submit_next(
            &mut s,
            &task,
            IntentKind::RequestModel {
                input_digest: content_digest(fixture.as_bytes()),
                context: BoundedText::new(fixture).unwrap(),
            },
            Phase::WaitingModel,
            "frontier-model",
        );
        s.apply(cmd(
            "tick60",
            StoreOperation::AdvanceTick {
                expected_tick: num(30),
                new_tick: num(60),
            },
        ))
        .unwrap();
        let candidate = CandidateResult {
            count: 3,
            sum: 22,
            input_digest: content_digest(fixture.as_bytes()),
        };
        let b1 = output_receipt(
            &task,
            &model,
            "model-r1",
            ReceiptOutput::Model {
                candidate: candidate.clone(),
            },
            true,
        );
        s.apply(cmd(
            "model-stale",
            StoreOperation::RecordReceipt {
                receipt: b1,
                lease_epoch: num(1),
            },
        ))
        .unwrap();
        s.apply(cmd(
            "model-uncertain",
            StoreOperation::RecordUncertainty {
                task_id: task.data().task_id.clone(),
                intent_id: model.intent_id.clone(),
                dispatch_id: model.action.dispatch_id.clone(),
                lease_epoch: num(1),
                reason: BoundedText::new("expired").unwrap(),
            },
        ))
        .unwrap();
        s.apply(cmd(
            "model-reclaim",
            StoreOperation::ClaimReconciliation {
                task_id: task.data().task_id.clone(),
                intent_id: model.intent_id.clone(),
                worker: worker.clone(),
                expected_epoch: num(0),
                expected_evidence_revision: num(1),
            },
        ))
        .unwrap();
        let b2 = output_receipt(
            &task,
            &model,
            "model-r2",
            ReceiptOutput::Model { candidate },
            true,
        );
        let bid = b2.evidence_id().unwrap();
        let result = s
            .apply(cmd(
                "model-settle",
                StoreOperation::SettleReconciliation {
                    task_id: task.data().task_id.clone(),
                    intent_id: model.intent_id.clone(),
                    worker,
                    lease_epoch: num(1),
                    expected_evidence_revision: num(1),
                    evidence_id: bid,
                    reconciled_receipt: Some(b2),
                },
            ))
            .unwrap();
        assert!(matches!(
            result,
            WriteOutcome::Committed(StoreReply {
                result: StoreResult::ReconciliationSettled { .. },
                ..
            })
        ));
        {
            let mut snap = s.audit().unwrap();
            let v = snap.projection(&task.data().task_id).unwrap();
            assert!(
                !v.task_account.integrity_hold
                    && !v.principal_account.integrity_hold
                    && !v.verification.unresolved
            );
            assert_eq!(v.task_account.posted.tool_calls, num(1));
            assert_eq!(v.task_account.posted.model_requests, num(1));
        }
        let later = receipt(&task, &read, "read-r3", known());
        s.apply(cmd(
            "read-later",
            StoreOperation::RecordReceipt {
                receipt: later,
                lease_epoch: num(1),
            },
        ))
        .unwrap();
        let mut snap = s.audit().unwrap();
        let v = snap.projection(&task.data().task_id).unwrap();
        assert!(
            v.task_account.integrity_hold
                && v.principal_account.integrity_hold
                && v.verification.unresolved
        );
        assert_eq!(
            snap.evidence(&read.intent_id, None).unwrap().records.len(),
            3
        );
        drop(snap);
        drop(s);
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "metron-engine-{}-{}.db",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }
    fn num(n: u64) -> SqlU64 {
        SqlU64::new(n).unwrap()
    }
    fn cmd(id: &str, operation: StoreOperation) -> StoreCommand {
        StoreCommand::new(CommandData {
            schema_version: SchemaVersion::V1,
            command_id: CommandId::new(id).unwrap(),
            operation,
        })
        .unwrap()
    }
    fn committed(out: WriteOutcome<StoreResult>) -> StoreReply<StoreResult> {
        match out {
            WriteOutcome::Committed(value) => value,
            other => panic!("unexpected {other:?}"),
        }
    }
    #[test]
    fn read_effect_is_atomic_and_recoverable_from_inbox() {
        let path = path();
        let mut store = SqliteStore::create(&path).unwrap();
        let task = Task::new(TaskData {
            schema_version: SchemaVersion::V1,
            task_id: TaskId::new("task").unwrap(),
            principal_id: PrincipalId::new("p").unwrap(),
            objective: BoundedText::new("sum").unwrap(),
            budget: BudgetRequest {
                max_model_requests: num(2),
                max_tool_calls: num(2),
                max_spend_micros_usd: num(0),
                max_elapsed_ms: None,
            },
            max_decision_steps: 8,
        })
        .unwrap();
        let genesis = Genesis {
            coding_policy: None,
            mode: EngineMode::Agent,
            engine_version: BoundedText::new("sum-agent/v1").unwrap(),
            require_publication_approval: false,
            policy_digest: content_digest(b"policy"),
            local_profile: None,
        };
        let create = cmd(
            "create",
            StoreOperation::CreateTask {
                task: task.clone(),
                genesis,
            },
        );
        assert_eq!(
            committed(store.apply(create.clone()).unwrap()).disposition,
            Disposition::FirstCommit
        );
        assert_eq!(
            committed(store.apply(create).unwrap()).disposition,
            Disposition::Duplicate
        );
        let (state, input) = {
            let mut snapshot = store.audit().unwrap();
            (
                snapshot.projection(&task.data().task_id).unwrap().state,
                snapshot
                    .pending_inbox(&task.data().task_id)
                    .unwrap()
                    .unwrap(),
            )
        };
        let intent = Intent {
            schema_version: SchemaVersion::V1,
            task_id: task.data().task_id.clone(),
            ordinal: 0,
            kind: IntentKind::ReadFixture,
        };
        let prepared =
            PreparedIntent::materialize(&intent, &task.data().principal_id, &state.genesis_digest)
                .unwrap();
        let mut next = state.clone();
        next.revision = num(1);
        next.decision_steps = 1;
        next.phase = Phase::WaitingRead;
        next.pending_intent = Some(prepared.intent_id.clone());
        committed(
            store
                .apply(cmd(
                    "transition",
                    StoreOperation::CommitTransition {
                        task_id: task.data().task_id.clone(),
                        expected_revision: num(0),
                        inbox_id: input.id,
                        inbox_digest: input.digest,
                        transition: Transition::Decision { next, intent },
                    },
                ))
                .unwrap(),
        );
        committed(
            store
                .apply(cmd(
                    "prepare",
                    StoreOperation::PrepareIntent {
                        task_id: task.data().task_id.clone(),
                        prepared: prepared.clone(),
                    },
                ))
                .unwrap(),
        );
        let worker = WorkerId::new("worker").unwrap();
        committed(
            store
                .apply(cmd(
                    "claim",
                    StoreOperation::Claim {
                        task_id: task.data().task_id.clone(),
                        intent_id: prepared.intent_id.clone(),
                        worker: worker.clone(),
                        expected_epoch: num(0),
                    },
                ))
                .unwrap(),
        );
        let admit = cmd(
            "admit",
            StoreOperation::Admit {
                check: AdmissionCheck {
                    intent_id: prepared.intent_id.clone(),
                    prepared: prepared.clone(),
                    worker,
                    lease_epoch: num(1),
                    expected_tick: num(0),
                    grant_revision: num(0),
                    approval_id: None,
                    reservation_id: content_digest(b"reservation"),
                },
            },
        );
        assert_eq!(
            committed(store.apply(admit.clone()).unwrap()).disposition,
            Disposition::FirstCommit
        );
        let repeated_operation = admit.data().operation.clone();
        assert_eq!(
            committed(store.apply(admit).unwrap()).disposition,
            Disposition::Duplicate
        );
        assert_eq!(
            committed(
                store
                    .apply(cmd("new-admit-id", repeated_operation))
                    .unwrap()
            )
            .disposition,
            Disposition::PreviouslyCommitted
        );
        let fixture = "{\"values\":[17,-4,9]}\n";
        let receipt = Receipt::new(ReceiptData {
            schema_version: SchemaVersion::V1,
            producer_receipt_id: ProducerReceiptId::new("r1").unwrap(),
            task_id: task.data().task_id.clone(),
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
            output: ReceiptOutput::Fixture {
                bytes: BoundedText::new(fixture).unwrap(),
                digest: content_digest(fixture.as_bytes()),
            },
        })
        .unwrap();
        committed(
            store
                .apply(cmd(
                    "receipt",
                    StoreOperation::RecordReceipt {
                        receipt,
                        lease_epoch: num(1),
                    },
                ))
                .unwrap(),
        );
        drop(store);
        let mut store = SqliteStore::open_existing(&path).unwrap();
        let mut snapshot = store.audit().unwrap();
        let projection = snapshot.projection(&task.data().task_id).unwrap();
        assert_eq!(projection.task_account.posted.tool_calls, num(1));
        assert_eq!(projection.task_account.reserved.tool_calls, num(0));
        assert!(matches!(
            snapshot
                .pending_inbox(&task.data().task_id)
                .unwrap()
                .unwrap()
                .event,
            AgentEvent::EffectSettled { .. }
        ));
        drop(snapshot);
        drop(store);
        let _ = std::fs::remove_file(path);
    }
}
