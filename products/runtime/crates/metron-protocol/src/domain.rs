//! Versioned runtime values; these describe requests, never authority.
use crate::types::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageVector {
    pub model_requests: DecimalUsage,
    pub tool_calls: DecimalUsage,
    pub spend_micros_usd: DecimalUsage,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActualUsage {
    Known {
        usage: UsageVector,
    },
    Unknown {
        reason: BoundedText<MAX_TEXT>,
        evidence: Option<EvidenceRef>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetRequest {
    pub max_model_requests: SqlU64,
    pub max_tool_calls: SqlU64,
    pub max_spend_micros_usd: SqlU64,
    pub max_elapsed_ms: Option<u64>,
}
impl BudgetRequest {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.max_elapsed_ms.is_some() {
            Err(ValidationError::UnsupportedBudgetDimension)
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskData {
    pub schema_version: SchemaVersion,
    pub task_id: TaskId,
    pub principal_id: PrincipalId,
    pub objective: BoundedText<MAX_TEXT>,
    pub budget: BudgetRequest,
    #[serde(default = "default_decision_steps")]
    pub max_decision_steps: u8,
}
fn default_decision_steps() -> u8 {
    8
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "TaskData", into = "TaskData")]
pub struct Task(TaskData);
impl Task {
    pub fn new(data: TaskData) -> Result<Self, ValidationError> {
        data.budget.validate()?;
        if data.max_decision_steps > 64 {
            return Err(ValidationError::Limit);
        }
        canonical_bytes(&data, MAX_ENVELOPE)?;
        Ok(Self(data))
    }
    pub fn data(&self) -> &TaskData {
        &self.0
    }
}
impl TryFrom<TaskData> for Task {
    type Error = ValidationError;
    fn try_from(value: TaskData) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<Task> for TaskData {
    fn from(value: Task) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateResult {
    pub count: u32,
    pub sum: i64,
    pub input_digest: Digest,
}
impl CandidateResult {
    pub fn artifact_bytes(&self) -> Result<Vec<u8>, ValidationError> {
        let mut bytes = canonical_bytes(self, MAX_ENVELOPE - 1)?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum IntentKind {
    ReadFixture,
    RequestModel {
        input_digest: Digest,
        context: BoundedText<MAX_CONTEXT>,
    },
    PublishArtifact {
        candidate: CandidateResult,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    pub schema_version: SchemaVersion,
    pub task_id: TaskId,
    pub ordinal: u32,
    pub kind: IntentKind,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tariff {
    pub model_requests: SqlU64,
    pub tool_calls: SqlU64,
    pub spend_micros_usd: SqlU64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactPlan {
    pub intent_id: Digest,
    pub dispatch_id: Digest,
    pub artifact_id: Digest,
    pub relative_path: BoundedText<MAX_TEXT>,
    pub candidate: CandidateResult,
    pub content_length: SqlU64,
    pub content_digest: Digest,
    pub tariff: Tariff,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ApprovedRequest {
    ReadFixture {
        fixture_key: BoundedText<MAX_TEXT>,
        tariff: Tariff,
    },
    RequestModel {
        input_digest: Digest,
        route: BoundedText<MAX_TEXT>,
        context: BoundedText<MAX_CONTEXT>,
        tariff: Tariff,
    },
    PublishArtifact {
        plan: ArtifactPlan,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionBinding {
    pub task_id: TaskId,
    pub principal_id: PrincipalId,
    pub intent_id: Digest,
    pub dispatch_id: Digest,
    pub request_digest: Digest,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedIntent {
    pub intent_id: Digest,
    pub intent_digest: Digest,
    pub request: ApprovedRequest,
    pub request_digest: Digest,
    pub action: ActionBinding,
    pub action_digest: Digest,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReceiptOutput {
    Fixture {
        bytes: BoundedText<MAX_FIXTURE>,
        digest: Digest,
    },
    Model {
        candidate: CandidateResult,
    },
    Published {
        artifact_id: Digest,
        content_digest: Digest,
        content_length: SqlU64,
    },
    Failure {
        diagnostic: BoundedText<MAX_TEXT>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptData {
    pub schema_version: SchemaVersion,
    pub producer_receipt_id: ProducerReceiptId,
    pub task_id: TaskId,
    pub intent_id: Digest,
    pub dispatch_id: Digest,
    pub action_digest: Digest,
    pub usage: ActualUsage,
    pub output: ReceiptOutput,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "ReceiptData", into = "ReceiptData")]
pub struct Receipt(ReceiptData);
impl Receipt {
    pub fn new(data: ReceiptData) -> Result<Self, ValidationError> {
        canonical_bytes(&data, MAX_ENVELOPE)?;
        Ok(Self(data))
    }
    pub fn data(&self) -> &ReceiptData {
        &self.0
    }
    pub fn evidence_id(&self) -> Result<Digest, ValidationError> {
        structural_digest(
            "receipt-evidence/v1",
            &[&canonical_bytes(self, MAX_ENVELOPE)?],
        )
    }
}
impl TryFrom<ReceiptData> for Receipt {
    type Error = ValidationError;
    fn try_from(value: ReceiptData) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<Receipt> for ReceiptData {
    fn from(value: Receipt) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    New,
    WaitingRead,
    WaitingModel,
    WaitingArtifact,
    AwaitingVerification,
    Completed,
    Rejected,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentState {
    pub task_id: TaskId,
    pub genesis_digest: Digest,
    pub max_decision_steps: u8,
    pub revision: SqlU64,
    pub decision_steps: u8,
    pub phase: Phase,
    pub pending_intent: Option<Digest>,
}
impl AgentState {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.max_decision_steps > 64 || self.decision_steps > self.max_decision_steps {
            Err(ValidationError::Limit)
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentEvent {
    TaskAccepted,
    EffectSettled {
        intent_id: Digest,
        evidence_id: Digest,
        output: ReceiptOutput,
    },
    EffectUncertain {
        intent_id: Digest,
    },
    Denied {
        intent_id: Digest,
        reason: BoundedText<MAX_TEXT>,
    },
    Verified {
        accepted: bool,
        reason: BoundedText<MAX_TEXT>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Transition {
    Decision { next: AgentState, intent: Intent },
    Finalization { next: AgentState },
}
impl Transition {
    /// Validate transition-wide invariants against the persisted prior state.
    pub fn validate_from(&self, previous: &AgentState) -> Result<(), ValidationError> {
        previous.validate()?;
        let (next, intent) = match self {
            Self::Decision { next, intent } => (next, Some(intent)),
            Self::Finalization { next } => (next, None),
        };
        next.validate()?;
        if next.task_id != previous.task_id
            || next.genesis_digest != previous.genesis_digest
            || next.max_decision_steps != previous.max_decision_steps
            || next.revision != previous.revision.checked_add(1)?
        {
            return Err(ValidationError::Binding);
        }
        match intent {
            Some(intent)
                if intent.task_id == previous.task_id
                    && previous.decision_steps < previous.max_decision_steps
                    && next.decision_steps == previous.decision_steps + 1 =>
            {
                Ok(())
            }
            None if next.decision_steps == previous.decision_steps => Ok(()),
            _ => Err(ValidationError::Binding),
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecisionError {
    InvalidEvent,
    InvariantViolation,
    MissingEvidence,
    DecisionLimit,
    Overflow,
}
pub trait Cognition: Send + Sync {
    fn transition(
        &self,
        state: &AgentState,
        event: &AgentEvent,
    ) -> Result<Transition, DecisionError>;
}

impl PreparedIntent {
    /// Derive all materialization fields from an original committed intent and genesis.
    pub fn materialize(
        intent: &Intent,
        principal: &PrincipalId,
        genesis_digest: &Digest,
    ) -> Result<Self, ValidationError> {
        let intent_bytes = canonical_bytes(intent, MAX_ENVELOPE)?;
        let intent_digest = structural_digest("intent/v1", &[&intent_bytes])?;
        let ordinal = intent.ordinal.to_be_bytes();
        let genesis = genesis_digest.bytes();
        let intent_id = structural_digest(
            "intent-id/v1",
            &[intent.task_id.as_str().as_bytes(), &genesis, &ordinal],
        )?;
        let dispatch_id = structural_digest("dispatch-id/v1", &[intent_id.as_str().as_bytes()])?;
        let zero = SqlU64::new(0)?;
        let one = SqlU64::new(1)?;
        let tariff = |model_requests, tool_calls| Tariff {
            model_requests,
            tool_calls,
            spend_micros_usd: zero,
        };
        let request = match &intent.kind {
            IntentKind::ReadFixture => ApprovedRequest::ReadFixture {
                fixture_key: BoundedText::new("fixture://sum-v1/input")?,
                tariff: tariff(zero, one),
            },
            IntentKind::RequestModel {
                input_digest,
                context,
            } => ApprovedRequest::RequestModel {
                input_digest: input_digest.clone(),
                route: BoundedText::new("scripted/sum-v1")?,
                context: context.clone(),
                tariff: tariff(one, zero),
            },
            IntentKind::PublishArtifact { candidate } => {
                let bytes = candidate.artifact_bytes()?;
                let artifact_id =
                    structural_digest("artifact-id/v1", &[dispatch_id.as_str().as_bytes()])?;
                let relative_path = BoundedText::new(format!(
                    "artifacts/{}/{}/result.json",
                    intent_id.as_str(),
                    dispatch_id.as_str()
                ))?;
                ApprovedRequest::PublishArtifact {
                    plan: ArtifactPlan {
                        intent_id: intent_id.clone(),
                        dispatch_id: dispatch_id.clone(),
                        artifact_id,
                        relative_path,
                        candidate: candidate.clone(),
                        content_length: SqlU64::new(bytes.len() as u64)?,
                        content_digest: content_digest(&bytes),
                        tariff: tariff(zero, one),
                    },
                }
            }
        };
        let request_limit = if matches!(&request, ApprovedRequest::RequestModel { .. }) {
            MAX_CONTEXT
        } else {
            MAX_ENVELOPE
        };
        let request_bytes = canonical_bytes(&request, request_limit)?;
        let request_digest = structural_digest("approved-request/v1", &[&request_bytes])?;
        let action = ActionBinding {
            task_id: intent.task_id.clone(),
            principal_id: principal.clone(),
            intent_id: intent_id.clone(),
            dispatch_id,
            request_digest: request_digest.clone(),
        };
        let action_digest =
            structural_digest("action/v1", &[&canonical_bytes(&action, MAX_ENVELOPE)?])?;
        Ok(Self {
            intent_id,
            intent_digest,
            request,
            request_digest,
            action,
            action_digest,
        })
    }
    /// Recompute immutable material before preparation or admission; fields alone confer no trust.
    pub fn validate_against(
        &self,
        intent: &Intent,
        principal: &PrincipalId,
        genesis_digest: &Digest,
    ) -> Result<(), ValidationError> {
        if *self == Self::materialize(intent, principal, genesis_digest)? {
            Ok(())
        } else {
            Err(ValidationError::Binding)
        }
    }
}
impl ArtifactPlan {
    pub fn candidate_bytes(&self) -> Result<Vec<u8>, ValidationError> {
        self.candidate.artifact_bytes()
    }
}

/// The fixture's input schema. Arithmetic is intentionally left to consumers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SumInput {
    pub values: Vec<i64>,
}

/// Decode fixture bytes only after enforcing the raw byte limit.
pub fn parse_sum_input(bytes: &[u8]) -> Result<SumInput, ValidationError> {
    if bytes.len() > MAX_FIXTURE {
        return Err(ValidationError::Limit);
    }
    serde_json::from_slice(bytes).map_err(|e| ValidationError::Json(e.to_string()))
}
