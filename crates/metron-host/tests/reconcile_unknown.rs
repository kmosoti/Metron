//! Coding evidence reconciliation preserves reported usage and one-shot admission.
use metron_agent::CodingAgent;
use metron_harness::orchestrator::{self, NoObserver};
use metron_protocol::*;
use metron_store_sqlite::SqliteStore;
use metron_testkit::coding::{ModelReplies, command, fixture, receipt};

fn setup(
    name: &str,
) -> (
    std::path::PathBuf,
    SqliteStore,
    TaskId,
    CodingAgent,
    IntentSnapshot,
    ReceiptOutput,
) {
    let root = std::env::temp_dir().join(format!("metron-coding-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
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
    let mut driver = ModelReplies::default();
    assert_eq!(
        orchestrator::drive_with_driver(&mut store, &id, &engine, &mut NoObserver, &mut driver)
            .unwrap(),
        orchestrator::DriveStatus::Blocked
    );
    assert_eq!(driver.calls, 1);
    store
        .apply(command(StoreOperation::AdvanceTick {
            expected_tick: SqlU64::new(0).unwrap(),
            new_tick: SqlU64::new(30).unwrap(),
        }))
        .unwrap();
    let intent = store
        .audit()
        .unwrap()
        .intents(&id, None)
        .unwrap()
        .records
        .remove(0);
    let ApprovedRequest::Coding { step, .. } = &intent.prepared.as_ref().unwrap().request else {
        panic!("coding request")
    };
    let coding_runtime::CodingStep::Model { input_digest, .. } = step.as_ref() else {
        panic!("model request")
    };
    let output = ReceiptOutput::Coding {
        observation: Box::new(coding_runtime::CodingObservation::ScriptedModel {
            input_digest: input_digest.clone(),
            content: BoundedText::new(r#"{"tool":"plan","steps":["inspect"]}"#).unwrap(),
        }),
    };
    (root, store, id, engine, intent, output)
}
fn known(models: &str) -> ActualUsage {
    ActualUsage::Known {
        usage: UsageVector {
            model_requests: DecimalUsage::new(models).unwrap(),
            tool_calls: DecimalUsage::new("0").unwrap(),
            spend_micros_usd: DecimalUsage::new("0").unwrap(),
        },
    }
}
fn record(
    store: &mut SqliteStore,
    intent: &IntentSnapshot,
    output: ReceiptOutput,
    usage: ActualUsage,
    producer: &str,
) -> Digest {
    let value = receipt(intent.prepared.as_ref().unwrap(), output, usage, producer);
    let id = value.evidence_id().unwrap();
    store
        .apply(command(StoreOperation::RecordReceipt {
            receipt: value,
            lease_epoch: intent.lease_epoch,
        }))
        .unwrap();
    id
}
fn settle(
    store: &mut SqliteStore,
    task: &TaskId,
    intent: &IntentSnapshot,
    id: Digest,
) -> serde_json::Value {
    let current = store
        .audit()
        .unwrap()
        .intent(&intent.intent_id)
        .unwrap()
        .unwrap();
    let epoch = current.reconciliation_epoch.get().to_string();
    let revision = current.evidence_revision.get().to_string();
    let root = std::env::temp_dir().join(format!(
        "metron-coding-{}-{}",
        task.as_str(),
        std::process::id()
    ));
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_metron-runtime"))
        .args([
            "coding",
            "reconcile",
            "--run-dir",
            root.to_str().unwrap(),
            "--intent",
            intent.intent_id.as_str(),
            "--evidence",
            id.as_str(),
            "--expected-evidence-revision",
            &revision,
            "--expected-epoch",
            &epoch,
            "--command-id",
            "public-reconcile",
        ])
        .output()
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    value
}
#[test]
fn existing_unknown_receipt_cannot_be_reconciled_as_known() {
    let (root, mut store, task, engine, intent, output) = setup("unknown");
    let original = record(
        &mut store,
        &intent,
        output.clone(),
        ActualUsage::Unknown {
            reason: BoundedText::new("unreported charge").unwrap(),
            evidence: None,
        },
        "unknown",
    );
    let replacement = record(&mut store, &intent, output, known("1"), "replacement");
    let head = store
        .audit()
        .unwrap()
        .projection(&task)
        .unwrap()
        .replay_head;
    assert_ne!(
        settle(&mut store, &task, &intent, replacement)["outcome"],
        "succeeded"
    );
    assert_eq!(
        store
            .audit()
            .unwrap()
            .projection(&task)
            .unwrap()
            .replay_head,
        head
    );
    let mut driver = ModelReplies::default();
    assert_eq!(
        orchestrator::drive_with_driver(&mut store, &task, &engine, &mut NoObserver, &mut driver)
            .unwrap(),
        orchestrator::DriveStatus::Blocked
    );
    assert_eq!(driver.calls, 0);
    let mut snapshot = store.audit().unwrap();
    let row = snapshot.intent(&intent.intent_id).unwrap().unwrap();
    assert_ne!(row.status, DispatchStatus::Settled);
    assert!(row.selected_evidence.is_none());
    assert!(
        snapshot
            .evidence(&intent.intent_id, None)
            .unwrap()
            .records
            .iter()
            .any(|r| r.evidence_id == original)
    );
    let projection = snapshot.projection(&task).unwrap();
    assert!(projection.task_account.incomplete || projection.task_account.integrity_hold);
    assert_eq!(projection.task_account.reserved.model_requests.get(), 1);
    drop(snapshot);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn oversized_known_coding_usage_rejects_without_changing_hold() {
    let (root, mut store, task, _, intent, output) = setup("unposted");
    let before = store.audit().unwrap().projection(&task).unwrap();
    for charge in ["9", "184467440737095516160"] {
        let value = receipt(
            intent.prepared.as_ref().unwrap(),
            output.clone(),
            known(charge),
            "large-usage",
        );
        assert!(matches!(
            store.apply(command(StoreOperation::RecordReceipt {
                receipt: value,
                lease_epoch: intent.lease_epoch
            })),
            Err(StoreError::Validation(_))
        ));
    }
    let after = store.audit().unwrap().projection(&task).unwrap();
    assert_eq!(after.replay_head, before.replay_head);
    assert_eq!(after.task_account, before.task_account);
    assert_eq!(after.task_account.reserved.model_requests.get(), 1);
    assert!(
        store
            .audit()
            .unwrap()
            .evidence(&intent.intent_id, None)
            .unwrap()
            .records
            .is_empty()
    );
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn stale_known_usage_reconciles_without_redispatch() {
    let (root, mut store, task, _, intent, output) = setup("overrun");
    let id = record(&mut store, &intent, output, known("1"), "stale");
    let head = store
        .audit()
        .unwrap()
        .projection(&task)
        .unwrap()
        .replay_head;
    let stale = std::process::Command::new(env!("CARGO_BIN_EXE_metron-runtime"))
        .args([
            "coding",
            "reconcile",
            "--run-dir",
            root.to_str().unwrap(),
            "--intent",
            intent.intent_id.as_str(),
            "--evidence",
            id.as_str(),
            "--expected-evidence-revision",
            "0",
            "--expected-epoch",
            "0",
            "--command-id",
            "stale-frontier",
        ])
        .output()
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&stale.stdout).unwrap();
    assert_eq!(value["outcome"], "blocked", "{value}");
    assert_eq!(
        store
            .audit()
            .unwrap()
            .projection(&task)
            .unwrap()
            .replay_head,
        head
    );
    assert_eq!(
        settle(&mut store, &task, &intent, id.clone())["outcome"],
        "succeeded"
    );
    let settled_head = store
        .audit()
        .unwrap()
        .projection(&task)
        .unwrap()
        .replay_head;
    // Repeating both durable operator writes must not settle/account twice.
    let retry = std::process::Command::new(env!("CARGO_BIN_EXE_metron-runtime"))
        .args([
            "coding",
            "reconcile",
            "--run-dir",
            root.to_str().unwrap(),
            "--intent",
            intent.intent_id.as_str(),
            "--evidence",
            id.as_str(),
            "--expected-evidence-revision",
            "1",
            "--expected-epoch",
            "0",
            "--command-id",
            "public-reconcile",
        ])
        .output()
        .unwrap();
    assert!(
        retry.status.success(),
        "{}",
        String::from_utf8_lossy(&retry.stdout)
    );
    assert_eq!(
        store
            .audit()
            .unwrap()
            .projection(&task)
            .unwrap()
            .replay_head,
        settled_head
    );
    let mut snapshot = store.audit().unwrap();
    let p = snapshot.projection(&task).unwrap();
    assert_eq!(p.task_account.posted.model_requests.get(), 1);
    assert_eq!(p.principal_account.posted.model_requests.get(), 1);
    assert!(!p.task_account.breached && !p.principal_account.breached);
    assert_eq!(p.task_account.reserved.model_requests.get(), 0);
    let head = p.replay_head;
    drop(snapshot);
    let row = store
        .audit()
        .unwrap()
        .intent(&intent.intent_id)
        .unwrap()
        .unwrap();
    assert_eq!(row.status, DispatchStatus::Settled);
    assert_eq!(
        store
            .audit()
            .unwrap()
            .projection(&task)
            .unwrap()
            .replay_head,
        head
    );
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn equivalent_multiple_receipts_are_reported_unresolved() {
    let (root, mut store, task, _, intent, output) = setup("multiple");
    record(&mut store, &intent, output.clone(), known("1"), "one");
    let id = record(&mut store, &intent, output, known("1"), "two");
    let _ = id; // No automatic selector is allowed to infer a unique receipt.
    let mut snapshot = store.audit().unwrap();
    let row = snapshot.intent(&intent.intent_id).unwrap().unwrap();
    assert!(row.selected_evidence.is_none());
    assert_eq!(
        snapshot
            .evidence(&intent.intent_id, None)
            .unwrap()
            .records
            .len(),
        2
    );
    assert_eq!(
        snapshot
            .projection(&task)
            .unwrap()
            .task_account
            .reserved
            .model_requests
            .get(),
        1
    );
    drop(snapshot);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
