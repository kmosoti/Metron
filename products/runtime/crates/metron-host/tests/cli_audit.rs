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
    metron_protocol::parse_internal(&output.stdout).unwrap()
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
        "--task-family",
        "sum-v1",
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
    let (path, intent_id) = {
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
        (
            run_dir.join(intent.relative_path.as_str()),
            intent.intent_id,
        )
    };
    fs::write(&path, b"corrupt").unwrap();
    let output = run(&["verify", run_dir.to_str().unwrap()]);
    assert!(!output.status.success());
    let report = decode(&output);
    assert_eq!(report.command, CliCommand::Verify);
    assert_eq!(report.audit.unwrap().artifact_valid, Some(false));
    assert_eq!(fs::read(&db_path).unwrap(), before);
    let parent = path.parent().unwrap();
    fs::remove_dir_all(parent).unwrap();
    fs::write(parent, b"not a directory").unwrap();
    let output = run(&[
        "reconcile",
        run_dir.to_str().unwrap(),
        "--intent",
        intent_id.as_str(),
    ]);
    assert_eq!(output.status.code(), Some(6));
    let report = decode(&output);
    assert_eq!(
        report.outcome,
        metron_protocol::CliOutcome::OperationalFailure
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim(),
        "orchestration unavailable"
    );
    assert_eq!(fs::read(&db_path).unwrap(), before);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn over_limit_audit_is_operational_and_leaves_database_intact() {
    use metron_protocol::{
        store::{CommandData, EngineMode, Genesis, StoreCommand, StoreOperation, WriteOutcome},
        types::{BoundedText, CommandId, SchemaVersion, SqlU64, content_digest},
    };
    let root = std::env::temp_dir().join(format!("metron-audit-limit-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let path = root.join("runtime.sqlite");
    let mut store = SqliteStore::create(&path).unwrap();
    let task = metron_testkit::sum_task(
        TaskId::new("limit").unwrap(),
        PrincipalId::new("principal").unwrap(),
    )
    .unwrap();
    let genesis = Genesis {
        coding_policy: None,
        mode: EngineMode::Agent,
        engine_version: BoundedText::new("sum-agent/v1").unwrap(),
        require_publication_approval: false,
        policy_digest: content_digest(b"metron-alpha0-host-policy-v1"),
        local_profile: None,
    };
    let command = |id: String, operation| {
        StoreCommand::new(CommandData {
            schema_version: SchemaVersion::V1,
            command_id: CommandId::new(id).unwrap(),
            operation,
        })
        .unwrap()
    };
    assert!(matches!(
        store
            .apply(command(
                "create".into(),
                StoreOperation::CreateTask { task, genesis }
            ))
            .unwrap(),
        WriteOutcome::Committed(_)
    ));
    for i in 0..metron_protocol::MAX_AUDIT_RECORDS {
        let operation = StoreOperation::AdvanceTick {
            expected_tick: SqlU64::new(i as u64).unwrap(),
            new_tick: SqlU64::new(i as u64 + 1).unwrap(),
        };
        assert!(matches!(
            store
                .apply(command(format!("tick-{i}"), operation))
                .unwrap(),
            WriteOutcome::Committed(_)
        ));
    }
    drop(store);
    let before = fs::read(&path).unwrap();
    let output = run(&["replay", root.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(6));
    let report: CliReport = metron_protocol::parse_internal(&output.stdout).unwrap();
    assert_eq!(
        report.outcome,
        metron_protocol::CliOutcome::OperationalFailure
    );
    assert_eq!(
        report.diagnostic.unwrap().as_str(),
        "audit resource limit exceeded"
    );
    assert!(report.audit.is_none());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim(),
        "audit resource limit exceeded"
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn corrupted_dedup_cache_exits_as_integrity_failure() {
    let root = std::env::temp_dir().join(format!("metron-cache-corrupt-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let task = metron_testkit::sum_task(
        TaskId::new("cache").unwrap(),
        PrincipalId::new("principal").unwrap(),
    )
    .unwrap();
    let file = root.join("task.json");
    fs::write(&file, canonical_bytes(&task, MAX_ENVELOPE).unwrap()).unwrap();
    let dir = root.join("run");
    let output = run(&[
        "run",
        "--task-family",
        "sum-v1",
        "--task",
        file.to_str().unwrap(),
        "--run-dir",
        dir.to_str().unwrap(),
    ]);
    assert!(output.status.success());
    let db = dir.join("runtime.sqlite");
    let connection = rusqlite::Connection::open(&db).unwrap();
    assert!(
        connection
            .execute(
                "UPDATE commands SET result = ?1 WHERE rowid = (SELECT min(rowid) FROM commands)",
                [b"{}".as_slice()]
            )
            .unwrap()
            == 1
    );
    drop(connection);
    let output = run(&["replay", dir.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(5));
    let report: CliReport = metron_protocol::parse_internal(&output.stdout).unwrap();
    assert_eq!(
        report.outcome,
        metron_protocol::CliOutcome::IntegrityFailure
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim(),
        "invalid audit data"
    );
    assert!(report.audit.is_none());
    fs::remove_dir_all(root).unwrap();
}
