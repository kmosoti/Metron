//! Pure, deterministic standalone planner for the scripted sum-v1 task.
use metron_protocol::{
    AgentEvent, AgentState, ApprovedRequest, BoundedText, Cognition, DecisionError, Digest, Intent,
    IntentKind, MAX_CONTEXT, Phase, ReceiptOutput, SchemaVersion, SqlU64, Tariff, Transition,
    canonical_bytes, content_digest, parse_sum_input, structural_digest,
};

/// Immutable replay identifier for this cognition implementation.
pub const ENGINE_VERSION: &str = "sum-harness/v1";

/// Stateless cognition. It makes proposals; it has no authority to execute them.
#[derive(Clone, Copy, Debug, Default)]
pub struct SumPlanner;

fn expected_pending(state: &AgentState, intent_id: &Digest) -> Result<(), DecisionError> {
    if state.pending_intent.as_ref() == Some(intent_id) {
        Ok(())
    } else {
        Err(DecisionError::InvalidEvent)
    }
}

fn next_state(state: &AgentState) -> Result<AgentState, DecisionError> {
    let mut next = state.clone();
    next.revision = state
        .revision
        .checked_add(1)
        .map_err(|_| DecisionError::Overflow)?;
    Ok(next)
}

fn decision(
    state: &AgentState,
    phase: Phase,
    kind: IntentKind,
) -> Result<Transition, DecisionError> {
    if state.decision_steps >= state.max_decision_steps {
        return Err(DecisionError::DecisionLimit);
    }
    let ordinal = u32::from(state.decision_steps);
    let genesis = state.genesis_digest.bytes();
    let pending = structural_digest(
        "intent-id/v1",
        &[
            state.task_id.as_str().as_bytes(),
            &genesis,
            &ordinal.to_be_bytes(),
        ],
    )
    .map_err(|_| DecisionError::InvariantViolation)?;
    let mut next = next_state(state)?;
    next.decision_steps += 1;
    next.phase = phase;
    next.pending_intent = Some(pending);
    let intent = Intent {
        schema_version: SchemaVersion::V1,
        task_id: state.task_id.clone(),
        ordinal,
        kind,
    };
    let result = Transition::Decision { next, intent };
    result
        .validate_from(state)
        .map_err(|_| DecisionError::InvariantViolation)?;
    Ok(result)
}

fn finalize(state: &AgentState, phase: Phase, clear: bool) -> Result<Transition, DecisionError> {
    let mut next = next_state(state)?;
    next.phase = phase;
    if clear {
        next.pending_intent = None;
    }
    let result = Transition::Finalization { next };
    result
        .validate_from(state)
        .map_err(|_| DecisionError::InvariantViolation)?;
    Ok(result)
}

impl Cognition for SumPlanner {
    fn transition(
        &self,
        state: &AgentState,
        event: &AgentEvent,
    ) -> Result<Transition, DecisionError> {
        state
            .validate()
            .map_err(|_| DecisionError::InvariantViolation)?;
        match state.phase {
            Phase::New | Phase::AwaitingVerification | Phase::Completed | Phase::Rejected
                if state.pending_intent.is_some() =>
            {
                return Err(DecisionError::InvariantViolation);
            }
            Phase::WaitingRead | Phase::WaitingModel | Phase::WaitingArtifact
                if state.pending_intent.is_none() =>
            {
                return Err(DecisionError::InvariantViolation);
            }
            _ => {}
        }
        match (state.phase, event) {
            (Phase::New, AgentEvent::TaskAccepted) => {
                decision(state, Phase::WaitingRead, IntentKind::ReadFixture)
            }
            (
                Phase::WaitingRead,
                AgentEvent::EffectSettled {
                    intent_id,
                    output: ReceiptOutput::Fixture { bytes, digest },
                    ..
                },
            ) => {
                expected_pending(state, intent_id)?;
                if content_digest(bytes.as_str().as_bytes()) != *digest
                    || parse_sum_input(bytes.as_str().as_bytes()).is_err()
                {
                    return Err(DecisionError::MissingEvidence);
                }
                let context = BoundedText::new(bytes.as_str())
                    .map_err(|_| DecisionError::InvariantViolation)?;
                let zero = SqlU64::new(0).map_err(|_| DecisionError::InvariantViolation)?;
                let request = ApprovedRequest::RequestModel {
                    input_digest: digest.clone(),
                    route: BoundedText::new("scripted/sum-v1")
                        .map_err(|_| DecisionError::InvariantViolation)?,
                    context: context.clone(),
                    tariff: Tariff {
                        model_requests: SqlU64::new(1)
                            .map_err(|_| DecisionError::InvariantViolation)?,
                        tool_calls: zero,
                        spend_micros_usd: zero,
                    },
                };
                canonical_bytes(&request, MAX_CONTEXT)
                    .map_err(|_| DecisionError::InvariantViolation)?;
                decision(
                    state,
                    Phase::WaitingModel,
                    IntentKind::RequestModel {
                        input_digest: digest.clone(),
                        context,
                    },
                )
            }
            (
                Phase::WaitingModel,
                AgentEvent::EffectSettled {
                    intent_id,
                    output: ReceiptOutput::Model { candidate },
                    ..
                },
            ) => {
                expected_pending(state, intent_id)?;
                decision(
                    state,
                    Phase::WaitingArtifact,
                    IntentKind::PublishArtifact {
                        candidate: candidate.clone(),
                    },
                )
            }
            (
                Phase::WaitingArtifact,
                AgentEvent::EffectSettled {
                    intent_id,
                    output: ReceiptOutput::Published { .. },
                    ..
                },
            ) => {
                expected_pending(state, intent_id)?;
                finalize(state, Phase::AwaitingVerification, true)
            }
            (
                Phase::WaitingRead | Phase::WaitingModel | Phase::WaitingArtifact,
                AgentEvent::EffectUncertain { intent_id },
            ) => {
                expected_pending(state, intent_id)?;
                finalize(state, state.phase, false)
            }
            (
                Phase::WaitingRead | Phase::WaitingModel | Phase::WaitingArtifact,
                AgentEvent::Denied { intent_id, .. }
                | AgentEvent::EffectSettled {
                    intent_id,
                    output: ReceiptOutput::Failure { .. },
                    ..
                },
            ) => {
                expected_pending(state, intent_id)?;
                finalize(state, Phase::Rejected, true)
            }
            (Phase::AwaitingVerification, AgentEvent::Verified { accepted, .. }) => finalize(
                state,
                if *accepted {
                    Phase::Completed
                } else {
                    Phase::Rejected
                },
                true,
            ),
            _ => Err(DecisionError::InvalidEvent),
        }
    }
}

/// Local-profile planner with an immutable replay identity.
pub const LOCAL_ENGINE_VERSION: &str = "sum-local-harness/v2";
pub struct LocalSumPlanner {
    profile_digest: Digest,
}
impl LocalSumPlanner {
    pub fn new(
        profile: &metron_protocol::LocalProfile,
    ) -> Result<Self, metron_protocol::ValidationError> {
        Ok(Self {
            profile_digest: profile.digest()?,
        })
    }
}
impl Cognition for LocalSumPlanner {
    fn transition(
        &self,
        state: &AgentState,
        event: &AgentEvent,
    ) -> Result<Transition, DecisionError> {
        if state.phase == Phase::WaitingModel
            && matches!(
                event,
                AgentEvent::EffectSettled {
                    output: ReceiptOutput::Model { .. },
                    ..
                }
            )
        {
            return Err(DecisionError::InvalidEvent);
        }
        let mapped = match event {
            AgentEvent::EffectSettled {
                intent_id,
                evidence_id,
                output: ReceiptOutput::LocalModel { observation },
            } => {
                if observation.profile_digest != self.profile_digest {
                    return Err(DecisionError::MissingEvidence);
                }
                let output = match &observation.response {
                    Some(metron_protocol::LocalResponse::Sum(sum)) => ReceiptOutput::Model {
                        candidate: metron_protocol::CandidateResult {
                            count: sum.count,
                            sum: sum.sum,
                            input_digest: observation.input_digest.clone(),
                        },
                    },
                    _ => ReceiptOutput::Failure {
                        diagnostic: BoundedText::new("invalid local response")
                            .map_err(|_| DecisionError::InvariantViolation)?,
                    },
                };
                AgentEvent::EffectSettled {
                    intent_id: intent_id.clone(),
                    evidence_id: evidence_id.clone(),
                    output,
                }
            }
            _ => event.clone(),
        };
        let mut transition = SumPlanner.transition(state, &mapped)?;
        if let Transition::Decision { intent, .. } = &mut transition {
            intent.schema_version = SchemaVersion::V2;
            if let IntentKind::RequestModel {
                input_digest,
                context,
            } = &intent.kind
            {
                intent.kind = IntentKind::RequestLocalModel {
                    input_digest: input_digest.clone(),
                    context: context.clone(),
                    profile_digest: self.profile_digest.clone(),
                    role: metron_protocol::LocalRole::Solver,
                    output_schema: metron_protocol::LocalOutputSchema::Sum,
                };
                let IntentKind::RequestLocalModel {
                    input_digest,
                    context,
                    profile_digest,
                    role,
                    output_schema,
                } = &intent.kind
                else {
                    return Err(DecisionError::InvariantViolation);
                };
                let request = ApprovedRequest::RequestLocalModel {
                    input_digest: input_digest.clone(),
                    context: context.clone(),
                    profile_digest: profile_digest.clone(),
                    role: *role,
                    output_schema: *output_schema,
                    tariff: Tariff {
                        model_requests: SqlU64::new(1)
                            .map_err(|_| DecisionError::InvariantViolation)?,
                        tool_calls: SqlU64::new(0)
                            .map_err(|_| DecisionError::InvariantViolation)?,
                        spend_micros_usd: SqlU64::new(0)
                            .map_err(|_| DecisionError::InvariantViolation)?,
                    },
                };
                canonical_bytes(&request, MAX_CONTEXT)
                    .map_err(|_| DecisionError::InvariantViolation)?;
            }
        }
        Ok(transition)
    }
}

/// Pure version-3 coding controller. Model and tool data remain proposals.
#[derive(Clone, Debug)]
pub struct CodingPlanner {
    task: metron_protocol::coding::CodingTask,
    policy: metron_protocol::coding::CodingPolicy,
    version: u8,
}
impl CodingPlanner {
    pub const ENGINE_VERSION: &str = "coding-harness/v3.8";
    pub const CONSTRAINED_ENGINE_VERSION: &str = "coding-harness/v3.6";
    pub const RECOVERING_ENGINE_VERSION: &str = "coding-harness/v3.5";
    pub const NATIVE_ENGINE_VERSION: &str = "coding-harness/v3.4";
    pub const FOCUSED_ENGINE_VERSION: &str = "coding-harness/v3.3";
    pub const CATALOG_ENGINE_VERSION: &str = "coding-harness/v3.2";
    pub const PHASED_ENGINE_VERSION: &str = "coding-harness/v3.1";
    pub const LEGACY_ENGINE_VERSION: &str = "coding-harness/v3";
    pub fn new(
        task: &metron_protocol::coding::CodingTask,
        policy: &metron_protocol::coding::CodingPolicy,
    ) -> Result<Self, metron_protocol::ValidationError> {
        policy.validate(task)?;
        Ok(Self {
            task: task.clone(),
            policy: policy.clone(),
            version: 8,
        })
    }
    pub fn legacy_v3_7(
        task: &metron_protocol::coding::CodingTask,
        policy: &metron_protocol::coding::CodingPolicy,
    ) -> Result<Self, metron_protocol::ValidationError> {
        policy.validate(task)?;
        Ok(Self {
            task: task.clone(),
            policy: policy.clone(),
            version: 7,
        })
    }
    pub fn legacy_v3_6(
        task: &metron_protocol::coding::CodingTask,
        policy: &metron_protocol::coding::CodingPolicy,
    ) -> Result<Self, metron_protocol::ValidationError> {
        policy.validate(task)?;
        Ok(Self {
            task: task.clone(),
            policy: policy.clone(),
            version: 6,
        })
    }
    pub fn legacy_v3_5(
        task: &metron_protocol::coding::CodingTask,
        policy: &metron_protocol::coding::CodingPolicy,
    ) -> Result<Self, metron_protocol::ValidationError> {
        policy.validate(task)?;
        Ok(Self {
            task: task.clone(),
            policy: policy.clone(),
            version: 5,
        })
    }
    pub fn legacy_v3_4(
        task: &metron_protocol::coding::CodingTask,
        policy: &metron_protocol::coding::CodingPolicy,
    ) -> Result<Self, metron_protocol::ValidationError> {
        policy.validate(task)?;
        Ok(Self {
            task: task.clone(),
            policy: policy.clone(),
            version: 4,
        })
    }
    pub fn legacy_v3_3(
        task: &metron_protocol::coding::CodingTask,
        policy: &metron_protocol::coding::CodingPolicy,
    ) -> Result<Self, metron_protocol::ValidationError> {
        policy.validate(task)?;
        Ok(Self {
            task: task.clone(),
            policy: policy.clone(),
            version: 3,
        })
    }
    pub fn legacy_v3(
        task: &metron_protocol::coding::CodingTask,
        policy: &metron_protocol::coding::CodingPolicy,
    ) -> Result<Self, metron_protocol::ValidationError> {
        policy.validate(task)?;
        Ok(Self {
            task: task.clone(),
            policy: policy.clone(),
            version: 0,
        })
    }
    pub fn legacy_v3_1(
        task: &metron_protocol::coding::CodingTask,
        policy: &metron_protocol::coding::CodingPolicy,
    ) -> Result<Self, metron_protocol::ValidationError> {
        policy.validate(task)?;
        Ok(Self {
            task: task.clone(),
            policy: policy.clone(),
            version: 1,
        })
    }
    pub fn legacy_v3_2(
        task: &metron_protocol::coding::CodingTask,
        policy: &metron_protocol::coding::CodingPolicy,
    ) -> Result<Self, metron_protocol::ValidationError> {
        policy.validate(task)?;
        Ok(Self {
            task: task.clone(),
            policy: policy.clone(),
            version: 2,
        })
    }
}
impl Cognition for CodingPlanner {
    fn transition(
        &self,
        state: &AgentState,
        event: &AgentEvent,
    ) -> Result<Transition, DecisionError> {
        match self.version {
            0 => metron_protocol::coding_runtime::coding_transition(
                &self.task,
                &self.policy,
                state,
                event,
            ),
            1 => metron_protocol::coding_runtime::coding_transition_phased(
                &self.task,
                &self.policy,
                state,
                event,
            ),
            2 => metron_protocol::coding_runtime::coding_transition_catalog(
                &self.task,
                &self.policy,
                state,
                event,
            ),
            3 => metron_protocol::coding_runtime::coding_transition_focused(
                &self.task,
                &self.policy,
                state,
                event,
            ),
            4 => metron_protocol::coding_runtime::coding_transition_native(
                &self.task,
                &self.policy,
                state,
                event,
            ),
            5 => metron_protocol::coding_runtime::coding_transition_recovering(
                &self.task,
                &self.policy,
                state,
                event,
            ),
            6 => metron_protocol::coding_runtime::coding_transition_constrained(
                &self.task,
                &self.policy,
                state,
                event,
            ),
            7 => metron_protocol::coding_runtime::coding_transition_format_neutral(
                &self.task,
                &self.policy,
                state,
                event,
            ),
            _ => metron_protocol::coding_runtime::coding_transition_memory(
                &self.task,
                &self.policy,
                state,
                event,
            ),
        }
    }
}
