//! The structure-keyed laboratory of ADR 0015: each class has a learner
//! that identifies its members, a cascade of learners solves every class,
//! and the structure profile never rules out the target's own class.

use metron_app::{EpisodeRunner, RunConfig, RunOutcome};
use metron_cli::{ComposedWorld, registry, schedule_from};
use metron_core::clock::ManualClock;
use metron_core::hash::ContentHash;
use metron_core::id::{EpisodeId, InquiryId};
use metron_core::inquiry::Inquiry;
use metron_lab::families::FamilyParams;
use metron_lab::manifest::ScheduleStep;
use metron_lab::{Family, LabWorld, PoolSpec, Protocol, TaskSet, TaskSetSpec};
use metron_operators::{JuntaSolve, StructureProfile, SymmetricSolve, frames, views};

fn steps(json: &str) -> Vec<ScheduleStep> {
    serde_json::from_str(json).unwrap()
}

/// One learner: its probe design, `verify` random probes, its solve, commit.
fn learner(class: &str, verify: u32) -> String {
    let (probe, solve) = match class {
        "affine" => ("affine-probe", "affine-solve"),
        "symmetric" => ("symmetric-probe", "symmetric-solve"),
        "junta" => ("junta-probe", "junta-solve"),
        other => panic!("no learner for {other}"),
    };
    format!(
        r#"{{"operator": "{probe}"}}, {{"operator": "probe-random-unobserved", "repeat": {verify}}},
           {{"operator": "{solve}"}}, {{"operator": "commit-truth-table"}}"#
    )
}

/// Learners in `order`, then the truth table.
fn cascade(order: &[&str], verify: u32) -> Vec<ScheduleStep> {
    let mut parts: Vec<String> = order.iter().map(|c| learner(c, verify)).collect();
    parts.push(
        r#"{"operator": "probe-next-unobserved", "repeat": 256}, {"operator": "observations-to-partial-table"},
           {"operator": "complete-table-by-default"}, {"operator": "commit-truth-table"}"#
            .to_owned(),
    );
    steps(&format!("[{}]", parts.join(", ")))
}

fn task_set(seed: u64, per_class: usize) -> TaskSet {
    TaskSet::generate(
        &TaskSetSpec {
            arity: 8,
            pool: PoolSpec {
                families: Family::STRUCTURE_KEYED.to_vec(),
                per_family: per_class,
                params: FamilyParams::default(),
            },
            targets_per_family: per_class,
            split: Default::default(),
            publish_pool: false,
            class_hash_splits: true,
        },
        seed,
    )
}

struct Outcome {
    correct: bool,
    probes: u64,
    solver: String,
    inquiry: Inquiry,
}

fn solve(set: &TaskSet, task: &metron_lab::Task, schedule: &[ScheduleStep]) -> Outcome {
    let mut world = ComposedWorld::new(LabWorld::for_task(
        set,
        task,
        Protocol {
            max_probes: Some(256),
        },
    ));
    let mut inquiry = Inquiry::new(InquiryId(1), world.lab().question());
    let runner =
        EpisodeRunner::new(registry().unwrap(), ManualClock::new(1)).with_config(RunConfig {
            max_steps: 1_024,
            stall_limit: 0,
        });
    let mut sched = schedule_from(schedule);
    let RunOutcome::Completed(_) = runner
        .run(
            EpisodeId(1),
            task.id.len() as u64,
            ContentHash::GENESIS,
            &mut inquiry,
            &mut world,
            &mut sched,
        )
        .unwrap()
    else {
        panic!("no consultation here");
    };
    let verdict = world.lab().judge(&inquiry);
    let solver = inquiry
        .view_str(views::COMPLETE_TABLE)
        .map(|v| v.derivation.operator.to_string())
        .unwrap_or_default();
    Outcome {
        correct: verdict.correct,
        probes: verdict.probes_answered,
        solver,
        inquiry,
    }
}

#[test]
fn each_learner_identifies_its_own_class() {
    let set = task_set(41, 8);
    for (class, solver, max_probes) in [
        ("affine", "affine-solve", 9 + 8),
        ("symmetric", "symmetric-solve", 9 + 8),
        ("junta", "junta-solve", 64),
    ] {
        let family = Family::parse(class).unwrap();
        let schedule = steps(&format!("[{}]", learner(class, 8)));
        let mut probes = Vec::new();
        for task in set.of_family(family) {
            let out = solve(&set, task, &schedule);
            assert!(out.correct, "{class} learner failed on {}", task.id);
            assert_eq!(out.solver, solver);
            assert!(out.probes <= max_probes, "{class}: {} probes", out.probes);
            probes.push(out.probes);
        }
        println!("{class} learner on its own class: probes {probes:?}");
    }
}

#[test]
fn a_cascade_of_learners_solves_every_class() {
    let set = task_set(42, 10);
    let schedule = cascade(&["affine", "symmetric", "junta"], 8);
    let mut wrong = Vec::new();
    for task in &set.tasks {
        let out = solve(&set, task, &schedule);
        if !out.correct {
            wrong.push(format!("{} via {}", task.id, out.solver));
        }
    }
    println!(
        "cascade affine -> symmetric -> junta: {} of {} wrong {wrong:?}",
        wrong.len(),
        set.tasks.len()
    );
    assert!(
        wrong.len() * 10 <= set.tasks.len(),
        "verification lets through at most a tenth"
    );
}

#[test]
fn the_structure_profile_never_rules_out_the_targets_class() {
    let set = task_set(43, 10);
    let schedule = steps(r#"[{"operator": "affine-probe"}, {"operator": "structure-profile"}]"#);
    for task in &set.tasks {
        let out = solve(&set, task, &schedule);
        let profile = out
            .inquiry
            .view_str(views::STRUCTURE_PROFILE)
            .expect("profile written");
        assert_eq!(profile.frame.as_str(), frames::STRUCTURE_PROFILE);
        let classes = profile.content.as_json().unwrap()["classes"]
            .as_array()
            .unwrap()
            .clone();
        let total: f64 = classes
            .iter()
            .map(|c| c["posterior"].as_f64().unwrap())
            .sum();
        assert!((total - 1.0).abs() < 1e-9);
        let own = classes
            .iter()
            .find(|c| c["name"] == task.target.family.label())
            .unwrap();
        assert!(
            own["consistent"].as_f64().unwrap() >= 1.0,
            "{}: its own class has no consistent member",
            task.id
        );
    }
}

#[test]
fn structural_contracts_validate_and_reach_the_answer_frame() {
    for contract in [
        SymmetricSolve::contract(),
        JuntaSolve::contract(),
        StructureProfile::contract(),
    ] {
        contract.validate().unwrap();
        assert!(
            contract
                .from
                .iter()
                .any(|f| f.as_str() == frames::OBSERVATIONS)
        );
    }
    assert_eq!(
        SymmetricSolve::contract().to.as_str(),
        frames::TRUTH_TABLE_COMPLETE
    );
    assert_eq!(
        JuntaSolve::contract().to.as_str(),
        frames::TRUTH_TABLE_COMPLETE
    );
    assert!(StructureProfile::contract().is_exact());
}
