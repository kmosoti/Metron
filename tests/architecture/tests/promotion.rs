//! Milestone M4: candidates are judged, not trusted.
//!
//! Candidates are proposed by the system from answered train-split
//! episodes; the laboratory's gate judges them on the test split, under
//! criteria the system cannot read, and the decision carries the gate's
//! fingerprint. A composition that only works on some targets is rejected;
//! one that works everywhere is promoted.

use metron_cli::{PromoteOptions, run_promote};
use metron_core::capability::PromotionVerdict;
use metron_lab::PromotionGate;
use std::fs;

#[test]
fn candidates_are_proposed_from_train_and_judged_on_held_out_targets() {
    let dir = std::env::temp_dir().join(format!("metron-promotion-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let manifest_path = dir.join("promotion.json");
    fs::write(
        &manifest_path,
        r#"{
  "name": "promotion-test", "seed": 21,
  "lab": {"family": "hidden-boolean-function",
          "tasks": {"arity": 5, "pool": {"families": ["affine", "monotone", "k-term-dnf"], "per_family": 14}, "targets_per_family": 4},
          "max_probes": 32},
  "system": {"strategies": [
      {"name": "greedy", "schedule": [{"operator": "version-space-filter"}, {"sequence": [{"operator": "greedy-split-probe"}, {"operator": "version-space-filter"}], "repeat": 32}, {"operator": "single-survivor-to-table"}, {"operator": "commit-truth-table"}]},
      {"name": "affine-blind", "schedule": [{"operator": "affine-probe"}, {"operator": "affine-solve"}, {"operator": "commit-truth-table"}]}
  ], "budget": {"max_oracle_probes": 32}, "max_steps": 128}
}"#,
    )
    .unwrap();
    let outcome = run_promote(
        &manifest_path,
        &PromoteOptions {
            results_root: dir.join("results"),
            strategy: None,
            rounds: None,
        },
    )
    .unwrap();
    assert_eq!(outcome.reports.len(), 2);
    let gate = PromotionGate::v0().fingerprint();
    for r in &outcome.reports {
        assert_eq!(
            r.gate_fingerprint, gate,
            "decisions carry the gate's fingerprint"
        );
    }
    let greedy = &outcome.reports[0];
    assert_eq!(greedy.strategy, "greedy");
    let top = &greedy.cases[0];
    assert!(top.train_support >= 2, "{top:?}");
    assert!(
        top.reuse_frequency > 0.5,
        "the composition recurs on held-out tasks: {}",
        top.reuse_frequency
    );
    assert_eq!(top.held_out.solved, 1.0);
    assert_eq!(top.decision, PromotionVerdict::Promoted);
    assert!(
        top.candidate
            .claimed_contract
            .as_ref()
            .unwrap()
            .assumes
            .iter()
            .any(|a| a.contains("pool"))
    );
    assert!(
        !top.candidate.program.is_null(),
        "the candidate carries an executable program"
    );

    let blind = &outcome.reports[1];
    assert_eq!(blind.strategy, "affine-blind");
    let case = &blind.cases[0];
    assert!(
        case.held_out.solved < 1.0,
        "the blind shortcut fails off-family: {case:?}"
    );
    let PromotionVerdict::Rejected { reasons } = &case.decision else {
        panic!("a composition that fails held-out targets must be rejected");
    };
    assert!(reasons.iter().any(|r| r.contains("held-out")));
    assert!(outcome.dir.join("promotion.md").exists());
    assert!(outcome.dir.join("promotion.json").exists());
    assert!(
        !outcome.markdown.contains("min_held_out"),
        "criteria values are not reported"
    );
    let _ = fs::remove_dir_all(&dir);
}
