use metron_agent::{ENGINE_VERSION, SumAgent};
use metron_protocol::*;

fn initial(limit: u8) -> AgentState {
    AgentState {
        coding_context: None,
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

fn local_profile() -> LocalProfile {
    LocalProfile {
        schema_version: SchemaVersion::V2,
        auxiliary_repository: BoundedText::new("LiquidAI/LFM3-1.7B").unwrap(),
        auxiliary_revision: BoundedText::new("1da06bec6a819da10b54c67e5c90972b46d81871").unwrap(),
        artifact_repository: BoundedText::new("LiquidAI/LFM3-1.7B-GGUF").unwrap(),
        artifact_revision: BoundedText::new("cd0c74c575b5c416f3011b5b4c298077f26ca8c2").unwrap(),
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
fn local_engine_replays_bound_observations_and_rejects_wrong_profile() {
    let profile = local_profile();
    let agent = metron_agent::LocalSumAgent::new(&profile).unwrap();
    let start = initial(8);
    let Transition::Decision {
        next: read_state,
        intent,
    } = agent.transition(&start, &AgentEvent::TaskAccepted).unwrap()
    else {
        panic!("read")
    };
    assert_eq!(intent.schema_version, SchemaVersion::V2);
    let Transition::Decision {
        next: model_state,
        intent,
    } = agent
        .transition(&read_state, &fixture_event(&read_state))
        .unwrap()
    else {
        panic!("model")
    };
    let IntentKind::RequestLocalModel {
        input_digest,
        profile_digest,
        role,
        output_schema,
        ..
    } = intent.kind
    else {
        panic!("local model")
    };
    assert_eq!(profile_digest, profile.digest().unwrap());
    assert_eq!(
        agent.transition(&model_state, &model_event(&model_state, 22)),
        Err(DecisionError::InvalidEvent)
    );
    assert_eq!(
        (role, output_schema),
        (LocalRole::Solver, LocalOutputSchema::Sum)
    );
    let observation = LocalObservation {
        schema_version: SchemaVersion::V2,
        profile_digest,
        request_digest: content_digest(b"request"),
        input_digest: input_digest.clone(),
        content: BoundedText::new("{\"count\":3,\"sum\":22}").unwrap(),
        reasoning: None,
        prompt_tokens: 12,
        completion_tokens: 8,
        submission_cohort: SubmissionCohort {
            id: BoundedText::new("c1").unwrap(),
            size: 1,
            position: 0,
        },
        backend_batch: BatchProvenance::Unknown,
        response: Some(LocalResponse::Sum(SumResponse { count: 3, sum: 22 })),
    };
    let event = AgentEvent::EffectSettled {
        intent_id: model_state.pending_intent.clone().unwrap(),
        evidence_id: content_digest(b"evidence"),
        output: ReceiptOutput::LocalModel {
            observation: Box::new(observation.clone()),
        },
    };
    let first = agent.transition(&model_state, &event).unwrap();
    assert_eq!(first, agent.transition(&model_state, &event).unwrap());
    let Transition::Decision { intent, .. } = first else {
        panic!("publication")
    };
    assert_eq!(intent.schema_version, SchemaVersion::V2);
    assert!(
        matches!(intent.kind, IntentKind::PublishArtifact { candidate } if candidate.sum == 22 && candidate.input_digest == input_digest)
    );
    let mut bad = observation;
    bad.profile_digest = content_digest(b"wrong");
    let invalid = AgentEvent::EffectSettled {
        intent_id: model_state.pending_intent.clone().unwrap(),
        evidence_id: content_digest(b"evidence"),
        output: ReceiptOutput::LocalModel {
            observation: Box::new(bad),
        },
    };
    assert_eq!(
        agent.transition(&model_state, &invalid),
        Err(DecisionError::MissingEvidence)
    );
}
