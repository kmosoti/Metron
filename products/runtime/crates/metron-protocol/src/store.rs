//! Synchronous persistence contract. Implementations enforce atomicity and recheck bindings.
use crate::{domain::*, types::*};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxSource {
    TaskAccepted,
    Denial,
    Uncertainty,
    Settlement,
    Verification,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboxInput {
    pub id: Digest,
    pub task_id: TaskId,
    pub principal_id: PrincipalId,
    pub source_seq: SqlU64,
    pub output_ordinal: u32,
    pub source: InboxSource,
    pub evidence_id: Option<Digest>,
    pub event: AgentEvent,
    pub digest: Digest,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionCheck {
    pub intent_id: Digest,
    pub prepared: PreparedIntent,
    pub worker: WorkerId,
    pub lease_epoch: SqlU64,
    pub expected_tick: SqlU64,
    pub grant_revision: SqlU64,
    pub approval_id: Option<ApprovalId>,
    pub reservation_id: Digest,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationSelection {
    pub evidence_revision: SqlU64,
    pub read_evidence: Digest,
    pub model_evidence: Digest,
    pub publication_evidence: Digest,
    pub plan_digest: Digest,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StoreOperation {
    CreateTask {
        task: Task,
        genesis: Genesis,
    },
    CommitTransition {
        task_id: TaskId,
        expected_revision: SqlU64,
        inbox_id: Digest,
        inbox_digest: Digest,
        transition: Transition,
    },
    PrepareIntent {
        task_id: TaskId,
        prepared: PreparedIntent,
    },
    Claim {
        task_id: TaskId,
        intent_id: Digest,
        worker: WorkerId,
        expected_epoch: SqlU64,
    },
    AdvanceTick {
        expected_tick: SqlU64,
        new_tick: SqlU64,
    },
    Admit {
        check: AdmissionCheck,
    },
    Deny {
        task_id: TaskId,
        intent_id: Digest,
        reason: BoundedText<MAX_TEXT>,
    },
    /// Store derives expiry from its current logical tick plus
    /// `APPROVAL_TTL_TICKS`, using checked arithmetic under the write lock.
    CreateApprovalRequest {
        task_id: TaskId,
        request_id: ApprovalId,
        intent_id: Digest,
        action_digest: Digest,
    },
    /// Reject an approval when its expiry is at or before the current DB tick.
    RecordApproval {
        request_id: ApprovalId,
        action_digest: Digest,
        decision: ApprovalDecision,
    },
    RevokeGrant {
        principal_id: PrincipalId,
        expected_revision: SqlU64,
    },
    RecordUncertainty {
        task_id: TaskId,
        intent_id: Digest,
        dispatch_id: Digest,
        lease_epoch: SqlU64,
        reason: BoundedText<MAX_TEXT>,
    },
    RecordReceipt {
        receipt: Receipt,
        lease_epoch: SqlU64,
    },
    ClaimReconciliation {
        task_id: TaskId,
        intent_id: Digest,
        worker: WorkerId,
        expected_epoch: SqlU64,
        expected_evidence_revision: SqlU64,
    },
    SettleReconciliation {
        task_id: TaskId,
        intent_id: Digest,
        worker: WorkerId,
        lease_epoch: SqlU64,
        expected_evidence_revision: SqlU64,
        evidence_id: Digest,
        reconciled_receipt: Option<Receipt>,
    },
    RecordVerification {
        task_id: TaskId,
        selection: VerificationSelection,
        accepted: bool,
        reason: BoundedText<MAX_TEXT>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandData {
    pub schema_version: SchemaVersion,
    pub command_id: CommandId,
    pub operation: StoreOperation,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "CommandData", into = "CommandData")]
pub struct StoreCommand(CommandData);
impl StoreCommand {
    pub fn new(data: CommandData) -> Result<Self, ValidationError> {
        canonical_bytes(&data, MAX_INTERNAL - 1024)?;
        Ok(Self(data))
    }
    pub fn data(&self) -> &CommandData {
        &self.0
    }
    pub fn digest(&self) -> Result<Digest, ValidationError> {
        structural_digest(
            "command/v1",
            &[&canonical_bytes(self, MAX_INTERNAL - 1024)?],
        )
    }
}
impl TryFrom<CommandData> for StoreCommand {
    type Error = ValidationError;
    fn try_from(value: CommandData) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<StoreCommand> for CommandData {
    fn from(value: StoreCommand) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approve,
    Deny,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    FirstCommit,
    Duplicate,
    PreviouslyCommitted,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotCommitted {
    RevisionConflict,
    RecheckAuthority,
    LeaseConflict,
    EvidenceChanged,
    NoEligible,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreReply<T> {
    pub result: T,
    pub disposition: Disposition,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum WriteOutcome<T> {
    Committed(StoreReply<T>),
    NotCommitted(NotCommitted),
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StoreResult {
    TaskCreated {
        genesis_digest: Digest,
    },
    TransitionCommitted {
        revision: SqlU64,
        intent_id: Option<Digest>,
    },
    Prepared {
        intent_id: Digest,
    },
    Claimed {
        lease_epoch: SqlU64,
        expires_at: SqlU64,
    },
    TickAdvanced {
        tick: SqlU64,
    },
    Admitted {
        dispatch_id: Digest,
    },
    Denied,
    ApprovalRequestCreated,
    ApprovalRecorded,
    GrantRevoked {
        revision: SqlU64,
    },
    UncertaintyRecorded,
    ReceiptRecorded {
        evidence_id: Digest,
        evidence_revision: SqlU64,
        settled: bool,
        conflict: bool,
    },
    ReconciliationClaimed {
        lease_epoch: SqlU64,
        expires_at: SqlU64,
    },
    ReconciliationSettled {
        evidence_id: Digest,
    },
    VerificationRecorded,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreError {
    Validation(ValidationError),
    Integrity,
    /// The write definitely did not commit. Retry the same immutable command.
    Busy,
    /// An operational failure may leave the commit acknowledgement ambiguous.
    /// Retry the same immutable command; lookup or duplicate recovery must never
    /// mint authority, including after an ambiguous admission acknowledgement.
    Operational(BoundedText<MAX_TEXT>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineMode {
    Agent,
    Harness,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Genesis {
    pub mode: EngineMode,
    pub engine_version: BoundedText<128>,
    pub require_publication_approval: bool,
    pub policy_digest: Digest,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counters {
    pub model_requests: SqlU64,
    pub tool_calls: SqlU64,
    pub spend_micros_usd: SqlU64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountSnapshot {
    pub limit: Counters,
    pub posted: Counters,
    pub reserved: Counters,
    pub breached: bool,
    pub incomplete: bool,
    pub integrity_hold: bool,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationSnapshot {
    pub evidence_revision: SqlU64,
    pub read: Option<Receipt>,
    pub model: Option<Receipt>,
    pub publication: Option<Receipt>,
    pub read_evidence: Option<Digest>,
    pub model_evidence: Option<Digest>,
    pub publication_evidence: Option<Digest>,
    pub plan: Option<ArtifactPlan>,
    pub unresolved: bool,
    pub conflict: bool,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Projection {
    pub genesis: Genesis,
    pub task: Task,
    pub state: AgentState,
    pub tick: SqlU64,
    pub task_account: AccountSnapshot,
    pub principal_account: AccountSnapshot,
    pub verification: VerificationSnapshot,
    pub replay_head: Digest,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum JournalEvent {
    CommandCommitted {
        command: StoreCommand,
        result: StoreResult,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalRecordData {
    pub seq: SqlU64,
    pub previous: Digest,
    pub event: JournalEvent,
    pub digest: Digest,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "JournalRecordData", into = "JournalRecordData")]
pub struct JournalRecord(JournalRecordData);
impl JournalRecord {
    pub fn new(data: JournalRecordData) -> Result<Self, ValidationError> {
        canonical_bytes(&data, MAX_INTERNAL)?;
        Ok(Self(data))
    }
    pub fn data(&self) -> &JournalRecordData {
        &self.0
    }
}
impl TryFrom<JournalRecordData> for JournalRecord {
    type Error = ValidationError;
    fn try_from(value: JournalRecordData) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<JournalRecord> for JournalRecordData {
    fn from(value: JournalRecord) -> Self {
        value.0
    }
}
pub const MAX_PAGE_RECORDS: usize = 16;
pub const MAX_PAGE_BYTES: usize = 1024 * 1024;
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournalPage {
    pub records: Vec<JournalRecord>,
    pub next: Option<SqlU64>,
}

/// One coherent read transaction for the complete audit, including all pages and projection.
/// Implementations inspect stored payload lengths before allocating and enforce both page limits.
pub trait AuditSnapshot {
    fn projection(&mut self, task: &TaskId) -> Result<Projection, StoreError>;
    fn journal_page(&mut self, after: Option<SqlU64>) -> Result<JournalPage, StoreError>;
    fn pending_inbox(&mut self, task: &TaskId) -> Result<Option<InboxInput>, StoreError>;
}
/// Each first committed command appends exactly one `CommandCommitted` record;
/// duplicates and rejected attempts append none. That record's sequence is the
/// source sequence for generated inbox inputs. Each operation emits at most one
/// input, at output ordinal zero: task creation, denial, first uncertainty,
/// successful authoritative settlement, and verification respectively. Other
/// outcomes, including a stale receipt that does not settle, emit no input.
/// Replay derives inbox IDs and projections from prior state and the command and
/// result, and checks the derived values against the materialized rows.
/// The store recomputes command digests, deduplicates before preconditions and atomically
/// commits domain effects. Preparation and admission rederive materialization against the
/// persisted original intent and selected fixture evidence. The store validates every
/// transition against persisted prior state with `Transition::validate_from`, as well
/// as checking its event, source and phase-specific preconditions. Reconciliation atomically
/// records a supplied plan-bound receipt and settles under its evidence revision and lease.
/// Valid bound new evidence is retained even when it invalidates a reconciliation
/// snapshot or cannot settle; that outcome is committed as evidence rather than
/// returned as NotCommitted. Verification uses the task-wide evidence revision;
/// intent snapshots carry the per-intent reconciliation evidence revision.
/// Approval creation uses checked current database tick plus 30 logical ticks;
/// recording a decision and admitting a dispatch both reject an approval whose
/// `expires_at` is at or before the current database tick, independently of the
/// worker lease. No wall clock is involved.
/// Only a FirstCommit admission acknowledgement can support authority.
pub trait StorePort {
    type Audit<'a>: RuntimeSnapshot
    where
        Self: 'a;
    fn apply(&mut self, command: StoreCommand) -> Result<WriteOutcome<StoreResult>, StoreError>;
    /// Opens a coherent snapshot. Replay/verify use an existing database opened
    /// read-only, without migrations, checkpoints or domain mutations.
    fn audit(&mut self) -> Result<Self::Audit<'_>, StoreError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchStatus {
    Ready,
    AwaitingApproval,
    Denied,
    Leased,
    Dispatched,
    Uncertain,
    Settled,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseSnapshot {
    pub worker: WorkerId,
    pub epoch: SqlU64,
    pub expires_at: SqlU64,
}
/// Fixed logical lifetime for an approval request, independent of leases.
pub const APPROVAL_TTL_TICKS: u64 = 30;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalSnapshot {
    pub task_id: TaskId,
    pub intent_id: Digest,
    pub request_id: ApprovalId,
    pub action_digest: Digest,
    pub expires_at: SqlU64,
    pub decision: Option<ApprovalDecision>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntentSnapshot {
    pub intent_id: Digest,
    pub intent: Intent,
    pub prepared: Option<PreparedIntent>,
    pub status: DispatchStatus,
    pub dispatch_id: Option<Digest>,
    pub lease_epoch: SqlU64,
    pub lease: Option<LeaseSnapshot>,
    pub reconciliation_epoch: SqlU64,
    pub reconciliation_lease: Option<LeaseSnapshot>,
    pub reservation: Option<ReservationSnapshot>,
    pub approval: Option<ApprovalSnapshot>,
    pub evidence_revision: SqlU64,
    pub selected_evidence: Option<Digest>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantSnapshot {
    pub principal_id: PrincipalId,
    pub revision: SqlU64,
    pub revoked: bool,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceSnapshot {
    pub evidence_id: Digest,
    pub receipt: Receipt,
    pub selected: bool,
    pub conflict: bool,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnpostedDebit {
    pub dispatch_id: Digest,
    pub usage: UsageVector,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxStatus {
    Pending,
    Consumed,
    Superseded,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboxSnapshot {
    pub input: InboxInput,
    pub status: InboxStatus,
    pub transition: Option<Transition>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReservationStatus {
    Held,
    Released,
    Settled,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReservationSnapshot {
    pub id: Digest,
    pub amount: Counters,
    pub status: ReservationStatus,
}

/// A bounded page within the same coherent read transaction. Cursors are stable,
/// unique ordering keys for their view, not non-unique source sequences. The adapter must
/// inspect payload lengths before allocating and enforce the record and byte limits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotPage<T> {
    pub records: Vec<T>,
    pub next: Option<SqlU64>,
}

/// Bounded materialized views for driving and independently checking projections.
/// The snapshot is dropped before an external effect or authoritative write.
pub trait RuntimeSnapshot: AuditSnapshot {
    fn intent(&mut self, id: &Digest) -> Result<Option<IntentSnapshot>, StoreError>;
    fn intents(
        &mut self,
        task: &TaskId,
        after: Option<SqlU64>,
    ) -> Result<SnapshotPage<IntentSnapshot>, StoreError>;
    fn grant(&mut self, principal: &PrincipalId) -> Result<GrantSnapshot, StoreError>;
    fn evidence(
        &mut self,
        intent: &Digest,
        after: Option<SqlU64>,
    ) -> Result<SnapshotPage<EvidenceSnapshot>, StoreError>;
    fn inbox(
        &mut self,
        task: &TaskId,
        after: Option<SqlU64>,
    ) -> Result<SnapshotPage<InboxSnapshot>, StoreError>;
    fn unposted_debits(
        &mut self,
        task: &TaskId,
        after: Option<SqlU64>,
    ) -> Result<SnapshotPage<UnpostedDebit>, StoreError>;
}
