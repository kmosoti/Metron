//! Closed versioned command reports and diagnostics for the runtime host.
use crate::{domain::*, store::*, types::*};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CliCommand {
    Run,
    Resume,
    Reconcile,
    Approval,
    Replay,
    Verify,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CliOutcome {
    Succeeded,
    Rejected,
    InvalidInput,
    Denied,
    Blocked,
    IntegrityFailure,
    OperationalFailure,
}

/// Current task health, separate from historical cognition completion.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CliHealth {
    pub verified_success: bool,
    pub pending_input: bool,
    pub unresolved_dispatch: bool,
    pub reservations_held: bool,
    pub incomplete: bool,
    pub breached: bool,
    pub integrity_hold: bool,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CliApproval {
    pub request_id: ApprovalId,
    pub action_digest: Digest,
    pub decision: Option<ApprovalDecision>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CliAccounting {
    pub task_posted: Counters,
    pub principal_posted: Counters,
    pub reported_read: Option<ActualUsage>,
    pub reported_model: Option<ActualUsage>,
    pub reported_publication: Option<ActualUsage>,
}
/// An absent check was not executed or was inapplicable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CliAudit {
    pub chain_valid: Option<bool>,
    pub accounting_valid: Option<bool>,
    pub projection_valid: Option<bool>,
    pub behavior_valid: Option<bool>,
    pub artifact_valid: Option<bool>,
}
/// Reports and stderr diagnostics share this closed envelope. A successful command
/// does not by itself assert verified task success.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CliReport {
    pub schema_version: SchemaVersion,
    pub command: CliCommand,
    pub outcome: CliOutcome,
    pub task_id: Option<TaskId>,
    pub phase: Option<Phase>,
    pub pending_intent: Option<Digest>,
    pub health: Option<CliHealth>,
    pub replay_head: Option<Digest>,
    pub approval: Option<CliApproval>,
    pub accounting: Option<CliAccounting>,
    pub audit: Option<CliAudit>,
    pub diagnostic: Option<BoundedText<MAX_TEXT>>,
}
impl CliReport {
    /// Encode a bounded report without embedding full receipts.
    pub fn encode(&self) -> Result<Vec<u8>, ValidationError> {
        canonical_bytes(self, MAX_INTERNAL)
    }
}
