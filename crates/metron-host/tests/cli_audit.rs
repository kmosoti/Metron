//! Current coding audit uses coherent read-only snapshots and bounded failures.
use metron_agent::CodingAgent;
use metron_harness::orchestrator::{self, NoObserver};
use metron_protocol::*;
use metron_store_sqlite::SqliteStore;
use metron_testkit::coding::{ModelReplies, command, fixture};
use std::{fs, process::Command};
fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_metron-runtime"))
        .args(args)
        .output()
        .unwrap()
}
fn setup(name: &str) -> (std::path::PathBuf, SqliteStore, TaskId, CodingAgent) {
    let root =
        std::env::temp_dir().join(format!("metron-coding-audit-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let (task, policy, genesis) = fixture(name, EngineMode::Agent);
    let id = task.data().task_id.clone();
    let engine = CodingAgent::new(&task, &policy).unwrap();
    let mut store = SqliteStore::create(root.join("runtime.sqlite")).unwrap();
    store
        .apply(command(StoreOperation::CreateTask {
            task: Task::new_coding(task).unwrap(),
            genesis,
        }))
        .unwrap();
    (root, store, id, engine)
}
#[test]
fn replay_and_verify_are_read_only_and_do_not_self_certify_coding() {
    let (root, mut store, id, engine) = setup("read-only");
    orchestrator::drive_with_driver(
        &mut store,
        &id,
        &engine,
        &mut NoObserver,
        &mut ModelReplies::default(),
    )
    .unwrap();
    drop(store);
    let before = fs::read(root.join("runtime.sqlite")).unwrap();
    for cmd in ["replay", "verify"] {
        let output = run(&[cmd, root.to_str().unwrap()]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: CliReport = parse_internal(&output.stdout).unwrap();
        let audit = report.audit.unwrap();
        assert_eq!(
            (
                audit.chain_valid,
                audit.accounting_valid,
                audit.projection_valid,
                audit.behavior_valid
            ),
            (Some(true), Some(true), Some(true), Some(true))
        );
        assert!(!report.health.unwrap().verified_success);
        assert_eq!(fs::read(root.join("runtime.sqlite")).unwrap(), before);
    }
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn over_limit_audit_is_operational_and_leaves_database_intact() {
    let (root, mut store, _, _) = setup("limit");
    for i in 0..MAX_AUDIT_RECORDS {
        assert!(matches!(
            store
                .apply(command(StoreOperation::AdvanceTick {
                    expected_tick: SqlU64::new(i as u64).unwrap(),
                    new_tick: SqlU64::new(i as u64 + 1).unwrap()
                }))
                .unwrap(),
            WriteOutcome::Committed(_)
        ));
    }
    drop(store);
    let before = fs::read(root.join("runtime.sqlite")).unwrap();
    let output = run(&["replay", root.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(6));
    let report: CliReport = parse_internal(&output.stdout).unwrap();
    assert_eq!(report.outcome, CliOutcome::OperationalFailure);
    assert_eq!(
        report.diagnostic.unwrap().as_str(),
        "audit resource limit exceeded"
    );
    assert!(report.audit.is_none());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim(),
        "audit resource limit exceeded"
    );
    assert_eq!(fs::read(root.join("runtime.sqlite")).unwrap(), before);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn corrupted_dedup_cache_exits_as_integrity_failure() {
    let (root, store, _, _) = setup("corrupt");
    drop(store);
    let db = root.join("runtime.sqlite");
    let connection = rusqlite::Connection::open(&db).unwrap();
    assert_eq!(
        connection
            .execute(
                "UPDATE commands SET result = ?1 WHERE rowid = (SELECT min(rowid) FROM commands)",
                [b"{}".as_slice()]
            )
            .unwrap(),
        1
    );
    drop(connection);
    let before = fs::read(&db).unwrap();
    let output = run(&["replay", root.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(5));
    let report: CliReport = parse_internal(&output.stdout).unwrap();
    assert_eq!(report.outcome, CliOutcome::IntegrityFailure);
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim(),
        "invalid audit data"
    );
    assert!(report.audit.is_none());
    assert_eq!(fs::read(&db).unwrap(), before);
    fs::remove_dir_all(root).unwrap();
}
