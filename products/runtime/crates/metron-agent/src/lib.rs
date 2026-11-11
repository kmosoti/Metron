//! Pure, deterministic cognition for the scripted sum-v1 task.
use metron_protocol::{
    AgentEvent, AgentState, ApprovedRequest, BoundedText, Cognition, DecisionError, Digest, Intent,
    IntentKind, MAX_CONTEXT, Phase, ReceiptOutput, SchemaVersion, SqlU64, Tariff, Transition,
    canonical_bytes, content_digest, parse_sum_input, structural_digest,
};

/// Immutable replay identifier for this cognition implementation.
pub const ENGINE_VERSION: &str = "sum-agent/v1";

/// Stateless cognition. It makes proposals; it has no authority to execute them.
#[derive(Clone, Copy, Debug, Default)]
pub struct SumAgent;

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

impl Cognition for SumAgent {
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
