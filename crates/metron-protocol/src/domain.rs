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
#[serde(untagged)]
enum TaskWire {
    Legacy(TaskData),
    Coding(Box<crate::coding::CodingTask>),
}

/// Versioned task with one shared header for the durable effect machinery.
/// Serialization preserves original legacy bytes or the complete coding
/// contract. A common header alone cannot reconstruct a coding task.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "TaskWire", into = "TaskWire")]
pub struct Task {
    common: TaskData,
    coding: Option<Box<crate::coding::CodingTask>>,
}
impl Task {
    pub fn new(data: TaskData) -> Result<Self, ValidationError> {
        if !matches!(data.schema_version, SchemaVersion::V1 | SchemaVersion::V2) {
            return Err(ValidationError::Version);
        }
        data.budget.validate()?;
        if data.max_decision_steps > 64 {
            return Err(ValidationError::Limit);
        }
        canonical_bytes(&data, MAX_ENVELOPE)?;
        Ok(Self {
            common: data,
            coding: None,
        })
    }
    pub fn new_coding(task: crate::coding::CodingTask) -> Result<Self, ValidationError> {
        canonical_bytes(&task, MAX_ENVELOPE)?;
        let data = task.data();
        let common = TaskData {
            schema_version: SchemaVersion::V3,
            task_id: data.task_id.clone(),
            principal_id: data.principal_id.clone(),
            objective: data.objective.clone(),
            budget: data.budget.clone(),
            max_decision_steps: data.max_decision_steps,
        };
        Ok(Self {
            common,
            coding: Some(Box::new(task)),
        })
    }
    pub fn data(&self) -> &TaskData {
        &self.common
    }
    pub fn coding(&self) -> Option<&crate::coding::CodingTask> {
        self.coding.as_deref()
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
        value.common
    }
}
impl TryFrom<TaskWire> for Task {
    type Error = ValidationError;
    fn try_from(value: TaskWire) -> Result<Self, Self::Error> {
        match value {
            TaskWire::Legacy(data) => Self::new(data),
            TaskWire::Coding(task) => Self::new_coding(*task),
        }
    }
}
impl From<Task> for TaskWire {
    fn from(value: Task) -> Self {
        match value.coding {
            Some(task) => Self::Coding(task),
            None => Self::Legacy(value.common),
        }
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
    Coding {
        step: Box<crate::coding_runtime::CodingStep>,
    },
    ReadFixture,
    RequestModel {
        input_digest: Digest,
        context: BoundedText<MAX_CONTEXT>,
    },
    RequestLocalModel {
        input_digest: Digest,
        context: BoundedText<MAX_CONTEXT>,
        profile_digest: Digest,
        role: crate::LocalRole,
        output_schema: crate::LocalOutputSchema,
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
    Coding {
        step: Box<crate::coding_runtime::CodingStep>,
        tariff: Tariff,
    },
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
    RequestLocalModel {
        input_digest: Digest,
        context: BoundedText<MAX_CONTEXT>,
        profile_digest: Digest,
        role: crate::LocalRole,
        output_schema: crate::LocalOutputSchema,
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
    Coding {
        observation: Box<crate::coding_runtime::CodingObservation>,
    },
    Fixture {
        bytes: BoundedText<MAX_FIXTURE>,
        digest: Digest,
    },
    Model {
        candidate: CandidateResult,
    },
    LocalModel {
        observation: Box<crate::LocalObservation>,
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
        match (&data.output, data.schema_version) {
            (ReceiptOutput::LocalModel { observation }, SchemaVersion::V2 | SchemaVersion::V3)
                if observation.schema_version == SchemaVersion::V2 => {}
            (ReceiptOutput::LocalModel { .. }, _) => return Err(ValidationError::Version),
            (ReceiptOutput::Coding { .. }, SchemaVersion::V3)
            | (ReceiptOutput::Failure { .. }, SchemaVersion::V3) => {}
            (ReceiptOutput::Coding { .. }, _) | (_, SchemaVersion::V3) => {
                return Err(ValidationError::Version);
            }
            _ => {}
        }
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
    WaitingTool,
    WaitingArtifact,
    AwaitingVerification,
    Completed,
    Rejected,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coding_context: Option<crate::coding_runtime::CodingContext>,
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
        if let Some(context) = &self.coding_context {
            context.validate()?;
        }
        if self.max_decision_steps > 64 || self.decision_steps > self.max_decision_steps {
            return Err(ValidationError::Limit);
        }
        let pending_required = matches!(
            self.phase,
            Phase::WaitingRead | Phase::WaitingModel | Phase::WaitingTool | Phase::WaitingArtifact
        );
        if self.pending_intent.is_some() != pending_required {
            return Err(ValidationError::Binding);
        }
        Ok(())
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
        if previous.coding_context.is_none()
            && next.coding_context.is_some()
            && !matches!(
                intent,
                Some(Intent {
                    schema_version: SchemaVersion::V3,
                    kind: IntentKind::Coding { .. },
                    ..
                })
            )
        {
            return Err(ValidationError::Binding);
        }
        if previous.coding_context.is_some() && next.coding_context.is_none() {
            return Err(ValidationError::Binding);
        }
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
            IntentKind::Coding { step } => {
                if intent.schema_version != SchemaVersion::V3 {
                    return Err(ValidationError::Version);
                }
                step.validate()?;
                ApprovedRequest::Coding {
                    step: step.clone(),
                    tariff: if matches!(
                        step.as_ref(),
                        crate::coding_runtime::CodingStep::Model { .. }
                    ) {
                        tariff(one, zero)
                    } else {
                        tariff(zero, one)
                    },
                }
            }
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
            IntentKind::RequestLocalModel {
                input_digest,
                context,
                profile_digest,
                role,
                output_schema,
            } => {
                if intent.schema_version != SchemaVersion::V2 {
                    return Err(ValidationError::Version);
                }
                if !matches!(
                    (role, output_schema),
                    (crate::LocalRole::Solver, crate::LocalOutputSchema::Sum)
                        | (
                            crate::LocalRole::ToolSelector,
                            crate::LocalOutputSchema::ToolSelection
                        )
                ) {
                    return Err(ValidationError::Binding);
                }
                ApprovedRequest::RequestLocalModel {
                    input_digest: input_digest.clone(),
                    context: context.clone(),
                    profile_digest: profile_digest.clone(),
                    role: *role,
                    output_schema: *output_schema,
                    tariff: tariff(one, zero),
                }
            }
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
        let request_limit = if matches!(
            &request,
            ApprovedRequest::RequestModel { .. } | ApprovedRequest::RequestLocalModel { .. }
        ) {
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
    /// Verify the request against the persisted genesis, including its local profile.
    pub fn validate_against_genesis(
        &self,
        intent: &Intent,
        principal: &PrincipalId,
        genesis_digest: &Digest,
        genesis: &crate::Genesis,
    ) -> Result<(), ValidationError> {
        self.validate_against(intent, principal, genesis_digest)?;
        match (&intent.kind, &genesis.local_profile, intent.schema_version) {
            (IntentKind::Coding { step }, profile, SchemaVersion::V3) => {
                if genesis.coding_policy.is_none() {
                    return Err(ValidationError::Binding);
                }
                match step.as_ref() {
                    crate::coding_runtime::CodingStep::Model { profile_digest, .. } => {
                        let expected = match profile {
                            Some(profile) => profile.digest()?,
                            None => content_digest(b"coding-scripted/v1"),
                        };
                        if *profile_digest != expected {
                            return Err(ValidationError::Binding);
                        }
                    }
                    // Scope/approval denial is a durable admission outcome,
                    // not permission granted by preparing the proposal.
                    crate::coding_runtime::CodingStep::Tool { .. }
                    | crate::coding_runtime::CodingStep::Freeze { .. } => {}
                }
                Ok(())
            }
            (
                IntentKind::RequestLocalModel { profile_digest, .. },
                Some(profile),
                SchemaVersion::V2,
            ) if *profile_digest == profile.digest()? => Ok(()),
            (IntentKind::RequestModel { .. }, None, SchemaVersion::V1) => Ok(()),
            (
                IntentKind::ReadFixture | IntentKind::PublishArtifact { .. },
                None,
                SchemaVersion::V1,
            ) => Ok(()),
            (
                IntentKind::ReadFixture | IntentKind::PublishArtifact { .. },
                Some(_),
                SchemaVersion::V2,
            ) => Ok(()),
            _ => Err(ValidationError::Binding),
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
