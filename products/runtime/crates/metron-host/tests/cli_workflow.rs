//! Operator approval must not dispatch until a later resume.
use metron_protocol::{
    cli::{CliOutcome, CliReport},
    store::{RuntimeSnapshot, StorePort},
    types::{MAX_ENVELOPE, PrincipalId, TaskId, canonical_bytes},
};
use metron_store_sqlite::SqliteStore;
use std::{fs, process::Command};
fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_metron-runtime"))
        .args(args)
        .output()
        .unwrap()
}
#[test]
fn approval_is_bound_and_never_dispatches_by_itself() {
    let root = std::env::temp_dir().join(format!("metron-approval-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let task = metron_testkit::sum_task(
        TaskId::new("approval").unwrap(),
        PrincipalId::new("principal").unwrap(),
    )
    .unwrap();
    let file = root.join("task.json");
    fs::write(&file, canonical_bytes(&task, MAX_ENVELOPE).unwrap()).unwrap();
    let dir = root.join("run");
    let output = run(&[
        "run",
        "--task",
        file.to_str().unwrap(),
        "--run-dir",
        dir.to_str().unwrap(),
        "--require-publication-approval",
    ]);
    assert_eq!(output.status.code(), Some(4));
    let report: CliReport = metron_protocol::parse_internal(&output.stdout).unwrap();
    let approval = report.approval.unwrap();
    let path = {
        let mut store = SqliteStore::open_read_only(dir.join("runtime.sqlite")).unwrap();
        let mut snapshot = store.audit().unwrap();
        snapshot
            .intents(&TaskId::new("approval").unwrap(), None)
            .unwrap()
            .records
            .into_iter()
            .find_map(|i| {
                i.prepared.and_then(|p| match p.request {
                    metron_protocol::ApprovedRequest::PublishArtifact { plan } => {
                        Some(dir.join(plan.relative_path.as_str()))
                    }
                    _ => None,
                })
            })
            .unwrap()
    };
    assert!(!path.exists());
    let output = run(&[
        "approval",
        dir.to_str().unwrap(),
        "--request-id",
        approval.request_id.as_str(),
        "--action-digest",
        approval.action_digest.as_str(),
        "--decision",
        "approve",
        "--command-id",
        "approve",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: CliReport = metron_protocol::parse_internal(&output.stdout).unwrap();
    assert_eq!(report.outcome, CliOutcome::Succeeded);
    assert!(!path.exists());
    let output = run(&["resume", dir.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(path.exists());
    let report: CliReport = metron_protocol::parse_internal(&output.stdout).unwrap();
    assert!(report.health.unwrap().verified_success);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn denied_run_reports_json_on_stdout() {
    let root = std::env::temp_dir().join(format!("metron-denial-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let task = metron_testkit::sum_task(
        TaskId::new("denial").unwrap(),
        PrincipalId::new("principal").unwrap(),
    )
    .unwrap();
    let mut data = task.data().clone();
    data.budget.max_tool_calls = metron_protocol::SqlU64::new(0).unwrap();
    let task = metron_protocol::Task::new(data).unwrap();
    let file = root.join("task.json");
    fs::write(&file, canonical_bytes(&task, MAX_ENVELOPE).unwrap()).unwrap();
    let dir = root.join("run");
    let output = run(&[
        "run",
        "--task",
        file.to_str().unwrap(),
        "--run-dir",
        dir.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(3));
    let report: CliReport = metron_protocol::parse_internal(&output.stdout).unwrap();
    assert_eq!(report.outcome, CliOutcome::Denied);
    assert!(output.stderr.is_empty(), "no diagnostic was reported");
    fs::remove_dir_all(root).unwrap();
}
