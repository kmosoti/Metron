//! Both public coordinators preserve pure decision and evidence boundaries.
use metron_protocol::*;
#[test]
fn both_coding_modes_are_deterministic_bounded_and_reject_unbound_events() {
    for mode in [EngineMode::Agent, EngineMode::Harness] {
        let (task, policy, _) = metron_testkit::coding::fixture("cognition", mode);
        let engine: Box<dyn Cognition> = match mode {
            EngineMode::Agent => Box::new(metron_agent::CodingAgent::new(&task, &policy).unwrap()),
            EngineMode::Harness => {
                Box::new(metron_harness::planner::CodingPlanner::new(&task, &policy).unwrap())
            }
        };
        let state = AgentState {
            coding_context: None,
            task_id: task.data().task_id.clone(),
            genesis_digest: content_digest(b"genesis"),
            max_decision_steps: 16,
            revision: SqlU64::new(0).unwrap(),
            decision_steps: 0,
            phase: Phase::New,
            pending_intent: None,
        };
        let first = engine
            .transition(&state, &AgentEvent::TaskAccepted)
            .unwrap();
        assert_eq!(
            first,
            engine
                .transition(&state, &AgentEvent::TaskAccepted)
                .unwrap()
        );
        let mut capped = state.clone();
        capped.max_decision_steps = 0;
        assert_eq!(
            engine.transition(&capped, &AgentEvent::TaskAccepted),
            Err(DecisionError::DecisionLimit)
        );
        let mut overflow = state.clone();
        overflow.revision = SqlU64::new(i64::MAX as u64).unwrap();
        assert_eq!(
            engine.transition(&overflow, &AgentEvent::TaskAccepted),
            Err(DecisionError::Overflow)
        );
        assert!(
            engine
                .transition(
                    &state,
                    &AgentEvent::Verified {
                        accepted: true,
                        reason: BoundedText::new("unbound").unwrap()
                    }
                )
                .is_err()
        );
        let Transition::Decision { next, .. } = first else {
            panic!("decision")
        };
        let wrong = AgentEvent::EffectSettled {
            intent_id: content_digest(b"wrong-intent"),
            evidence_id: content_digest(b"evidence"),
            output: ReceiptOutput::Failure {
                diagnostic: BoundedText::new("controlled").unwrap(),
            },
        };
        assert!(engine.transition(&next, &wrong).is_err());
    }
}
