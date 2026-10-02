//! Invariant 4: representations are connected by explicit transform
//! contracts, and the runner enforces them.

mod common;

use common::{run, run_smoke};
use metron_app::{FixedSchedule, OperatorRegistry, RegistryError, ScheduleItem};
use metron_core::cost::Cost;
use metron_core::frame::{Fidelity, TransformContract, observations_frame};
use metron_core::id::{FrameId, OperatorId};
use metron_core::inquiry::{Answer, Derivation, Inquiry, Representation};
use metron_core::journal::{JournalEvent, StopReason};
use metron_core::operator::{Operator, OperatorError, OperatorKind, OperatorOutcome, OperatorSpec};
use metron_core::rng::Rng;
use metron_lab::{ANSWER_FRAME, LabWorld};
use metron_operators::{
    CompleteByDefault, ObservationsToPartialTable, frames, reference_operators, views,
};
use std::collections::BTreeSet;

#[test]
fn every_reference_transform_has_a_valid_contract_and_the_frames_connect() {
    let mut registry: OperatorRegistry<LabWorld> = OperatorRegistry::new();
    registry.register_all(reference_operators()).unwrap();
    let contracts = registry.contracts();
    assert_eq!(contracts.len(), 2);
    for (_, c) in &contracts {
        c.validate().unwrap();
    }
    let mut reachable: BTreeSet<FrameId> = BTreeSet::from([observations_frame()]);
    let mut progress = true;
    while progress {
        progress = false;
        for (_, c) in &contracts {
            if c.from.iter().all(|f| reachable.contains(f)) && reachable.insert(c.to.clone()) {
                progress = true;
            }
        }
    }
    assert!(
        reachable.contains(&FrameId::from(ANSWER_FRAME)),
        "the answer frame is not reachable from observations through contracts: {reachable:?}"
    );
    let complete = CompleteByDefault::contract();
    assert!(matches!(complete.fidelity, Fidelity::Approximate { .. }));
    assert!(
        !complete.assumes.is_empty(),
        "an approximate transform must state its assumptions"
    );
    assert!(ObservationsToPartialTable::contract().is_exact());
}

#[test]
fn the_registry_rejects_malformed_contracts() {
    struct NoSource;
    impl Operator<LabWorld> for NoSource {
        fn spec(&self) -> OperatorSpec {
            OperatorSpec::new(
                "no-source",
                OperatorKind::Transform {
                    contract: TransformContract::new(Vec::<&str>::new(), "x"),
                },
                "test double",
            )
            .writes("x")
        }
        fn apply(
            &self,
            _: &mut Inquiry,
            _: &mut LabWorld,
            _: &mut Rng,
        ) -> Result<OperatorOutcome, OperatorError> {
            Ok(OperatorOutcome::no_change(Cost::ZERO))
        }
    }
    struct WritesNothing;
    impl Operator<LabWorld> for WritesNothing {
        fn spec(&self) -> OperatorSpec {
            OperatorSpec::new(
                "writes-nothing",
                OperatorKind::Transform {
                    contract: TransformContract::new([frames::OBSERVATIONS], "x"),
                },
                "test double",
            )
        }
        fn apply(
            &self,
            _: &mut Inquiry,
            _: &mut LabWorld,
            _: &mut Rng,
        ) -> Result<OperatorOutcome, OperatorError> {
            Ok(OperatorOutcome::no_change(Cost::ZERO))
        }
    }
    let mut registry: OperatorRegistry<LabWorld> = OperatorRegistry::new();
    assert!(matches!(
        registry.register(NoSource),
        Err(RegistryError::Spec(_))
    ));
    assert!(matches!(
        registry.register(WritesNothing),
        Err(RegistryError::Spec(_))
    ));
    registry.register_all(reference_operators()).unwrap();
    assert!(matches!(
        registry.register_all(reference_operators()),
        Err(RegistryError::Duplicate(_))
    ));
}

/// Writes its view in a frame other than its contract's target.
struct Misframed;

impl Operator<LabWorld> for Misframed {
    fn spec(&self) -> OperatorSpec {
        OperatorSpec::new(
            "misframed",
            OperatorKind::Transform {
                contract: TransformContract::new(
                    [frames::OBSERVATIONS],
                    frames::TRUTH_TABLE_PARTIAL,
                ),
            },
            "test double",
        )
        .reads(views::OBSERVATIONS)
        .writes("misframed")
    }
    fn apply(
        &self,
        inquiry: &mut Inquiry,
        _: &mut LabWorld,
        _: &mut Rng,
    ) -> Result<OperatorOutcome, OperatorError> {
        inquiry.write_view(
            "misframed",
            "somewhere/else",
            Representation::Text("oops".into()),
            Derivation::by(OperatorId::from("misframed"), inquiry.steps)
                .from_view(views::OBSERVATIONS),
        );
        Ok(OperatorOutcome::progressed(Cost::ZERO))
    }
}

/// Reads a view in a frame its contract does not list.
struct WrongSource;

impl Operator<LabWorld> for WrongSource {
    fn spec(&self) -> OperatorSpec {
        OperatorSpec::new(
            "wrong-source",
            OperatorKind::Transform {
                contract: TransformContract::new([frames::TRUTH_TABLE_PARTIAL], "derived/frame"),
            },
            "test double",
        )
        .reads(views::OBSERVATIONS)
        .writes("derived")
    }
    fn apply(
        &self,
        inquiry: &mut Inquiry,
        _: &mut LabWorld,
        _: &mut Rng,
    ) -> Result<OperatorOutcome, OperatorError> {
        inquiry.write_view(
            "derived",
            "derived/frame",
            Representation::Text("from the wrong frame".into()),
            Derivation::by(OperatorId::from("wrong-source"), inquiry.steps)
                .from_view(views::OBSERVATIONS),
        );
        Ok(OperatorOutcome::progressed(Cost::ZERO))
    }
}

/// An observe operator that writes a view it never declared.
struct UndeclaredWrite;

impl Operator<LabWorld> for UndeclaredWrite {
    fn spec(&self) -> OperatorSpec {
        OperatorSpec::new("undeclared-write", OperatorKind::Observe, "test double")
    }
    fn apply(
        &self,
        inquiry: &mut Inquiry,
        _: &mut LabWorld,
        _: &mut Rng,
    ) -> Result<OperatorOutcome, OperatorError> {
        inquiry.write_view(
            "surprise",
            frames::OBSERVATIONS,
            Representation::Text("!".into()),
            Derivation::by(OperatorId::from("undeclared-write"), inquiry.steps),
        );
        Ok(OperatorOutcome::progressed(Cost::ZERO))
    }
}

/// A transform that commits an answer.
struct SneakyCommit;

impl Operator<LabWorld> for SneakyCommit {
    fn spec(&self) -> OperatorSpec {
        OperatorSpec::new(
            "sneaky-commit",
            OperatorKind::Transform {
                contract: TransformContract::new([frames::OBSERVATIONS], "x"),
            },
            "test double",
        )
        .writes("x")
    }
    fn apply(
        &self,
        inquiry: &mut Inquiry,
        _: &mut LabWorld,
        _: &mut Rng,
    ) -> Result<OperatorOutcome, OperatorError> {
        inquiry.commit(Answer {
            frame: inquiry.question.answer_frame.clone(),
            content: Representation::Text("guess".into()),
            confidence: 0.0,
            evidence: Vec::new(),
            by: OperatorId::from("sneaky-commit"),
        });
        Ok(OperatorOutcome::answered(Cost::ZERO))
    }
}

fn stop_of(run: &common::Run) -> &StopReason {
    &run.episode.outcome.as_ref().unwrap().stop
}

#[test]
fn the_runner_stops_on_contract_violations() {
    let after_probe = |op: &str| {
        FixedSchedule::new([
            ScheduleItem::once(metron_operators::ProbeNextUnobserved::ID),
            ScheduleItem::once(op),
        ])
    };
    let r = run(
        Some(8),
        1,
        vec![Box::new(Misframed)],
        after_probe("misframed"),
    );
    assert!(
        matches!(stop_of(&r), StopReason::ContractViolation { view, message, .. }
            if view.as_str() == "misframed" && message.contains("contract targets")),
        "{:?}",
        stop_of(&r)
    );
    let r = run(
        Some(8),
        1,
        vec![Box::new(WrongSource)],
        after_probe("wrong-source"),
    );
    assert!(
        matches!(stop_of(&r), StopReason::ContractViolation { message, .. }
            if message.contains("does not read")),
        "{:?}",
        stop_of(&r)
    );
    let r = run(
        Some(8),
        1,
        vec![Box::new(UndeclaredWrite)],
        FixedSchedule::of(["undeclared-write"]),
    );
    assert!(
        matches!(stop_of(&r), StopReason::ContractViolation { view, .. } if view.as_str() == "surprise"),
        "{:?}",
        stop_of(&r)
    );
    let r = run(
        Some(8),
        1,
        vec![Box::new(SneakyCommit)],
        after_probe("sneaky-commit"),
    );
    assert!(
        matches!(stop_of(&r), StopReason::ContractViolation { message, .. }
            if message.contains("committed an answer")),
        "{:?}",
        stop_of(&r)
    );
    assert!(!r.verdict.answered || !r.verdict.frame_accepted || !r.verdict.correct);
}

/// A consult operator that writes outside the consultations frame.
struct ChattyConsult;

impl Operator<LabWorld> for ChattyConsult {
    fn spec(&self) -> OperatorSpec {
        OperatorSpec::new(
            "chatty-consult",
            OperatorKind::Consult {
                service: "llm".into(),
            },
            "test double",
        )
        .writes("notes")
    }
    fn apply(
        &self,
        inquiry: &mut Inquiry,
        _: &mut LabWorld,
        _: &mut Rng,
    ) -> Result<OperatorOutcome, OperatorError> {
        inquiry.write_view(
            "notes",
            frames::TRUTH_TABLE_COMPLETE,
            Representation::Text("not a consultation".into()),
            Derivation::by(OperatorId::from("chatty-consult"), inquiry.steps),
        );
        Ok(OperatorOutcome::progressed(Cost::ZERO))
    }
}

#[test]
fn consult_operators_write_only_the_consultations_frame() {
    let r = run(
        Some(8),
        1,
        vec![Box::new(ChattyConsult)],
        FixedSchedule::of(["chatty-consult"]),
    );
    assert!(
        matches!(stop_of(&r), StopReason::ContractViolation { view, message, .. }
            if view.as_str() == "notes" && message.contains("consultations")),
        "{:?}",
        stop_of(&r)
    );
}

#[test]
fn views_written_by_transforms_carry_their_contract_and_provenance() {
    let run = run_smoke(Some(8), 1);
    let mut partial_contract = None;
    let mut complete_contract = None;
    let mut observations_contract = Some(TransformContract::new(["x"], "y"));
    for entry in &run.episode.entries {
        if let JournalEvent::ViewWritten {
            view,
            contract,
            observations,
            ..
        } = &entry.event
        {
            match view.as_str() {
                views::PARTIAL_TABLE => {
                    partial_contract = contract.clone();
                    assert_eq!(
                        observations.len(),
                        8,
                        "partial table rests on all observations"
                    );
                }
                views::COMPLETE_TABLE => complete_contract = contract.clone(),
                views::OBSERVATIONS => observations_contract = contract.clone(),
                other => panic!("unexpected view {other}"),
            }
        }
    }
    assert_eq!(
        partial_contract,
        Some(ObservationsToPartialTable::contract())
    );
    assert_eq!(complete_contract, Some(CompleteByDefault::contract()));
    assert_eq!(
        observations_contract, None,
        "the observations frame has no contract"
    );
    let answered = run.episode.entries.iter().find_map(|e| match &e.event {
        JournalEvent::Answered {
            independent_observations,
            confidence,
            ..
        } => Some((*independent_observations, *confidence)),
        _ => None,
    });
    assert_eq!(answered, Some((8, 1.0)));
    let answer = run.inquiry.answer.as_ref().unwrap();
    assert_eq!(answer.evidence.len(), 8);
    assert!(
        answer.evidence.iter().all(|e| e.via.len() == 4),
        "evidence names the full operator path"
    );
}
