//! Kill the production orchestrator at durable checkpoints, then recover.
use metron_agent::SumAgent;
use metron_harness::orchestrator::{self, Checkpoint, NoObserver, Observer};
use metron_protocol::{
    store::{
        AuditSnapshot, CommandData, EngineMode, Genesis, RuntimeSnapshot, StoreCommand,
        StoreOperation, StorePort, WriteOutcome,
    },
    types::{BoundedText, CommandId, PrincipalId, SchemaVersion, SqlU64, TaskId, content_digest},
};
use metron_store_sqlite::SqliteStore;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
struct AdmissionCounter(usize);
impl Observer for AdmissionCounter {
    fn checkpoint(&mut self, point: Checkpoint) {
        if point == Checkpoint::AdmissionCommitted {
            self.0 += 1;
        }
    }
}

fn command(id: &str, operation: StoreOperation) -> StoreCommand {
    StoreCommand::new(CommandData {
        schema_version: SchemaVersion::V1,
        command_id: CommandId::new(id).unwrap(),
        operation,
    })
    .unwrap()
}
fn task_id() -> TaskId {
    TaskId::new("crash-task").unwrap()
}
fn setup(root: &Path) {
    fs::create_dir_all(root).unwrap();
    let mut store = SqliteStore::create(root.join("runtime.sqlite")).unwrap();
    let task = metron_testkit::sum_task(task_id(), PrincipalId::new("principal").unwrap()).unwrap();
    let genesis = Genesis {
        mode: EngineMode::Agent,
        engine_version: BoundedText::new("sum-agent/v1").unwrap(),
        require_publication_approval: false,
        policy_digest: content_digest(b"metron-alpha0-host-policy-v1"),
        local_profile: None,
    };
    assert!(matches!(
        store
            .apply(command(
                "create",
                StoreOperation::CreateTask { task, genesis }
            ))
            .unwrap(),
        WriteOutcome::Committed(_)
    ));
}
fn name(point: Checkpoint) -> &'static str {
    match point {
        Checkpoint::OutboxCommitted => "outbox",
        Checkpoint::AdmissionCommitted => "admission",
        Checkpoint::PublicationSynced => "publication",
        Checkpoint::ReceiptCommitted => "receipt",
        Checkpoint::VerificationCommitted => "verification",
    }
}
#[test]
fn crash_child() {
    let Some(root) = std::env::var_os("METRON_CRASH_TEST_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let target = std::env::var("METRON_CRASH_TEST_POINT").unwrap();
    let mut store = SqliteStore::open_existing(root.join("runtime.sqlite")).unwrap();
    let _ = orchestrator::drive(
        &mut store,
        &task_id(),
        &SumAgent,
        &root,
        &mut metron_testkit::FileBarrier::new(root.join("barrier"), target),
    )
    .unwrap();
    panic!("target checkpoint was not reached");
}
#[test]
fn production_checkpoints_survive_process_kill() {
    let base = std::env::temp_dir().join(format!("metron-crashes-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    for point in [
        Checkpoint::OutboxCommitted,
        Checkpoint::AdmissionCommitted,
        Checkpoint::PublicationSynced,
        Checkpoint::ReceiptCommitted,
        Checkpoint::VerificationCommitted,
    ] {
        let root = base.join(name(point));
        setup(&root);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "crash_child", "--nocapture"])
            .env("METRON_CRASH_TEST_ROOT", &root)
            .env("METRON_CRASH_TEST_POINT", name(point))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !root.join("barrier").exists() {
            assert!(
                child.try_wait().unwrap().is_none(),
                "child exited before {}",
                name(point)
            );
            assert!(
                Instant::now() < deadline,
                "checkpoint {} timed out",
                name(point)
            );
            thread::sleep(Duration::from_millis(10));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        let mut store = SqliteStore::open_existing(root.join("runtime.sqlite")).unwrap();
        if point == Checkpoint::AdmissionCommitted {
            assert!(matches!(
                store
                    .apply(command(
                        "tick",
                        StoreOperation::AdvanceTick {
                            expected_tick: SqlU64::new(0).unwrap(),
                            new_tick: SqlU64::new(30).unwrap()
                        }
                    ))
                    .unwrap(),
                WriteOutcome::Committed(_)
            ));
            let mut observer = AdmissionCounter::default();
            assert_eq!(
                orchestrator::drive(&mut store, &task_id(), &SumAgent, &root, &mut observer)
                    .unwrap(),
                orchestrator::DriveStatus::Blocked
            );
            assert_eq!(observer.0, 0, "restart admitted a second call");
            let mut snapshot = store.audit().unwrap();
            let page = snapshot.intents(&task_id(), None).unwrap();
            assert_eq!(page.records.len(), 1);
            assert_eq!(
                page.records[0].status,
                metron_protocol::store::DispatchStatus::Uncertain
            );
            assert!(page.records[0].reservation.is_some());
            assert!(page.records[0].selected_evidence.is_none());
            assert!(
                snapshot
                    .evidence(&page.records[0].intent_id, None)
                    .unwrap()
                    .records
                    .is_empty()
            );
            continue;
        }
        if point == Checkpoint::PublicationSynced {
            drop(store);
            let output = Command::new(env!("CARGO_BIN_EXE_metron-runtime"))
                .arg("resume")
                .arg(&root)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(4));
            let report: metron_protocol::CliReport =
                metron_protocol::parse_envelope(&output.stdout).unwrap();
            let intent = report
                .pending_intent
                .expect("blocked report must identify the intent");
            let output = Command::new(env!("CARGO_BIN_EXE_metron-runtime"))
                .arg("reconcile")
                .arg(&root)
                .arg("--intent")
                .arg(intent.as_str())
                .args([
                    "--advance-tick",
                    "30",
                    "--expected-tick",
                    "0",
                    "--command-id",
                    "tick-cli",
                ])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "reconcile failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let report: metron_protocol::CliReport =
                metron_protocol::parse_envelope(&output.stdout).unwrap();
            assert_eq!(report.outcome, metron_protocol::CliOutcome::Succeeded);
            assert!(
                !report.health.unwrap().verified_success,
                "reconciliation must not drive cognition"
            );
            store = SqliteStore::open_existing(root.join("runtime.sqlite")).unwrap();
        }
        assert_eq!(
            orchestrator::drive(&mut store, &task_id(), &SumAgent, &root, &mut NoObserver).unwrap(),
            orchestrator::DriveStatus::Complete,
            "{} did not recover",
            name(point)
        );
        let mut snapshot = store.audit().unwrap();
        assert_eq!(
            snapshot.projection(&task_id()).unwrap().state.phase,
            metron_protocol::domain::Phase::Completed
        );
    }
    fs::remove_dir_all(base).unwrap();
}
