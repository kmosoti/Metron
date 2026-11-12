//! Real child-process crashes around current coding admission and artifact freeze.
use super::tests::{assert_audits, driver, setup};
use super::*;
use metron_harness::orchestrator::{Checkpoint, Observer};
use std::{
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
const REPLIES: [&str; 3] = [
    r#"{"tool":"plan","steps":["write controlled note"]}"#,
    r#"{"tool":"edit","path":"note.txt","expected_digest":null,"content":"controlled note"}"#,
    r#"{"tool":"finish","summary":"request independent check"}"#,
];
struct Reject;
impl CodingVerifier for Reject {
    fn verify(
        &mut self,
        p: &Projection,
    ) -> Result<(CodingVerificationSelection, bool), DriveError> {
        let ReceiptOutput::Coding { observation } =
            &p.verification.publication.as_ref().unwrap().data().output
        else {
            panic!("coding artifact")
        };
        let CodingObservation::Artifact { artifact } = observation.as_ref() else {
            panic!("artifact")
        };
        let context = p.state.coding_context.as_ref().unwrap();
        Ok((
            CodingVerificationSelection {
                evidence_revision: p.verification.evidence_revision,
                plan_evidence: context.plan_evidence.clone().unwrap(),
                model_evidence: p.verification.model_evidence.clone().unwrap(),
                artifact_evidence: p.verification.publication_evidence.clone().unwrap(),
                artifact_digest: artifact.tree_digest.clone(),
                acceptance_digest: p.task.coding().unwrap().data().acceptance_digest.clone(),
                evaluator_digest: content_digest(b"controlled-crash-verifier"),
                report_digest: content_digest(b"controlled-rejection"),
            },
            false,
        ))
    }
}
struct Port {
    inner: Driver,
    freeze_barrier: Option<metron_testkit::FileBarrier>,
    calls: usize,
}
impl EffectDriver for Port {
    type Outcome = DriverOutcome;
    fn execute(&mut self, effect: AuthorizedEffect) -> DriverOutcome {
        self.calls += 1;
        let freeze = matches!(&effect.request().request, ApprovedRequest::Coding { step, .. } if matches!(step.as_ref(), CodingStep::Freeze { .. }));
        let out = self.inner.execute(effect);
        if freeze
            && matches!(out, DriverOutcome::Receipt(_))
            && let Some(b) = &mut self.freeze_barrier
        {
            b.checkpoint(Checkpoint::EffectReturned);
        }
        out
    }
}
fn port(root: &Path, s: &Settings, observed: usize, freeze: bool) -> Port {
    Port {
        inner: driver(root, s, &REPLIES[observed.min(REPLIES.len())..]),
        freeze_barrier: freeze
            .then(|| metron_testkit::FileBarrier::new(root.join("barrier"), "effect".into())),
        calls: 0,
    }
}
#[test]
fn crash_child() {
    let Some(root) = std::env::var_os("METRON_CODING_CRASH_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let target = std::env::var("METRON_CODING_CRASH_POINT").unwrap();
    let settings: Settings = read(&root.join("crash-settings.json")).unwrap();
    let mut store = SqliteStore::open_existing(root.join("run/runtime.sqlite")).unwrap();
    let task = Task::new_coding(settings.task.clone()).unwrap();
    let engine = crate::cognition(&settings.genesis().unwrap(), &task).unwrap();
    let mut worker = port(&root, &settings, 0, target == "freeze");
    let mut barrier = metron_testkit::FileBarrier::new(
        root.join("barrier"),
        if target == "freeze" {
            "unused".into()
        } else {
            target
        },
    );
    orchestrator::drive_coding_with_driver(
        &mut store,
        &settings.task.data().task_id,
        engine.as_ref(),
        &mut barrier,
        &mut worker,
        &mut Reject,
    )
    .unwrap();
    panic!("checkpoint not reached");
}
#[test]
fn production_coding_checkpoints_survive_process_kill() {
    for mode in [EngineMode::Agent, EngineMode::Harness] {
        for target in [
            "outbox",
            "admission",
            "effect",
            "freeze",
            "receipt",
            "verification",
        ] {
            let name = format!("crash-{mode:?}-{target}");
            let (root, s, store) = setup(
                &name,
                mode,
                vec![CodingTool::Edit, CodingTool::Finish],
                false,
            );
            drop(store);
            fs::write(
                root.join("crash-settings.json"),
                canonical_bytes(&s, MAX_INTERNAL).unwrap(),
            )
            .unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "coding_run::crash_tests::crash_child",
                    "--nocapture",
                ])
                .env("METRON_CODING_CRASH_ROOT", &root)
                .env("METRON_CODING_CRASH_POINT", target)
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(20);
            while !root.join("barrier").exists() {
                if child.try_wait().unwrap().is_some() {
                    let out = child.wait_with_output().unwrap();
                    panic!("{name} exited: {}", String::from_utf8_lossy(&out.stderr));
                }
                assert!(Instant::now() < deadline, "{name} timed out");
                thread::sleep(Duration::from_millis(10));
            }
            child.kill().unwrap();
            child.wait().unwrap();
            let mut store = SqliteStore::open_existing(root.join("run/runtime.sqlite")).unwrap();
            let task = Task::new_coding(s.task.clone()).unwrap();
            let engine = crate::cognition(&s.genesis().unwrap(), &task).unwrap();
            let p = store
                .audit()
                .unwrap()
                .projection(&s.task.data().task_id)
                .unwrap();
            let mut worker = port(
                &root,
                &s,
                p.task_account.posted.model_requests.get() as usize,
                false,
            );
            if matches!(target, "admission" | "effect" | "freeze") {
                store
                    .apply(
                        command(StoreOperation::AdvanceTick {
                            expected_tick: SqlU64::new(0).unwrap(),
                            new_tick: SqlU64::new(30).unwrap(),
                        })
                        .unwrap(),
                    )
                    .unwrap();
                assert_eq!(
                    orchestrator::drive_coding_with_driver(
                        &mut store,
                        &s.task.data().task_id,
                        engine.as_ref(),
                        &mut NoObserver,
                        &mut worker,
                        &mut Reject
                    )
                    .unwrap(),
                    orchestrator::DriveStatus::Blocked
                );
                assert_eq!(
                    worker.calls, 0,
                    "restart must not redispatch the admitted effect"
                );
                let mut snapshot = store.audit().unwrap();
                let p = snapshot.projection(&s.task.data().task_id).unwrap();
                assert!(
                    p.task_account.reserved.model_requests.get()
                        + p.task_account.reserved.tool_calls.get()
                        > 0
                );
                let pending = snapshot
                    .intent(p.state.pending_intent.as_ref().unwrap())
                    .unwrap()
                    .unwrap();
                assert_eq!(pending.status, DispatchStatus::Uncertain);
                assert!(pending.selected_evidence.is_none());
                if target == "freeze" {
                    assert!(
                        fs::read_dir(root.join("run")).unwrap().any(|entry| entry
                            .unwrap()
                            .file_name()
                            .to_string_lossy()
                            .starts_with("source-")),
                        "freeze artifact must survive crash"
                    );
                }
            } else {
                assert_eq!(
                    orchestrator::drive_coding_with_driver(
                        &mut store,
                        &s.task.data().task_id,
                        engine.as_ref(),
                        &mut NoObserver,
                        &mut worker,
                        &mut Reject
                    )
                    .unwrap(),
                    orchestrator::DriveStatus::Rejected
                );
                assert_eq!(
                    store
                        .audit()
                        .unwrap()
                        .projection(&s.task.data().task_id)
                        .unwrap()
                        .state
                        .phase,
                    Phase::Rejected
                );
            }
            assert_audits(&mut store, &s);
            drop(worker);
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
    }
}
