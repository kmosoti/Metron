//! Exact approval is a durable decision, never an effect dispatch.
use metron_agent::CodingAgent;
use metron_harness::orchestrator::{self, NoObserver};
use metron_protocol::*;
use metron_store_sqlite::SqliteStore;
use metron_testkit::coding::{ModelReplies, command, fixture};
#[test]
fn approval_is_bound_and_never_dispatches_by_itself() {
    approval(false);
}
#[test]
fn denied_run_reports_json_on_stdout() {
    approval(true);
}
fn approval(deny: bool) {
    for mode in [EngineMode::Agent, EngineMode::Harness] {
        let root = std::env::temp_dir().join(format!(
            "metron-coding-approval-{deny}-{mode:?}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let (task, mut policy, mut genesis) = fixture("approval", mode);
        policy.require_mutation_approval = true;
        genesis.policy_digest = policy.digest(&task).unwrap();
        genesis.coding_policy = Some(Box::new(policy.clone()));
        genesis.require_publication_approval = true;
        let engine: Box<dyn Cognition> = match mode {
            EngineMode::Agent => Box::new(CodingAgent::new(&task, &policy).unwrap()),
            EngineMode::Harness => {
                Box::new(metron_harness::planner::CodingPlanner::new(&task, &policy).unwrap())
            }
        };
        let id = task.data().task_id.clone();
        let mut store = SqliteStore::create(root.join("runtime.sqlite")).unwrap();
        store
            .apply(command(StoreOperation::CreateTask {
                task: Task::new_coding(task).unwrap(),
                genesis,
            }))
            .unwrap();
        let mut driver = ModelReplies::new(&[
            r#"{"tool":"plan","steps":["create note"]}"#,
            r#"{"tool":"edit","path":"note.txt","expected_digest":null,"content":"controlled"}"#,
        ]);
        assert_eq!(
            orchestrator::drive_with_driver(
                &mut store,
                &id,
                engine.as_ref(),
                &mut NoObserver,
                &mut driver
            )
            .unwrap(),
            orchestrator::DriveStatus::Blocked
        );
        assert_eq!(driver.calls, 2, "mutation must wait before driver entry");
        let page = store.audit().unwrap().intents(&id, None).unwrap();
        let pending = page
            .records
            .iter()
            .find_map(|i| i.approval.as_ref())
            .unwrap();
        let wrong = store.apply(command(StoreOperation::RecordApproval {
            request_id: pending.request_id.clone(),
            action_digest: content_digest(b"wrong"),
            decision: ApprovalDecision::Approve,
        }));
        assert!(!matches!(wrong, Ok(WriteOutcome::Committed(_))));
        store
            .apply(command(StoreOperation::RecordApproval {
                request_id: pending.request_id.clone(),
                action_digest: pending.action_digest.clone(),
                decision: if deny {
                    ApprovalDecision::Deny
                } else {
                    ApprovalDecision::Approve
                },
            }))
            .unwrap();
        assert_eq!(driver.calls, 2, "approval itself cannot dispatch");
        if deny {
            assert_eq!(
                orchestrator::drive_with_driver(
                    &mut store,
                    &id,
                    engine.as_ref(),
                    &mut NoObserver,
                    &mut driver
                )
                .unwrap(),
                orchestrator::DriveStatus::Denied
            );
            assert_eq!(driver.calls, 2);
        }
        drop(store);
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_metron-runtime"))
            .arg("replay")
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: CliReport = parse_internal(&output.stdout).unwrap();
        assert!(report.audit.unwrap().behavior_valid.unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }
}
