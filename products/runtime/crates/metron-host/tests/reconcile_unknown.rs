//! Existing unknown usage cannot be replaced with an inferred tariff.
use metron_agent::SumAgent;
use metron_harness::orchestrator::{self, Checkpoint, Observer};
use metron_protocol::{
    domain::{ActualUsage, ApprovedRequest, Receipt, ReceiptData, ReceiptOutput},
    store::{
        AuditSnapshot, CommandData, EngineMode, Genesis, RuntimeSnapshot, StoreCommand,
        StoreOperation, StorePort,
    },
    types::{
        BoundedText, CommandId, PrincipalId, ProducerReceiptId, SchemaVersion, TaskId,
        content_digest,
    },
};
use metron_store_sqlite::SqliteStore;
struct Stop;
impl Observer for Stop {
    fn checkpoint(&mut self, point: Checkpoint) {
        if point == Checkpoint::PublicationSynced {
            panic!("stop after sync");
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
#[test]
fn existing_unknown_receipt_cannot_be_reconciled_as_known() {
    let root = std::env::temp_dir().join(format!("metron-unknown-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    let task = TaskId::new("unknown").unwrap();
    let mut store = SqliteStore::create(root.join("runtime.sqlite")).unwrap();
    let genesis = Genesis {
        mode: EngineMode::Agent,
        engine_version: BoundedText::new("sum-agent/v1").unwrap(),
        require_publication_approval: false,
        policy_digest: content_digest(b"metron-alpha0-host-policy-v1"),
        local_profile: None,
    };
    store
        .apply(command(
            "create",
            StoreOperation::CreateTask {
                task: metron_testkit::sum_task(
                    task.clone(),
                    PrincipalId::new("principal").unwrap(),
                )
                .unwrap(),
                genesis,
            },
        ))
        .unwrap();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| orchestrator::drive(
            &mut store, &task, &SumAgent, &root, &mut Stop
        )))
        .is_err()
    );
    let publication = {
        let mut snapshot = store.audit().unwrap();
        snapshot
            .intents(&task, None)
            .unwrap()
            .records
            .into_iter()
            .find(|i| {
                matches!(
                    i.intent.kind,
                    metron_protocol::domain::IntentKind::PublishArtifact { .. }
                )
            })
            .unwrap()
    };
    let prepared = publication.prepared.unwrap();
    let ApprovedRequest::PublishArtifact { plan } = &prepared.request else {
        panic!("expected plan")
    };
    let receipt = Receipt::new(ReceiptData {
        schema_version: SchemaVersion::V1,
        producer_receipt_id: ProducerReceiptId::new("unknown-publisher").unwrap(),
        task_id: task.clone(),
        intent_id: prepared.intent_id.clone(),
        dispatch_id: prepared.action.dispatch_id.clone(),
        action_digest: prepared.action_digest.clone(),
        usage: ActualUsage::Unknown {
            reason: BoundedText::new("provider did not report usage").unwrap(),
            evidence: None,
        },
        output: ReceiptOutput::Published {
            artifact_id: plan.artifact_id.clone(),
            content_digest: plan.content_digest.clone(),
            content_length: plan.content_length,
        },
    })
    .unwrap();
    store
        .apply(command(
            "unknown-receipt",
            StoreOperation::RecordReceipt {
                receipt,
                lease_epoch: publication.lease_epoch,
            },
        ))
        .unwrap();
    assert_eq!(
        orchestrator::reconcile(&mut store, &task, &publication.intent_id, &root).unwrap(),
        orchestrator::DriveStatus::Blocked
    );
    let mut snapshot = store.audit().unwrap();
    let intent = snapshot.intent(&publication.intent_id).unwrap().unwrap();
    let evidence = snapshot.evidence(&publication.intent_id, None).unwrap();
    assert_eq!(evidence.records.len(), 1);
    assert!(matches!(
        evidence.records[0].receipt.data().usage,
        ActualUsage::Unknown { .. }
    ));
    assert!(
        intent
            .reservation
            .is_some_and(|r| r.status == metron_protocol::store::ReservationStatus::Held)
    );
    assert!(snapshot.projection(&task).unwrap().task_account.incomplete);
    drop(snapshot);
    drop(store);
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_metron-runtime"))
        .arg("resume")
        .arg(&root)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
    let report: metron_protocol::CliReport =
        metron_protocol::parse_internal(&output.stdout).unwrap();
    assert!(report.health.unwrap().incomplete);
    assert!(matches!(
        report.accounting.unwrap().reported_publication,
        Some(ActualUsage::Unknown { .. })
    ));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unposted_large_usage_is_reported_exactly() {
    let root = std::env::temp_dir().join(format!("metron-unposted-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    let task = TaskId::new("unposted").unwrap();
    let mut store = SqliteStore::create(root.join("runtime.sqlite")).unwrap();
    let genesis = Genesis {
        mode: EngineMode::Agent,
        engine_version: BoundedText::new("sum-agent/v1").unwrap(),
        require_publication_approval: false,
        policy_digest: content_digest(b"metron-alpha0-host-policy-v1"),
        local_profile: None,
    };
    store
        .apply(command(
            "create",
            StoreOperation::CreateTask {
                task: metron_testkit::sum_task(
                    task.clone(),
                    PrincipalId::new("principal").unwrap(),
                )
                .unwrap(),
                genesis,
            },
        ))
        .unwrap();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| orchestrator::drive(
            &mut store, &task, &SumAgent, &root, &mut Stop
        )))
        .is_err()
    );
    let publication = {
        let mut snapshot = store.audit().unwrap();
        snapshot
            .intents(&task, None)
            .unwrap()
            .records
            .into_iter()
            .find(|i| {
                matches!(
                    i.intent.kind,
                    metron_protocol::domain::IntentKind::PublishArtifact { .. }
                )
            })
            .unwrap()
    };
    let prepared = publication.prepared.unwrap();
    let ApprovedRequest::PublishArtifact { plan } = &prepared.request else {
        panic!("expected plan")
    };
    let receipt = Receipt::new(ReceiptData {
        schema_version: SchemaVersion::V1,
        producer_receipt_id: ProducerReceiptId::new("unposted-publisher").unwrap(),
        task_id: task.clone(),
        intent_id: prepared.intent_id.clone(),
        dispatch_id: prepared.action.dispatch_id.clone(),
        action_digest: prepared.action_digest.clone(),
        usage: ActualUsage::Known {
            usage: metron_protocol::UsageVector {
                model_requests: metron_protocol::DecimalUsage::new("0").unwrap(),
                tool_calls: metron_protocol::DecimalUsage::new("184467440737095516160").unwrap(),
                spend_micros_usd: metron_protocol::DecimalUsage::new("0").unwrap(),
            },
        },
        output: ReceiptOutput::Published {
            artifact_id: plan.artifact_id.clone(),
            content_digest: plan.content_digest.clone(),
            content_length: plan.content_length,
        },
    })
    .unwrap();
    store
        .apply(command(
            "unposted-receipt",
            StoreOperation::RecordReceipt {
                receipt,
                lease_epoch: publication.lease_epoch,
            },
        ))
        .unwrap();
    assert_eq!(
        orchestrator::reconcile(&mut store, &task, &publication.intent_id, &root).unwrap(),
        orchestrator::DriveStatus::Blocked
    );
    let mut snapshot = store.audit().unwrap();
    let intent = snapshot.intent(&publication.intent_id).unwrap().unwrap();
    let evidence = snapshot.evidence(&publication.intent_id, None).unwrap();
    assert_eq!(evidence.records.len(), 1);
    let ActualUsage::Known { usage } = &evidence.records[0].receipt.data().usage else {
        panic!("expected known usage")
    };
    assert_eq!(usage.tool_calls.as_str(), "184467440737095516160");
    assert!(
        intent
            .reservation
            .is_some_and(|r| r.status == metron_protocol::store::ReservationStatus::Held)
    );
    assert!(snapshot.projection(&task).unwrap().task_account.incomplete);
    drop(snapshot);
    drop(store);
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_metron-runtime"))
        .arg("resume")
        .arg(&root)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
    let report: metron_protocol::CliReport =
        metron_protocol::parse_internal(&output.stdout).unwrap();
    assert!(report.health.unwrap().incomplete);
    let Some(ActualUsage::Known { usage }) = report.accounting.unwrap().reported_publication else {
        panic!("missing reported usage")
    };
    assert_eq!(usage.tool_calls.as_str(), "184467440737095516160");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn stale_known_overrun_reconciles_full_actual_and_breaches() {
    use metron_protocol::{
        domain::UsageVector,
        types::{DecimalUsage, SqlU64},
    };
    let root = std::env::temp_dir().join(format!("metron-overrun-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    let task = TaskId::new("overrun").unwrap();
    let mut store = SqliteStore::create(root.join("runtime.sqlite")).unwrap();
    let genesis = Genesis {
        mode: EngineMode::Agent,
        engine_version: BoundedText::new("sum-agent/v1").unwrap(),
        require_publication_approval: false,
        policy_digest: content_digest(b"metron-alpha0-host-policy-v1"),
        local_profile: None,
    };
    store
        .apply(command(
            "create",
            StoreOperation::CreateTask {
                task: metron_testkit::sum_task(
                    task.clone(),
                    PrincipalId::new("principal").unwrap(),
                )
                .unwrap(),
                genesis,
            },
        ))
        .unwrap();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| orchestrator::drive(
            &mut store, &task, &SumAgent, &root, &mut Stop
        )))
        .is_err()
    );
    let publication = {
        let mut snapshot = store.audit().unwrap();
        snapshot
            .intents(&task, None)
            .unwrap()
            .records
            .into_iter()
            .find(|i| {
                matches!(
                    i.intent.kind,
                    metron_protocol::domain::IntentKind::PublishArtifact { .. }
                )
            })
            .unwrap()
    };
    let prepared = publication.prepared.unwrap();
    let ApprovedRequest::PublishArtifact { plan } = &prepared.request else {
        panic!("expected plan")
    };
    let receipt = Receipt::new(ReceiptData {
        schema_version: SchemaVersion::V1,
        producer_receipt_id: ProducerReceiptId::new("stale-publisher").unwrap(),
        task_id: task.clone(),
        intent_id: prepared.intent_id.clone(),
        dispatch_id: prepared.action.dispatch_id.clone(),
        action_digest: prepared.action_digest.clone(),
        usage: ActualUsage::Known {
            usage: UsageVector {
                model_requests: DecimalUsage::new("0").unwrap(),
                tool_calls: DecimalUsage::new("2").unwrap(),
                spend_micros_usd: DecimalUsage::new("0").unwrap(),
            },
        },
        output: ReceiptOutput::Published {
            artifact_id: plan.artifact_id.clone(),
            content_digest: plan.content_digest.clone(),
            content_length: plan.content_length,
        },
    })
    .unwrap();
    store
        .apply(command(
            "tick",
            StoreOperation::AdvanceTick {
                expected_tick: SqlU64::new(0).unwrap(),
                new_tick: SqlU64::new(30).unwrap(),
            },
        ))
        .unwrap();
    store
        .apply(command(
            "stale-receipt",
            StoreOperation::RecordReceipt {
                receipt,
                lease_epoch: publication.lease_epoch,
            },
        ))
        .unwrap();
    assert_eq!(
        orchestrator::reconcile(&mut store, &task, &publication.intent_id, &root).unwrap(),
        orchestrator::DriveStatus::Complete
    );
    let mut snapshot = store.audit().unwrap();
    let p = snapshot.projection(&task).unwrap();
    assert_eq!(p.task_account.posted.tool_calls.get(), 3);
    assert_eq!(p.principal_account.posted.tool_calls.get(), 3);
    assert!(p.task_account.breached && p.principal_account.breached);
    assert_eq!(p.task_account.reserved.tool_calls.get(), 0);
    let head = p.replay_head;
    drop(snapshot);
    assert_eq!(
        orchestrator::reconcile(&mut store, &task, &publication.intent_id, &root).unwrap(),
        orchestrator::DriveStatus::Complete
    );
    let mut snapshot = store.audit().unwrap();
    assert_eq!(snapshot.projection(&task).unwrap().replay_head, head);
    drop(snapshot);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn equivalent_multiple_receipts_are_reported_unresolved() {
    use metron_protocol::{
        domain::UsageVector,
        types::{DecimalUsage, SqlU64},
    };
    let root = std::env::temp_dir().join(format!("metron-multiple-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    let task = TaskId::new("multiple").unwrap();
    let mut store = SqliteStore::create(root.join("runtime.sqlite")).unwrap();
    let genesis = Genesis {
        mode: EngineMode::Agent,
        engine_version: BoundedText::new("sum-agent/v1").unwrap(),
        require_publication_approval: false,
        policy_digest: content_digest(b"metron-alpha0-host-policy-v1"),
        local_profile: None,
    };
    store
        .apply(command(
            "create",
            StoreOperation::CreateTask {
                task: metron_testkit::sum_task(
                    task.clone(),
                    PrincipalId::new("principal").unwrap(),
                )
                .unwrap(),
                genesis,
            },
        ))
        .unwrap();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| orchestrator::drive(
            &mut store, &task, &SumAgent, &root, &mut Stop
        )))
        .is_err()
    );
    let publication = {
        let mut snapshot = store.audit().unwrap();
        snapshot
            .intents(&task, None)
            .unwrap()
            .records
            .into_iter()
            .find(|i| {
                matches!(
                    i.intent.kind,
                    metron_protocol::domain::IntentKind::PublishArtifact { .. }
                )
            })
            .unwrap()
    };
    let prepared = publication.prepared.unwrap();
    let ApprovedRequest::PublishArtifact { plan } = &prepared.request else {
        panic!("expected plan")
    };
    store
        .apply(command(
            "tick",
            StoreOperation::AdvanceTick {
                expected_tick: SqlU64::new(0).unwrap(),
                new_tick: SqlU64::new(30).unwrap(),
            },
        ))
        .unwrap();
    for producer in ["producer-a", "producer-b"] {
        let receipt = Receipt::new(ReceiptData {
            schema_version: SchemaVersion::V1,
            producer_receipt_id: ProducerReceiptId::new(producer).unwrap(),
            task_id: task.clone(),
            intent_id: prepared.intent_id.clone(),
            dispatch_id: prepared.action.dispatch_id.clone(),
            action_digest: prepared.action_digest.clone(),
            usage: ActualUsage::Known {
                usage: UsageVector {
                    model_requests: DecimalUsage::new("0").unwrap(),
                    tool_calls: DecimalUsage::new("1").unwrap(),
                    spend_micros_usd: DecimalUsage::new("0").unwrap(),
                },
            },
            output: ReceiptOutput::Published {
                artifact_id: plan.artifact_id.clone(),
                content_digest: plan.content_digest.clone(),
                content_length: plan.content_length,
            },
        })
        .unwrap();
        store
            .apply(command(
                producer,
                StoreOperation::RecordReceipt {
                    receipt,
                    lease_epoch: publication.lease_epoch,
                },
            ))
            .unwrap();
    }
    assert_eq!(
        orchestrator::reconcile(&mut store, &task, &publication.intent_id, &root).unwrap(),
        orchestrator::DriveStatus::MultipleEvidence
    );
    let mut snapshot = store.audit().unwrap();
    let evidence = snapshot.evidence(&publication.intent_id, None).unwrap();
    assert_eq!(evidence.records.len(), 2);
    assert!(evidence.records.iter().all(|e| !e.conflict));
    let intent = snapshot.intent(&publication.intent_id).unwrap().unwrap();
    assert!(
        intent
            .reservation
            .is_some_and(|r| r.status == metron_protocol::store::ReservationStatus::Held)
    );
    drop(snapshot);
    drop(store);
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_metron-runtime"))
        .arg("reconcile")
        .arg(&root)
        .arg("--intent")
        .arg(publication.intent_id.as_str())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
    let report: metron_protocol::CliReport =
        metron_protocol::parse_internal(&output.stdout).unwrap();
    assert_eq!(
        report.diagnostic.unwrap().as_str(),
        "multiple evidence records remain unresolved"
    );
    std::fs::remove_dir_all(root).unwrap();
}
