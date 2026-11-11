//! Audits use a coherent read-only snapshot and inspect the actual artifact.
use metron_protocol::{
    cli::{CliCommand, CliReport},
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
fn decode(output: &std::process::Output) -> CliReport {
    let bytes = if output.status.success() {
        &output.stdout
    } else {
        &output.stderr
    };
    metron_protocol::parse_internal(bytes).unwrap()
}
#[test]
fn replay_and_verify_are_read_only_and_verify_checks_artifact() {
    let root = std::env::temp_dir().join(format!("metron-cli-audit-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let task = metron_testkit::sum_task(
        TaskId::new("audit").unwrap(),
        PrincipalId::new("principal").unwrap(),
    )
    .unwrap();
    let task_file = root.join("task.json");
    fs::write(&task_file, canonical_bytes(&task, MAX_ENVELOPE).unwrap()).unwrap();
    let run_dir = root.join("run");
    let output = run(&[
        "run",
        "--task",
        task_file.to_str().unwrap(),
        "--run-dir",
        run_dir.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let db_path = run_dir.join("runtime.sqlite");
    let before = fs::read(&db_path).unwrap();
    for cmd in ["replay", "verify"] {
        let output = run(&[cmd, run_dir.to_str().unwrap()]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report = decode(&output);
        let audit = report.audit.unwrap();
        assert_eq!(audit.chain_valid, Some(true));
        assert_eq!(audit.accounting_valid, Some(true));
        assert_eq!(audit.projection_valid, Some(true));
        assert_eq!(audit.behavior_valid, Some(true));
        assert_eq!(
            audit.artifact_valid,
            if cmd == "verify" { Some(true) } else { None }
        );
        assert_eq!(fs::read(&db_path).unwrap(), before);
    }
    let path = {
        let mut store = SqliteStore::open_read_only(&db_path).unwrap();
        let mut snap = store.audit().unwrap();
        let page = snap.intents(&TaskId::new("audit").unwrap(), None).unwrap();
        let intent = page
            .records
            .into_iter()
            .find_map(|i| {
                i.prepared.and_then(|p| match p.request {
                    metron_protocol::ApprovedRequest::PublishArtifact { plan } => Some(plan),
                    _ => None,
                })
            })
            .unwrap();
        run_dir.join(intent.relative_path.as_str())
    };
    fs::write(path, b"corrupt").unwrap();
    let output = run(&["verify", run_dir.to_str().unwrap()]);
    assert!(!output.status.success());
    let report = decode(&output);
    assert_eq!(report.command, CliCommand::Verify);
    assert_eq!(report.audit.unwrap().artifact_valid, Some(false));
    assert_eq!(fs::read(&db_path).unwrap(), before);
    fs::remove_dir_all(root).unwrap();
}
