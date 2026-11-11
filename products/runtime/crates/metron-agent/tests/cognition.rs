use metron_agent::{ENGINE_VERSION, SumAgent};
use metron_protocol::*;

fn initial(limit: u8) -> AgentState {
    AgentState {
        task_id: TaskId::new("task").unwrap(),
        genesis_digest: content_digest(b"genesis"),
        max_decision_steps: limit,
        revision: SqlU64::new(0).unwrap(),
        decision_steps: 0,
        phase: Phase::New,
        pending_intent: None,
    }
}
fn next(transition: Transition) -> AgentState {
    match transition {
        Transition::Decision { next, .. } | Transition::Finalization { next } => next,
    }
}
fn fixture_event(state: &AgentState) -> AgentEvent {
    let bytes = BoundedText::new("{\"values\":[17,-4,9]}\n").unwrap();
    AgentEvent::EffectSettled {
        intent_id: state.pending_intent.clone().unwrap(),
        evidence_id: content_digest(b"read receipt"),
        output: ReceiptOutput::Fixture {
            digest: content_digest(bytes.as_str().as_bytes()),
            bytes,
        },
    }
}
fn model_event(state: &AgentState, sum: i64) -> AgentEvent {
    AgentEvent::EffectSettled {
        intent_id: state.pending_intent.clone().unwrap(),
        evidence_id: content_digest(b"model receipt"),
        output: ReceiptOutput::Model {
            candidate: CandidateResult {
                count: 3,
                sum,
                input_digest: content_digest(b"fixture"),
            },
        },
    }
}
fn published(state: &AgentState) -> AgentEvent {
    AgentEvent::EffectSettled {
        intent_id: state.pending_intent.clone().unwrap(),
        evidence_id: content_digest(b"publish receipt"),
        output: ReceiptOutput::Published {
            artifact_id: content_digest(b"artifact"),
            content_digest: content_digest(b"bytes"),
            content_length: SqlU64::new(3).unwrap(),
        },
    }
}
#[test]
fn scripted_path_emits_three_decisions_and_two_free_finalizations() {
    assert_eq!(ENGINE_VERSION, "sum-agent/v1");
    let agent = SumAgent;
    let start = initial(8);
    let read = agent.transition(&start, &AgentEvent::TaskAccepted).unwrap();
    assert_eq!(
        read,
        agent.transition(&start, &AgentEvent::TaskAccepted).unwrap()
    );
    let Transition::Decision {
        next: read_state,
        intent,
    } = read
    else {
        panic!("read decision")
    };
    assert_eq!(intent.kind, IntentKind::ReadFixture);
    assert_eq!(
        (read_state.decision_steps, read_state.phase),
        (1, Phase::WaitingRead)
    );
    let model = agent
        .transition(&read_state, &fixture_event(&read_state))
        .unwrap();
    let Transition::Decision {
        next: model_state,
        intent,
    } = model
    else {
        panic!("model decision")
    };
    let IntentKind::RequestModel {
        input_digest,
        context,
    } = intent.kind
    else {
        panic!("model intent")
    };
    assert_eq!(context.as_str(), "{\"values\":[17,-4,9]}\n");
    assert_eq!(input_digest, content_digest(context.as_str().as_bytes()));
    let publication = agent
        .transition(&model_state, &model_event(&model_state, 999))
        .unwrap();
    let Transition::Decision {
        next: publish_state,
        intent,
    } = publication
    else {
        panic!("publish decision")
    };
    assert!(
        matches!(intent.kind, IntentKind::PublishArtifact { candidate } if candidate.sum == 999)
    );
    assert_eq!(publish_state.decision_steps, 3);
    let await_verify = next(
        agent
            .transition(&publish_state, &published(&publish_state))
            .unwrap(),
    );
    assert_eq!(
        (
            await_verify.phase,
            await_verify.decision_steps,
            await_verify.revision.get()
        ),
        (Phase::AwaitingVerification, 3, 4)
    );
    let rejected = next(
        agent
            .transition(
                &await_verify,
                &AgentEvent::Verified {
                    accepted: false,
                    reason: BoundedText::new("wrong answer").unwrap(),
                },
            )
            .unwrap(),
    );
    assert_eq!(
        (
            rejected.phase,
            rejected.decision_steps,
            rejected.revision.get()
        ),
        (Phase::Rejected, 3, 5)
    );
    assert_eq!(
        agent.transition(&rejected, &AgentEvent::TaskAccepted),
        Err(DecisionError::InvalidEvent)
    );
}
#[test]
fn cap_preserves_pending_inputs_but_allows_finalization() {
    let agent = SumAgent;
    assert_eq!(
        agent.transition(&initial(0), &AgentEvent::TaskAccepted),
        Err(DecisionError::DecisionLimit)
    );
    let read = next(
        agent
            .transition(&initial(1), &AgentEvent::TaskAccepted)
            .unwrap(),
    );
    assert_eq!(
        agent.transition(&read, &fixture_event(&read)),
        Err(DecisionError::DecisionLimit)
    );
    let uncertain = next(
        agent
            .transition(
                &read,
                &AgentEvent::EffectUncertain {
                    intent_id: read.pending_intent.clone().unwrap(),
                },
            )
            .unwrap(),
    );
    assert_eq!(uncertain.phase, read.phase);
    assert_eq!(uncertain.pending_intent, read.pending_intent);
    assert_eq!(uncertain.decision_steps, 1);
    assert_eq!(
        agent.transition(&uncertain, &fixture_event(&uncertain)),
        Err(DecisionError::DecisionLimit)
    );
    let rejected = next(
        agent
            .transition(
                &uncertain,
                &AgentEvent::Denied {
                    intent_id: uncertain.pending_intent.clone().unwrap(),
                    reason: BoundedText::new("denied").unwrap(),
                },
            )
            .unwrap(),
    );
    assert_eq!(
        (rejected.phase, rejected.decision_steps),
        (Phase::Rejected, 1)
    );
}
#[test]
fn rejects_unbound_evidence_wrong_phase_and_revision_overflow() {
    let agent = SumAgent;
    let read = next(
        agent
            .transition(&initial(8), &AgentEvent::TaskAccepted)
            .unwrap(),
    );
    let mut wrong = fixture_event(&read);
    if let AgentEvent::EffectSettled { intent_id, .. } = &mut wrong {
        *intent_id = content_digest(b"another");
    }
    assert_eq!(
        agent.transition(&read, &wrong),
        Err(DecisionError::InvalidEvent)
    );
    let mut wrong = fixture_event(&read);
    if let AgentEvent::EffectSettled {
        output: ReceiptOutput::Fixture { digest, .. },
        ..
    } = &mut wrong
    {
        *digest = content_digest(b"wrong");
    }
    assert_eq!(
        agent.transition(&read, &wrong),
        Err(DecisionError::MissingEvidence)
    );
    assert_eq!(
        agent.transition(&read, &model_event(&read, 22)),
        Err(DecisionError::InvalidEvent)
    );
    let mut maxed = read.clone();
    maxed.revision = SqlU64::new(MAX_SQL_INTEGER).unwrap();
    assert_eq!(
        agent.transition(
            &maxed,
            &AgentEvent::EffectUncertain {
                intent_id: maxed.pending_intent.clone().unwrap()
            }
        ),
        Err(DecisionError::Overflow)
    );
}

#[test]
fn limits_two_and_three_block_only_next_decision() {
    let agent = SumAgent;
    for limit in [2, 3] {
        let read = next(
            agent
                .transition(&initial(limit), &AgentEvent::TaskAccepted)
                .unwrap(),
        );
        let model = next(agent.transition(&read, &fixture_event(&read)).unwrap());
        if limit == 2 {
            assert_eq!(
                agent.transition(&model, &model_event(&model, 22)),
                Err(DecisionError::DecisionLimit)
            );
        } else {
            let publication = next(agent.transition(&model, &model_event(&model, 22)).unwrap());
            let awaiting = next(
                agent
                    .transition(&publication, &published(&publication))
                    .unwrap(),
            );
            let complete = next(
                agent
                    .transition(
                        &awaiting,
                        &AgentEvent::Verified {
                            accepted: true,
                            reason: BoundedText::new("accepted").unwrap(),
                        },
                    )
                    .unwrap(),
            );
            assert_eq!(
                (complete.phase, complete.decision_steps),
                (Phase::Completed, 3)
            );
            assert!(complete.pending_intent.is_none());
        }
    }
}

#[test]
fn known_failure_finalizes_at_cap_and_rejects_wrong_intent() {
    let agent = SumAgent;
    let read = next(
        agent
            .transition(&initial(1), &AgentEvent::TaskAccepted)
            .unwrap(),
    );
    let failure = |intent_id| AgentEvent::EffectSettled {
        intent_id,
        evidence_id: content_digest(b"failure"),
        output: ReceiptOutput::Failure {
            diagnostic: BoundedText::new("failed").unwrap(),
        },
    };
    assert_eq!(
        agent.transition(&read, &failure(content_digest(b"wrong"))),
        Err(DecisionError::InvalidEvent)
    );
    let rejected = next(
        agent
            .transition(&read, &failure(read.pending_intent.clone().unwrap()))
            .unwrap(),
    );
    assert_eq!(
        (
            rejected.phase,
            rejected.decision_steps,
            rejected.revision.get()
        ),
        (Phase::Rejected, 1, 2)
    );
}
