//! Invariant 3: every externally executed operation produces a resource
//! receipt that reaches the journal, whatever the operator did with it.

mod common;

use common::{run, run_smoke, smoke_schedule};
use metron_app::{FixedSchedule, ScheduleItem};
use metron_core::cost::Cost;
use metron_core::frame::TransformContract;
use metron_core::id::OperatorId;
use metron_core::inquiry::{Derivation, Inquiry, Representation};
use metron_core::journal::{JournalEvent, StopReason};
use metron_core::operator::{Operator, OperatorError, OperatorKind, OperatorOutcome, OperatorSpec};
use metron_core::rng::Rng;
use metron_core::world::Oracle;
use metron_lab::LabWorld;
use metron_operators::{ProbeNextUnobserved, frames, views};
use std::collections::BTreeSet;

#[test]
fn every_probe_has_a_receipt_in_the_journal() {
    let run = run_smoke(Some(8), 1);
    let receipts: Vec<_> = run.episode.receipts().collect();
    assert_eq!(run.world.probes_answered(), 8);
    assert_eq!(receipts.len(), 8, "one receipt per probe");
    assert_eq!(run.inquiry.observations.len(), 8);
    let receipt_ids: BTreeSet<_> = receipts.iter().map(|r| r.id).collect();
    for observation in &run.inquiry.observations {
        assert!(
            receipt_ids.contains(&observation.receipt),
            "{} has no receipt",
            observation.id
        );
        assert!(
            receipts
                .iter()
                .any(|r| r.observations.contains(&observation.id)),
            "no receipt names {}",
            observation.id
        );
    }
    assert!(receipts.iter().all(|r| r.verify()));
    assert_eq!(run.episode.receipt_cost(), Cost::probes(8));
    assert_eq!(
        run.inquiry.spent.oracle_probes, 8,
        "receipt cost is charged to the inquiry"
    );
    run.episode.verify().unwrap();
    assert!(run.verdict.correct);
}

/// A transform operator that probes the oracle, which its kind forbids.
struct RogueTransform;

impl Operator<LabWorld> for RogueTransform {
    fn spec(&self) -> OperatorSpec {
        OperatorSpec::new(
            "rogue-transform",
            OperatorKind::Transform {
                contract: TransformContract::new([frames::OBSERVATIONS], "rogue/frame"),
            },
            "test double: probes from inside a transform",
        )
        .writes("rogue")
    }

    fn apply(
        &self,
        inquiry: &mut Inquiry,
        world: &mut LabWorld,
        _rng: &mut Rng,
    ) -> Result<OperatorOutcome, OperatorError> {
        let probe = Representation::Json(serde_json::json!({"assignment": [1, 1, 1]}));
        let observation = world.probe(&OperatorId::from("rogue-transform"), inquiry.id, &probe)?;
        inquiry.write_view(
            "rogue",
            "rogue/frame",
            observation.result.clone(),
            Derivation::by(OperatorId::from("rogue-transform"), inquiry.steps),
        );
        Ok(OperatorOutcome::progressed(Cost::ZERO))
    }
}

#[test]
fn a_rogue_operator_cannot_suppress_its_receipt() {
    let schedule = FixedSchedule::new([ScheduleItem::once("rogue-transform")]);
    let run = run(Some(8), 1, vec![Box::new(RogueTransform)], schedule);
    assert_eq!(run.world.probes_answered(), 1);
    assert_eq!(
        run.episode.receipts().count(),
        1,
        "the receipt is journaled regardless"
    );
    assert!(matches!(
        run.episode.outcome.as_ref().unwrap().stop,
        StopReason::ContractViolation { ref operator, .. } if operator.as_str() == "rogue-transform"
    ));
    assert_eq!(
        run.inquiry.spent.oracle_probes, 1,
        "the probe is still charged"
    );
}

/// An observe operator that probes and then fails.
struct ProbeThenFail;

impl Operator<LabWorld> for ProbeThenFail {
    fn spec(&self) -> OperatorSpec {
        OperatorSpec::new("probe-then-fail", OperatorKind::Observe, "test double")
            .writes(views::OBSERVATIONS)
    }

    fn apply(
        &self,
        inquiry: &mut Inquiry,
        world: &mut LabWorld,
        _rng: &mut Rng,
    ) -> Result<OperatorOutcome, OperatorError> {
        let probe = Representation::Json(serde_json::json!({"assignment": [0, 1, 0]}));
        world.probe(&OperatorId::from("probe-then-fail"), inquiry.id, &probe)?;
        Err(OperatorError::Internal(
            "dropped the observation on the floor".into(),
        ))
    }
}

#[test]
fn a_failing_operator_still_leaves_its_receipt() {
    let schedule = FixedSchedule::new([ScheduleItem::once("probe-then-fail")]);
    let run = run(Some(8), 1, vec![Box::new(ProbeThenFail)], schedule);
    assert_eq!(run.episode.receipts().count(), 1);
    assert!(matches!(
        run.episode.outcome.as_ref().unwrap().stop,
        StopReason::OperatorFailed { .. }
    ));
    assert_eq!(run.inquiry.spent.oracle_probes, 1);
    run.episode.verify().unwrap();
}

#[test]
fn the_protocol_cap_is_enforced_by_the_oracle_not_the_operator() {
    let run = run_smoke(Some(4), 1);
    assert_eq!(run.world.probes_answered(), 4);
    assert_eq!(run.episode.receipts().count(), 4);
    let skipped_probes = run
        .episode
        .entries
        .iter()
        .filter(|e| {
            matches!(&e.event, JournalEvent::OperatorApplied { operator, status, .. }
                if operator.as_str() == ProbeNextUnobserved::ID
                    && matches!(status, metron_core::operator::StepStatus::NoChange))
        })
        .count();
    assert_eq!(
        skipped_probes, 4,
        "probes beyond the cap are no-ops with a note"
    );
    assert!(run.verdict.answered && !run.verdict.correct);
    assert_eq!(run.verdict.probes_answered, 4);
    let _ = smoke_schedule(8);
}
