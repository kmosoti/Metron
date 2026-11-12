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
        let pending = pending.clone();
        let inspect = cli(&root, "inspect", &[]);
        assert_eq!(inspect["outcome"], "succeeded");
        assert!(
            inspect["intents"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["approval"]["request_id"] == pending.request_id.as_str())
        );
        let head = store.audit().unwrap().projection(&id).unwrap().replay_head;
        for (request, digest, command_id) in [
            (
                pending.request_id.as_str(),
                content_digest(b"wrong"),
                "wrong-binding",
            ),
            (
                "stale-request",
                pending.action_digest.clone(),
                "stale-request",
            ),
        ] {
            let rejected = cli(
                &root,
                "approval",
                &[
                    "--request-id",
                    request,
                    "--action-digest",
                    digest.as_str(),
                    "--decision",
                    "approve",
                    "--command-id",
                    command_id,
                ],
            );
            assert_ne!(rejected["outcome"], "succeeded");
        }
        assert_eq!(
            store.audit().unwrap().projection(&id).unwrap().replay_head,
            head
        );
        let args = [
            "--request-id",
            pending.request_id.as_str(),
            "--action-digest",
            pending.action_digest.as_str(),
            "--decision",
            if deny { "deny" } else { "approve" },
            "--command-id",
            "operator-decision",
        ];
        assert_eq!(cli(&root, "approval", &args)["outcome"], "succeeded");
        let decided_head = store.audit().unwrap().projection(&id).unwrap().replay_head;
        assert_eq!(
            cli(&root, "approval", &args)["outcome"],
            "succeeded",
            "idempotent retry"
        );
        assert_eq!(
            store.audit().unwrap().projection(&id).unwrap().replay_head,
            decided_head
        );
        let after = store.audit().unwrap().intents(&id, None).unwrap();
        assert_eq!(
            after.records.len(),
            page.records.len(),
            "operator cannot produce new work"
        );
        assert_eq!(
            store
                .audit()
                .unwrap()
                .intent(
                    &page
                        .records
                        .iter()
                        .find(|i| i.approval.is_some())
                        .unwrap()
                        .intent_id
                )
                .unwrap()
                .unwrap()
                .dispatch_id,
            None
        );
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

fn cli(root: &std::path::Path, action: &str, args: &[&str]) -> serde_json::Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_metron-runtime"))
        .args(["coding", action, "--run-dir", root.to_str().unwrap()])
        .args(args)
        .output()
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        output.status.success(),
        value["outcome"] == "succeeded",
        "{value}"
    );
    value
}

#[test]
fn explicit_clock_cas_expires_approval_without_dispatch() {
    for mode in [EngineMode::Agent, EngineMode::Harness] {
        let root = std::env::temp_dir().join(format!(
            "metron-coding-expired-{mode:?}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let (task, mut policy, mut genesis) = fixture("expiry", mode);
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
        orchestrator::drive_with_driver(
            &mut store,
            &id,
            engine.as_ref(),
            &mut NoObserver,
            &mut driver,
        )
        .unwrap();
        let pending = store
            .audit()
            .unwrap()
            .intents(&id, None)
            .unwrap()
            .records
            .into_iter()
            .find_map(|i| i.approval)
            .unwrap();
        let expiry = pending.expires_at.get().to_string();
        assert_eq!(
            cli(
                &root,
                "advance-tick",
                &[
                    "--expected-tick",
                    "1",
                    "--new-tick",
                    &expiry,
                    "--command-id",
                    "bad-clock"
                ]
            )["outcome"],
            "blocked"
        );
        assert_eq!(
            cli(
                &root,
                "advance-tick",
                &[
                    "--expected-tick",
                    "0",
                    "--new-tick",
                    &expiry,
                    "--command-id",
                    "expire"
                ]
            )["outcome"],
            "succeeded"
        );
        let head = store.audit().unwrap().projection(&id).unwrap().replay_head;
        assert_eq!(
            cli(
                &root,
                "approval",
                &[
                    "--request-id",
                    pending.request_id.as_str(),
                    "--action-digest",
                    pending.action_digest.as_str(),
                    "--decision",
                    "approve",
                    "--command-id",
                    "expired"
                ]
            )["outcome"],
            "blocked"
        );
        assert_eq!(
            store.audit().unwrap().projection(&id).unwrap().replay_head,
            head
        );
        assert_eq!(driver.calls, 2);
        assert!(!root.join("note.txt").exists());
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
