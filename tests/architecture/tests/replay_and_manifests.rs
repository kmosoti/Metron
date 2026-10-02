//! Replay: the same manifest and seed produce the same journal. The
//! committed manifests and fixtures are runnable.

mod common;

use common::run_smoke;
use metron_architecture_tests::workspace_root;
use metron_cli::registry;
use metron_core::journal::JournalEvent;
use metron_lab::{BooleanFixture, Manifest, TaskSet};
use std::fs;

#[test]
fn identical_runs_produce_identical_journals() {
    let a = run_smoke(Some(8), 7);
    let b = run_smoke(Some(8), 7);
    assert_eq!(a.episode.head_hash(), b.episode.head_hash());
    assert_eq!(a.episode.entries.len(), b.episode.entries.len());
    for (x, y) in a.episode.entries.iter().zip(&b.episode.entries) {
        assert_eq!(x.event.replay_projection(), y.event.replay_projection());
        assert_eq!(x.hash, y.hash);
    }
    assert_eq!(a.inquiry.answer, b.inquiry.answer);
    assert_eq!(a.verdict, b.verdict);
    let started = a
        .episode
        .entries
        .iter()
        .filter(|e| matches!(e.event, JournalEvent::EpisodeStarted { .. }))
        .count();
    let ended = a
        .episode
        .entries
        .iter()
        .filter(|e| matches!(e.event, JournalEvent::EpisodeEnded { .. }))
        .count();
    assert_eq!((started, ended), (1, 1));
}

#[test]
fn committed_manifests_and_fixtures_are_runnable() {
    let root = workspace_root();
    let manifests = root.join("experiments/manifests");
    let registry = registry().unwrap();
    let mut seen = 0;
    for entry in fs::read_dir(&manifests).unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        seen += 1;
        let manifest = Manifest::from_json(&fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        if let Some(fixture) = manifest.lab.fixture() {
            let fixture_path = root.join(fixture);
            let fixture = BooleanFixture::from_json(&fs::read_to_string(&fixture_path).unwrap())
                .unwrap_or_else(|e| panic!("{}: {e}", fixture_path.display()));
            fixture.seal().unwrap();
        }
        if let Some(spec) = manifest.lab.tasks() {
            let set = TaskSet::generate(spec, manifest.seed);
            set.verify_split_hygiene().unwrap();
            assert!(!set.tasks.is_empty(), "{}: empty task set", path.display());
        }
        for strategy in manifest.strategies() {
            let mut ops = Vec::new();
            for step in &strategy.schedule {
                step.operators(&mut ops);
            }
            for op in ops {
                assert!(
                    registry.spec(&op.as_str().into()).is_some(),
                    "{}: strategy `{}` names unknown operator {op}",
                    path.display(),
                    strategy.name
                );
            }
        }
    }
    assert!(
        seen >= 4,
        "expected committed manifests under experiments/manifests"
    );
}
