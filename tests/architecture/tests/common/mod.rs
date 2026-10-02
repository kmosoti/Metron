//! Shared composition for the runtime invariants: the same wiring the CLI
//! does, with a manual clock.

#![allow(dead_code)]

use metron_app::{EpisodeRunner, FixedSchedule, OperatorRegistry, RunConfig, ScheduleItem};
use metron_core::clock::ManualClock;
use metron_core::cost::Budget;
use metron_core::hash::ContentHash;
use metron_core::id::{EpisodeId, InquiryId};
use metron_core::inquiry::Inquiry;
use metron_core::journal::Episode;
use metron_core::operator::Operator;
use metron_lab::{BooleanFixture, LabWorld, Protocol, Verdict};
use metron_operators::{
    CommitTruthTable, CompleteByDefault, ObservationsToPartialTable, ProbeNextUnobserved,
    reference_operators,
};

pub const MAJORITY3: &str = r#"{"name":"majority-3","arity":3,"rows":"00010111"}"#;

pub fn world(fixture_json: &str, max_probes: Option<u64>) -> LabWorld {
    let fixture = BooleanFixture::from_json(fixture_json).expect("fixture parses");
    LabWorld::new(
        fixture.seal().expect("fixture seals"),
        Protocol { max_probes },
    )
}

pub fn registry() -> OperatorRegistry<LabWorld> {
    let mut registry = OperatorRegistry::new();
    registry
        .register_all(reference_operators())
        .expect("reference operators register");
    registry
}

pub fn smoke_schedule(probes: u32) -> FixedSchedule {
    FixedSchedule::new([
        ScheduleItem::times(ProbeNextUnobserved::ID, probes),
        ScheduleItem::once(ObservationsToPartialTable::ID),
        ScheduleItem::once(CompleteByDefault::ID),
        ScheduleItem::once(CommitTruthTable::ID),
    ])
}

pub struct Run {
    pub episode: Episode,
    pub inquiry: Inquiry,
    pub world: LabWorld,
    pub verdict: Verdict,
}

/// Runs `schedule` against the majority-3 target with the reference
/// operators plus `extra`, under `max_probes`, and judges the result.
pub fn run(
    max_probes: Option<u64>,
    seed: u64,
    extra: Vec<Box<dyn Operator<LabWorld>>>,
    mut schedule: FixedSchedule,
) -> Run {
    let mut world = world(MAJORITY3, max_probes);
    let mut inquiry = Inquiry::new(InquiryId(1), world.question())
        .with_budget(Budget::UNLIMITED.with_oracle_probes(64));
    let mut registry = registry();
    registry
        .register_all(extra)
        .expect("extra operators register");
    let runner = EpisodeRunner::new(registry, ManualClock::new(1_700_000_000_000_000_000))
        .with_config(RunConfig {
            max_steps: 64,
            stall_limit: 0,
        });
    let episode = runner
        .run(
            EpisodeId(1),
            seed,
            ContentHash::of_bytes(b"test-manifest"),
            &mut inquiry,
            &mut world,
            &mut schedule,
        )
        .expect("run completes");
    let verdict = world.judge(&inquiry);
    Run {
        episode,
        inquiry,
        world,
        verdict,
    }
}

pub fn run_smoke(max_probes: Option<u64>, seed: u64) -> Run {
    run(max_probes, seed, Vec::new(), smoke_schedule(8))
}
