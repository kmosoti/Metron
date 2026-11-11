//! New submissions must select an executable contract before any effect occurs.
use metron_protocol::{CliOutcome, CliReport, parse_internal};
use std::{fs, process::Command};

#[test]
fn calculator_and_unknown_families_never_reach_the_sum_verifier() {
    let root = std::env::temp_dir().join(format!("metron-family-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let task = root.join("calculator.json");
    fs::write(&task, br#"{"schema_version":1,"task_id":"calculator","principal_id":"operator","objective":"Plan and implement a Rust calculator","budget":{"max_model_requests":1,"max_tool_calls":2,"max_spend_micros_usd":0,"max_elapsed_ms":null},"max_decision_steps":8}"#).unwrap();
    for mode in ["agent", "harness"] {
        for family in [None, Some("coding-v1"), Some("unknown")] {
            let dir = root.join(format!("{mode}-{}", family.unwrap_or("missing")));
            let mut command = Command::new(env!("CARGO_BIN_EXE_metron-runtime"));
            command
                .args(["run", "--task"])
                .arg(&task)
                .arg("--run-dir")
                .arg(&dir)
                .args(["--mode", mode]);
            if let Some(family) = family {
                command.args(["--task-family", family]);
            }
            let output = command.output().unwrap();
            assert_eq!(output.status.code(), Some(2));
            let report: CliReport = parse_internal(&output.stdout).unwrap();
            assert_eq!(report.outcome, CliOutcome::InvalidInput);
            assert!(report.health.is_none());
            assert!(!dir.exists(), "unsupported task created run state");
        }
    }
    // Explicit fixture selection remains available and verifiable. It does
    // not claim that the descriptive calculator objective was implemented.
    let dir = root.join("explicit-fixture");
    let output = Command::new(env!("CARGO_BIN_EXE_metron-runtime"))
        .args(["run", "--task-family", "sum-v1", "--task"])
        .arg(&task)
        .arg("--run-dir")
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let before: CliReport = parse_internal(&output.stdout).unwrap();
    assert!(before.health.unwrap().verified_success);
    for action in ["replay", "verify"] {
        let output = Command::new(env!("CARGO_BIN_EXE_metron-runtime"))
            .arg(action)
            .arg(&dir)
            .output()
            .unwrap();
        assert!(output.status.success());
        let report: CliReport = parse_internal(&output.stdout).unwrap();
        assert_eq!(report.replay_head, before.replay_head);
    }
    fs::remove_dir_all(root).unwrap();
}
