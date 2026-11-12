//! Legacy execution rejects before state creation or worker initialization.
use std::{fs, process::Command};
#[test]
fn all_legacy_mutations_reject_before_opening_state_or_worker() {
    let root = std::env::temp_dir().join(format!("metron-retired-entry-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    for args in [
        vec!["run", "--task-family", "sum-v1", "--run-dir"],
        vec!["resume"],
        vec!["approval"],
        vec!["reconcile"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_metron-runtime"))
            .args(args)
            .arg(&root)
            .args([
                "--worker",
                "/must-not-spawn",
                "--model-profile",
                "/must-not-read",
            ])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
        let report: metron_protocol::CliReport =
            metron_protocol::parse_internal(&out.stdout).unwrap();
        assert_eq!(
            report.diagnostic.unwrap().as_str(),
            "legacy execution removed; use coding"
        );
        assert!(!root.exists());
    }
}
#[test]
fn existing_legacy_journal_cannot_be_mutated_through_either_entry() {
    use metron_protocol::*;
    let root = std::env::temp_dir().join(format!("metron-retired-existing-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let task = Task::new(TaskData {
        schema_version: SchemaVersion::V1,
        task_id: TaskId::new("historical").unwrap(),
        principal_id: PrincipalId::new("operator").unwrap(),
        objective: BoundedText::new("Frozen historical schema fixture").unwrap(),
        budget: BudgetRequest {
            max_model_requests: SqlU64::new(1).unwrap(),
            max_tool_calls: SqlU64::new(2).unwrap(),
            max_spend_micros_usd: SqlU64::new(0).unwrap(),
            max_elapsed_ms: None,
        },
        max_decision_steps: 8,
    })
    .unwrap();
    let genesis = Genesis {
        coding_policy: None,
        mode: EngineMode::Agent,
        engine_version: BoundedText::new("sum-agent/v1").unwrap(),
        require_publication_approval: false,
        policy_digest: content_digest(b"frozen-reader-test"),
        local_profile: None,
    };
    let mut store = metron_store_sqlite::SqliteStore::create(root.join("runtime.sqlite")).unwrap();
    store
        .apply(
            StoreCommand::new(CommandData {
                schema_version: SchemaVersion::V1,
                command_id: CommandId::new("fixture").unwrap(),
                operation: StoreOperation::CreateTask { task, genesis },
            })
            .unwrap(),
        )
        .unwrap();
    drop(store);
    let before = fs::read(root.join("runtime.sqlite")).unwrap();
    for command in ["resume", "approval", "reconcile"] {
        let out = Command::new(env!("CARGO_BIN_EXE_metron-runtime"))
            .arg(command)
            .arg(&root)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
        assert_eq!(fs::read(root.join("runtime.sqlite")).unwrap(), before);
    }
    let out = Command::new(env!("CARGO_BIN_EXE_metron-runtime"))
        .args(["coding", "resume", "--run-dir"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("legacy execution removed"));
    assert_eq!(fs::read(root.join("runtime.sqlite")).unwrap(), before);
    assert!(!root.join("coding-settings.json").exists());
    fs::remove_dir_all(root).unwrap();
}
