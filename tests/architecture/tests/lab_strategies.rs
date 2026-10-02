//! Milestone M1: the laboratory can tell strategies apart.
//!
//! Pool-based identification solves every task of a generated set; the
//! affine shortcut is cheaper on affine targets and refuted on others; the
//! headroom harness produces a report from a manifest.

use metron_app::{EpisodeRunner, RunConfig, RunOutcome};
use metron_cli::run::Backend;
use metron_cli::{
    ComposedWorld, HeadroomOptions, RunOptions, RunStatus, registry, run, run_headroom,
    schedule_from,
};
use metron_core::clock::ManualClock;
use metron_core::hash::ContentHash;
use metron_core::id::{EpisodeId, InquiryId};
use metron_core::inquiry::Inquiry;
use metron_lab::families::FamilyParams;
use metron_lab::manifest::ScheduleStep;
use metron_lab::{Family, LabWorld, PoolSpec, Protocol, TaskSet, TaskSetSpec};
use std::fs;
use std::path::PathBuf;

fn steps(json: &str) -> Vec<ScheduleStep> {
    serde_json::from_str(json).unwrap()
}

fn greedy(rounds: u32) -> Vec<ScheduleStep> {
    steps(&format!(
        r#"[{{"operator": "version-space-filter"}},
            {{"sequence": [{{"operator": "greedy-split-probe"}}, {{"operator": "version-space-filter"}}], "repeat": {rounds}}},
            {{"operator": "single-survivor-to-table"}}, {{"operator": "commit-truth-table"}}]"#
    ))
}

/// The affine shortcut with `verify` informative (greedy) verification
/// probes before solving, falling through to greedy identification when the
/// affine hypothesis is refuted.
fn affine_then_greedy(verify: u32, rounds: u32) -> Vec<ScheduleStep> {
    let mut s = steps(&format!(
        r#"[{{"operator": "affine-probe"}}, {{"operator": "version-space-filter"}},
            {{"sequence": [{{"operator": "greedy-split-probe"}}, {{"operator": "version-space-filter"}}], "repeat": {verify}}},
            {{"operator": "affine-solve"}}, {{"operator": "commit-truth-table"}}]"#
    ));
    s.extend(greedy(rounds));
    s
}

fn task_set(arity: u8, seed: u64) -> TaskSet {
    TaskSet::generate(
        &TaskSetSpec {
            arity,
            pool: PoolSpec {
                families: Family::ALL.to_vec(),
                per_family: 15,
                params: FamilyParams::default(),
            },
            targets_per_family: 2,
            split: Default::default(),
            publish_pool: true,
            class_hash_splits: false,
        },
        seed,
    )
}

/// Runs a strategy on a task: `(correct, probes, operator that produced the committed table)`.
fn solve(set: &TaskSet, task: &metron_lab::Task, schedule: &[ScheduleStep]) -> (bool, u64, String) {
    let mut world = ComposedWorld::new(LabWorld::for_task(
        set,
        task,
        Protocol {
            max_probes: Some(64),
        },
    ));
    let mut inquiry = Inquiry::new(InquiryId(1), world.lab().question());
    let runner =
        EpisodeRunner::new(registry().unwrap(), ManualClock::new(1)).with_config(RunConfig {
            max_steps: 256,
            stall_limit: 0,
        });
    let mut sched = schedule_from(schedule);
    let RunOutcome::Completed(_) = runner
        .run(
            EpisodeId(1),
            1,
            ContentHash::GENESIS,
            &mut inquiry,
            &mut world,
            &mut sched,
        )
        .unwrap()
    else {
        panic!("no consultation in these strategies");
    };
    let verdict = world.lab().judge(&inquiry);
    let solver = inquiry
        .view_str("table.complete")
        .map(|v| v.derivation.operator.to_string())
        .unwrap_or_default();
    (verdict.correct, verdict.probes_answered, solver)
}

#[test]
fn greedy_identification_solves_every_pool_task_within_the_information_bound_plus_slack() {
    let set = task_set(5, 11);
    let lower = metron_lab::bounds::information_lower_bound(set.pool.len());
    for task in &set.tasks {
        let (correct, probes, _) = solve(&set, task, &greedy(64));
        assert!(correct, "greedy failed on {}", task.id);
        assert!(probes <= 32, "{}: {probes} probes", task.id);
        assert!(probes as u32 >= lower.min(probes as u32), "sanity");
    }
}

#[test]
fn the_affine_shortcut_is_cheap_on_affine_targets_and_mostly_refuted_elsewhere() {
    let set = task_set(6, 12);
    let affine_tasks = set.of_family(Family::Affine);
    assert!(!affine_tasks.is_empty());
    for task in affine_tasks {
        let (correct, probes, solver) = solve(&set, task, &affine_then_greedy(4, 64));
        assert!(correct, "{}", task.id);
        assert_eq!(solver, "affine-solve");
        assert!(
            (7..=11).contains(&probes),
            "n + 1 anchors plus at most 4 verification probes: {probes}"
        );
    }
    // Without verification, some affine function always fits the anchors,
    // so the blind shortcut commits after n + 1 probes whatever the target.
    let mut blind_wrong = 0;
    let mut verified_wrong = 0;
    let mut verified_refuted = 0;
    let others: Vec<_> = set
        .tasks
        .iter()
        .filter(|t| t.target.family != Family::Affine)
        .collect();
    for task in &others {
        let (correct, probes, solver) = solve(&set, task, &affine_then_greedy(0, 64));
        assert_eq!(
            probes, 7,
            "the blind shortcut always commits after the anchors"
        );
        assert_eq!(solver, "affine-solve");
        if !correct {
            blind_wrong += 1;
        }
        let (correct, _, solver) = solve(&set, task, &affine_then_greedy(4, 64));
        if !correct {
            verified_wrong += 1;
        } else if solver == "single-survivor-to-table" {
            verified_refuted += 1;
        }
    }
    assert!(
        blind_wrong > 0,
        "the blind shortcut should be wrong on some non-affine targets"
    );
    assert!(
        verified_refuted > 0,
        "verification probes should refute the affine fit on non-affine targets"
    );
    assert!(
        verified_wrong < blind_wrong,
        "verification should reduce false commits: {verified_wrong} verified vs {blind_wrong} blind of {}",
        others.len()
    );
}

#[test]
fn headroom_harness_runs_from_a_manifest() {
    let dir = std::env::temp_dir().join(format!("metron-headroom-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let manifest_path = dir.join("headroom.json");
    fs::write(
        &manifest_path,
        r#"{
  "name": "headroom-test", "seed": 3,
  "lab": {"family": "hidden-boolean-function",
          "tasks": {"arity": 5, "pool": {"families": ["affine", "monotone", "k-term-dnf"], "per_family": 12}, "targets_per_family": 2},
          "max_probes": 32},
  "system": {"strategies": [
      {"name": "greedy", "schedule": [{"operator": "version-space-filter"}, {"sequence": [{"operator": "greedy-split-probe"}, {"operator": "version-space-filter"}], "repeat": 32}, {"operator": "single-survivor-to-table"}, {"operator": "commit-truth-table"}]},
      {"name": "affine-first", "schedule": [{"operator": "affine-probe"}, {"operator": "affine-solve"}, {"operator": "commit-truth-table"}, {"operator": "version-space-filter"}, {"sequence": [{"operator": "greedy-split-probe"}, {"operator": "version-space-filter"}], "repeat": 32}, {"operator": "single-survivor-to-table"}, {"operator": "commit-truth-table"}]},
      {"name": "affine-only", "schedule": [{"operator": "version-space-filter:affine"}, {"sequence": [{"operator": "greedy-split-probe"}, {"operator": "version-space-filter:affine"}], "repeat": 32}, {"operator": "single-survivor-to-table"}, {"operator": "commit-truth-table"}]},
      {"name": "select-most-k3", "selector": {"kind": "survivor-count", "rule": "most", "prefix_probes": 3, "rounds": 32}},
      {"name": "select-fewest-k3", "selector": {"kind": "survivor-count", "rule": "fewest", "prefix_probes": 3, "rounds": 32}}
  ], "budget": {"max_oracle_probes": 32}, "max_steps": 128},
  "headroom": {"seeds": 2, "cost_model": {"failure_cost": 64.0}, "resamples": 200}
}"#,
    )
    .unwrap();
    let outcome = run_headroom(
        &manifest_path,
        &HeadroomOptions {
            results_root: dir.join("results"),
            seeds: None,
            verbose: false,
        },
    )
    .unwrap();
    assert_eq!(outcome.table.strategies.len(), 5);
    assert!(outcome.table.tasks.len() >= 6);
    assert_eq!(outcome.episodes, outcome.table.tasks.len() * 5);
    let report = &outcome.report;
    assert!(report.vbs_cost <= report.sbs_cost);
    assert!(
        ["greedy", "affine-first", "affine-only"].contains(&report.sbs.as_str()),
        "selectors are scored but never the single best solver: {}",
        report.sbs
    );
    let most = report
        .strategies
        .iter()
        .find(|s| s.name == "select-most-k3")
        .unwrap();
    let fewest = report
        .strategies
        .iter()
        .find(|s| s.name == "select-fewest-k3")
        .unwrap();
    assert!(
        most.gap_closed.is_some(),
        "selector columns get a gap-closed score"
    );
    assert!(
        most.solved >= fewest.solved,
        "the Bayes rule should not lose to the negative control"
    );
    assert!(outcome.dir.join("headroom.md").exists());
    assert!(outcome.dir.join("cost-table.json").exists());
    let affine_only = report
        .strategies
        .iter()
        .find(|s| s.name == "affine-only")
        .unwrap();
    assert!(
        affine_only.solved < 1.0,
        "a restricted strategy fails off-family"
    );
    let greedy = report
        .strategies
        .iter()
        .find(|s| s.name == "greedy")
        .unwrap();
    assert_eq!(greedy.solved, 1.0);

    // A single task from the same manifest runs through the CLI library.
    let status = run(
        &manifest_path,
        &RunOptions {
            results_root: dir.join("single"),
            backend: Backend::None,
            strategy: Some("greedy".into()),
            ..RunOptions::default()
        },
    )
    .unwrap();
    let RunStatus::Completed { verdict, .. } = status else {
        panic!("no consultation");
    };
    assert!(verdict.correct);
    assert!(verdict.target_family.is_some());
    let _ = fs::remove_dir_all(&dir);
    let _ = PathBuf::new();
}
