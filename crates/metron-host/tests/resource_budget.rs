use std::{fs, process::Command};
#[test]
fn calculator_is_read_only_and_never_authorizes_larger_context_execution() {
    let root =
        std::env::temp_dir().join(format!("metron-context-budget-cli-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let path = root.join("request.json");
    let mut request = serde_json::json!({"planner_min":8192,"planner_desired":32768,"planner_output":512,"executor_count":4,"executor_context":8192,"executor_output":512,"usable_tokens":65536});
    fs::write(&path, request.to_string()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_metron-runtime"))
        .args(["model", "budget", "--input"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["planner_context"], 32768);
    assert_eq!(report["executor_context"], 8192);
    assert_eq!(report["executor_count"], 4);
    assert_eq!(report["runtime_supported"], false);
    assert_eq!(report["physical_fit"], "not_evaluated");
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    request["usable_tokens"] = serde_json::json!(40959);
    fs::write(&path, request.to_string()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_metron-runtime"))
        .args(["model", "budget", "--input"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Capacity"));
    request["usable_tokens"] = serde_json::json!(65536);
    request["memory"] = serde_json::json!({"device_budget_bytes":4294967296_u64,"external_reserved_bytes":0,"weights_upper_bytes":1073741824,"recurrent_upper_bytes":1048576,"workspace_upper_bytes":268435456,"runtime_upper_bytes":134217728,"headroom_bytes":134217728,"scalar_bytes":4,"accounting":"disjoint_upper_bounds","provenance":"synthetic calculator inputs, not physical qualification"});
    fs::write(&path, request.to_string()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_metron-runtime"))
        .args(["model", "budget", "--input"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["executor_context"], 8192);
    assert_eq!(report["executor_count"], 4);
    assert!(report["planner_context"].as_u64().unwrap() < 32768);
    assert_eq!(report["physical_fit"], "conditional_on_declared_bounds");
    assert_eq!(report["planning_kv_scalar_bytes"], 4);
    request["memory"]["workspace_upper_bytes"] = serde_json::Value::Null;
    fs::write(&path, request.to_string()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_metron-runtime"))
        .args(["model", "budget", "--input"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Unqualified"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn resume_of_budgeted_run_cannot_omit_external_policy() {
    use metron_protocol::*;
    use metron_store_sqlite::SqliteStore;
    let root =
        std::env::temp_dir().join(format!("metron-sticky-budget-cli-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let (task, policy, genesis) =
        metron_testkit::coding::fixture("sticky-cli", EngineMode::Harness);
    let mut store = SqliteStore::create(root.join("runtime.sqlite")).unwrap();
    store
        .apply(metron_testkit::coding::command(
            StoreOperation::CreateTask {
                task: Task::new_coding(task.clone()).unwrap(),
                genesis,
            },
        ))
        .unwrap();
    drop(store);
    let settings = serde_json::json!({"task":task,"policy":policy,"catalog":{"schema_version":1,"tools":[]},"acceptance":{"version":1,"kind":"calculator-v1"},"profile":null,"worker":null,"artifact":null,"script":["unused"],"memory_db":null,"mode":"harness","context_version":8,"resource_budget_required":true});
    fs::write(root.join("coding-settings.json"), settings.to_string()).unwrap();
    let before = fs::read(root.join("runtime.sqlite")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_metron-runtime"))
        .args(["coding", "resume", "--run-dir"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("requires --resource-budget"),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(root.join("runtime.sqlite")).unwrap(), before);
    assert!(fs::read_dir(&root).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("worker-events")
    }));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn resume_rejects_workspace_budget_policies_before_reading_or_persisting() {
    use metron_protocol::*;
    use metron_store_sqlite::SqliteStore;
    use std::os::unix::fs::symlink;
    for alias in [false, true] {
        let root = std::env::temp_dir().join(format!(
            "metron-workspace-budget-cli-{}-{alias}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        let workspace = root.join("workspace");
        let run = root.join("run");
        fs::create_dir(&workspace).unwrap();
        fs::create_dir(&run).unwrap();
        let (task, mut policy, mut genesis) =
            metron_testkit::coding::fixture("workspace-budget-cli", EngineMode::Harness);
        policy.workspace_root = BoundedText::new(workspace.to_str().unwrap()).unwrap();
        genesis.policy_digest = policy.digest(&task).unwrap();
        genesis.coding_policy = Some(Box::new(policy.clone()));
        let mut store = SqliteStore::create(run.join("runtime.sqlite")).unwrap();
        store
            .apply(metron_testkit::coding::command(
                StoreOperation::CreateTask {
                    task: Task::new_coding(task.clone()).unwrap(),
                    genesis,
                },
            ))
            .unwrap();
        drop(store);
        let settings = serde_json::json!({"task":task,"policy":policy,"catalog":{"schema_version":1,"tools":[]},"acceptance":{"version":1,"kind":"calculator-v1"},"profile":null,"worker":null,"artifact":null,"script":["unused"],"memory_db":null,"mode":"harness","context_version":8,"resource_budget_required":false});
        let settings_path = run.join("coding-settings.json");
        fs::write(&settings_path, settings.to_string()).unwrap();
        let inside = workspace.join("resource-policy.json");
        // Invalid JSON proves the placement rejection precedes accepting/reading contents.
        fs::write(&inside, b"model-authored policy bytes").unwrap();
        let supplied = if alias {
            let supplied = root.join("operator-policy-alias.json");
            symlink(&inside, &supplied).unwrap();
            supplied
        } else {
            inside
        };
        let before_settings = fs::read(&settings_path).unwrap();
        let before_db = fs::read(run.join("runtime.sqlite")).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_metron-runtime"))
            .args(["coding", "resume", "--run-dir"])
            .arg(&run)
            .arg("--resource-budget")
            .arg(&supplied)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains("must resolve outside the mutable workspace"),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read(&settings_path).unwrap(), before_settings);
        assert_eq!(fs::read(run.join("runtime.sqlite")).unwrap(), before_db);
        assert!(fs::read_dir(&run).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("worker-events")
        }));
        fs::remove_dir_all(root).unwrap();
    }
}
