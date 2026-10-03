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

#[test]
fn every_committed_report_names_the_hash_of_its_manifest() {
    // A report's file name carries the short hash of the manifest that
    // produced it. If a code change alters how a manifest serialises, the
    // hash moves and this fails, so old reports cannot silently lose their
    // provenance. A report produced before such a change is listed in
    // `experiments/reports/provenance.json` with the hash its manifest has
    // now and why it moved; that entry must stay current too.
    let root = workspace_root();
    let provenance: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(root.join("experiments/reports/provenance.json"))
            .expect("provenance.json"),
    )
    .expect("provenance.json parses");
    let mut checked = 0;
    let mut moved = Vec::new();
    for entry in fs::read_dir(root.join("experiments/reports")).expect("reports dir") {
        let name = entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .into_owned();
        let Some(stem) = name.strip_suffix(".json") else {
            continue;
        };
        if name == "provenance.json" {
            continue;
        }
        // `<manifest>-<hash>` or `<manifest>-<hash>-<kind>`, the hash being
        // the last twelve-hex-digit segment.
        let parts: Vec<&str> = stem.split('-').collect();
        let Some(at) = parts
            .iter()
            .rposition(|p| p.len() == 12 && p.chars().all(|c| c.is_ascii_hexdigit()))
        else {
            continue;
        };
        let manifest_name = parts[..at].join("-");
        let hash = parts[at];
        let path = root.join(format!("experiments/manifests/{manifest_name}.json"));
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{name}: no manifest at {}: {e}", path.display()));
        let manifest = Manifest::from_json(&text).expect("manifest parses");
        let now = manifest.hash().short();
        let alias = provenance
            .get(&name)
            .and_then(|e| e.get("manifest_hash_now"))
            .and_then(serde_json::Value::as_str);
        match alias {
            _ if now == hash => {}
            Some(recorded) if recorded == now => {}
            _ => moved.push(format!("{name}: manifest now hashes to {now}")),
        }
        checked += 1;
    }
    assert!(
        checked >= 6,
        "expected the committed reports, found {checked}"
    );
    assert!(
        moved.is_empty(),
        "reports whose manifests now hash differently:\n{}",
        moved.join("\n")
    );
}
