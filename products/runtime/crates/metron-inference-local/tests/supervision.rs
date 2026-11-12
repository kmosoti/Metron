#![forbid(unsafe_code)]
use metron_harness::qualification::{QualificationSession, SessionKind, SessionSources};
use metron_inference_local::{
    artifact::sha256_file,
    driver::{LocalInferenceDriver, QualificationOutcome, WorkerEvent},
};
use metron_protocol::{
    BoundedText, Digest, LocalDevice, LocalProfile, SchemaVersion, SqlU64, content_digest,
    gguf_embedded_tokenizer_digest,
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

static NEXT: AtomicU64 = AtomicU64::new(0);
const RECIPE: &[u8] = include_bytes!("fixtures/prompt-recipe-v1.json");
const CORPUS: &[u8] = include_bytes!("fixtures/calibration-v1.json");

fn profile(worker: &std::path::Path, concurrency: u16) -> LocalProfile {
    let artifact_digest =
        Digest::new("3793e1e6e2266919d6e015287221b41100175a98eddaba92e111aed3b05c93c4").unwrap();
    LocalProfile {
        schema_version: SchemaVersion::V2,
        auxiliary_repository: BoundedText::new("LiquidAI/LFM3-1.7B").unwrap(),
        auxiliary_revision: BoundedText::new("1da06bec6a819da10b54c67e5c90972b46d81871").unwrap(),
        artifact_repository: BoundedText::new("LiquidAI/LFM3-1.7B-GGUF").unwrap(),
        artifact_revision: BoundedText::new("cd0c74c575b5c416f3011b5b4c298077f26ca8c2").unwrap(),
        artifact_name: BoundedText::new("LFM3-1.7B-Q4_K_M.gguf").unwrap(),
        artifact_bytes: SqlU64::new(1_099_627_808).unwrap(),
        embedded_tokenizer_digest: gguf_embedded_tokenizer_digest(&artifact_digest).unwrap(),
        artifact_digest,
        template_digest: content_digest(b"template"),
        prompt_recipe_digest: content_digest(RECIPE),
        backend_revision: BoundedText::new("fb4cdc3730dc64b366724e7637a205a8b7747499").unwrap(),
        backend_build_digest: Digest::new(sha256_file(worker).unwrap()).unwrap(),
        cpu_identity: BoundedText::new("test-cpu").unwrap(),
        cpu_features_digest: content_digest(b"test-cpu"),
        device: LocalDevice::Cpu,
        cpu_threads: 2,
        context_tokens: 2048,
        max_input_tokens: 1536,
        max_output_tokens: 512,
        temperature_milli: 100,
        top_k: 50,
        repeat_penalty_milli: 1100,
        max_sequences: concurrency,
        queue_capacity: 8,
        queue_wait_ms: 250,
        deadline_ms: 350,
        chunked_prefill: false,
        scoped_prefix_cache: false,
        cache_target_bytes: None,
        strict_memory_cap_bytes: None,
    }
}
fn fixture(
    mode: &str,
    concurrency: u16,
) -> (
    PathBuf,
    LocalInferenceDriver,
    QualificationSession,
    Arc<AtomicBool>,
) {
    let path = std::env::temp_dir().join(format!(
        "metron-worker-fault-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&path).unwrap();
    let worker = path.join("fake-worker");
    let script = format!(
        r##"#!/usr/bin/python3
import json,struct,sys,os,time
mode={mode:?}
def read():
 h=sys.stdin.buffer.read(4)
 if len(h)!=4: sys.exit(0)
 return json.loads(sys.stdin.buffer.read(struct.unpack('>I',h)[0]))
def send(x):
 b=json.dumps(x,separators=(',',':')).encode(); sys.stdout.buffer.write(struct.pack('>I',len(b))+b); sys.stdout.buffer.flush()
def event(kind,i,**kw): send(dict(kind=kind,dispatch_id=i,**kw))
a=read(); i=a['dispatch_id']
if mode=='wrong': event('started','f'*64)
elif mode=='truncated': sys.stdout.buffer.write(b'\x00\x00'); sys.stdout.buffer.flush()
elif mode=='oversize': sys.stdout.buffer.write(struct.pack('>I',65537)); sys.stdout.buffer.flush()
elif mode=='malformed': sys.stdout.buffer.write(struct.pack('>I',1)+b'!'); sys.stdout.buffer.flush()
elif mode=='kill_before': os._exit(0)
elif mode=='kill_after':
 event('tokenized',i,token_count=1,token_ids_digest='0'*64,rendered_digest='0'*64)
 event('started',i); os._exit(0)
elif mode=='reject': event('rejected',i,diagnostic='before submission')
elif mode in ('reordered','duplicate'):
 b=read(); j=b['dispatch_id']; event('rejected',j,diagnostic='second')
 event('rejected',j if mode=='duplicate' else i,diagnostic='first')
elif mode=='cancel':
 b=read(); cancel=read(); event('cancelled_before_start',cancel['dispatch_id']); event('rejected',b['dispatch_id'],diagnostic='second')
time.sleep(1)
"##
    );
    fs::write(&worker, script).unwrap();
    fs::set_permissions(&worker, fs::Permissions::from_mode(0o700)).unwrap();
    let profile = profile(&worker, concurrency);
    let session = QualificationSession::create(
        &path,
        content_digest(path.to_string_lossy().as_bytes()),
        SessionKind::Cold,
        profile.clone(),
        SessionSources {
            recipe: RECIPE,
            corpus: CORPUS,
            calibration: CORPUS,
            calibration_proof: None,
        },
    )
    .unwrap();
    let spawned = Arc::new(AtomicBool::new(false));
    let marker = Arc::clone(&spawned);
    let sink = Arc::new(move |event| {
        if let WorkerEvent::Spawned { .. } = event {
            marker.store(true, Ordering::Release);
        }
    });
    let driver =
        LocalInferenceDriver::new(worker, path.join("model.gguf"), profile, Some(sink)).unwrap();
    (path, driver, session, spawned)
}
#[test]
fn untrusted_child_faults_never_fabricate_success() {
    for mode in [
        "wrong",
        "truncated",
        "oversize",
        "malformed",
        "kill_before",
        "kill_after",
    ] {
        let (path, mut driver, mut session, spawned) = fixture(mode, 1);
        let permits = session.admit(&[("calibration-sum-02", 0, false)]).unwrap();
        let dispatch_id = permits[0].request().dispatch_id.clone();
        let outcome = driver.execute_qualification_cohort(permits);
        assert!(spawned.load(Ordering::Acquire), "{mode}");
        assert!(
            matches!(outcome.as_slice(), [QualificationOutcome::Uncertain]),
            "{mode}"
        );
        session
            .settle(&[metron_harness::qualification::QualificationResult {
                dispatch_id,
                observation: None,
                rejection: None,
            }])
            .unwrap();
        assert!(session.admit(&[("calibration-sum-02", 0, false)]).is_err());
        assert!(session.admit(&[("calibration-tool-00", 0, false)]).is_err());
        drop(driver);
        drop(session);
        fs::remove_dir_all(path).unwrap();
    }
}
#[test]
fn pre_submission_rejection_and_out_of_order_replies_are_bound() {
    for mode in ["reject", "reordered", "duplicate"] {
        let count = if mode == "reject" { 1 } else { 2 };
        let (path, mut driver, mut session, _) = fixture(mode, count);
        let trials = if count == 1 {
            vec![("calibration-sum-02", 0, false)]
        } else {
            vec![
                ("calibration-sum-02", 0, false),
                ("calibration-tool-00", 0, false),
            ]
        };
        let outcome = driver.execute_qualification_cohort(session.admit(&trials).unwrap());
        match mode {
            "reject" => assert!(matches!(
                outcome.as_slice(),
                [QualificationOutcome::Rejected(_)]
            )),
            "reordered" => assert!(matches!(
                outcome.as_slice(),
                [
                    QualificationOutcome::Rejected(_),
                    QualificationOutcome::Rejected(_)
                ]
            )),
            _ => assert!(matches!(
                outcome.as_slice(),
                [
                    QualificationOutcome::Uncertain,
                    QualificationOutcome::Rejected(_)
                ]
            )),
        }
        drop(driver);
        drop(session);
        fs::remove_dir_all(path).unwrap();
    }
}
#[test]
fn reordered_permits_are_rejected_without_spawning() {
    let (path, mut driver, mut session, spawned) = fixture("reject", 2);
    let mut permits = session
        .admit(&[
            ("calibration-sum-02", 0, false),
            ("calibration-tool-00", 0, false),
        ])
        .unwrap();
    permits.reverse();
    let outcome = driver.execute_qualification_cohort(permits);
    assert!(matches!(
        outcome.as_slice(),
        [
            QualificationOutcome::Rejected(_),
            QualificationOutcome::Rejected(_)
        ]
    ));
    assert!(!spawned.load(Ordering::Acquire));
    drop(driver);
    drop(session);
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn queued_cancellation_does_not_cancel_other_requests() {
    use metron_inference_local::driver::WorkerControl;
    use std::sync::Mutex;
    let (path, driver, mut session, _) = fixture("cancel", 2);
    drop(driver);
    let worker = path.join("fake-worker");
    let state = Arc::new(Mutex::new((None::<WorkerControl>, None::<Digest>)));
    let callback_state = Arc::clone(&state);
    let sink = Arc::new(move |event| {
        let mut state = callback_state.lock().unwrap();
        match event {
            WorkerEvent::Spawned { control, .. } => state.0 = Some(control),
            WorkerEvent::Sent { dispatch_id } => {
                if let Some(first) = state.1.take() {
                    state.0.as_ref().unwrap().cancel(first).unwrap();
                } else {
                    state.1 = Some(dispatch_id);
                }
            }
            _ => {}
        }
    });
    let mut driver = LocalInferenceDriver::new(
        worker.clone(),
        path.join("model.gguf"),
        profile(&worker, 2),
        Some(sink),
    )
    .unwrap();
    let permits = session
        .admit(&[
            ("calibration-sum-02", 0, false),
            ("calibration-tool-00", 0, false),
        ])
        .unwrap();
    let outcome = driver.execute_qualification_cohort(permits);
    assert!(matches!(
        outcome.as_slice(),
        [
            QualificationOutcome::Rejected(_),
            QualificationOutcome::Rejected(_)
        ]
    ));
    drop(driver);
    drop(session);
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn different_profile_is_rejected_before_worker_start() {
    let (path, driver, mut session, _) = fixture("reject", 1);
    drop(driver);
    let worker = path.join("fake-worker");
    let mut changed = profile(&worker, 1);
    changed.cpu_threads = 1;
    let spawned = Arc::new(AtomicBool::new(false));
    let marker = Arc::clone(&spawned);
    let sink = Arc::new(move |event| {
        if let WorkerEvent::Spawned { .. } = event {
            marker.store(true, Ordering::Release);
        }
    });
    let mut driver =
        LocalInferenceDriver::new(worker, path.join("model.gguf"), changed, Some(sink)).unwrap();
    let permit = session.admit(&[("calibration-sum-02", 0, false)]).unwrap();
    assert!(matches!(
        driver.execute_qualification_cohort(permit).as_slice(),
        [QualificationOutcome::Rejected(_)]
    ));
    assert!(!spawned.load(Ordering::Acquire));
    drop(driver);
    drop(session);
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn chunk_prefill_configuration_is_opt_in_and_bounded() {
    use metron_inference_local::chunk_prefill::kv_target;
    use metron_protocol::CHUNKED_PREFILL_MAX_KV_BYTES;
    let mut p = profile(&std::env::current_exe().unwrap(), 2);
    assert_eq!(kv_target(&p), Ok(None));
    p.cache_target_bytes = Some(SqlU64::new(1).unwrap());
    assert!(kv_target(&p).is_err());
    p.chunked_prefill = true;
    assert!(kv_target(&p).is_err());
    p.device = LocalDevice::Cuda {
        uuid: BoundedText::new("GPU-test").unwrap(),
        model: BoundedText::new("test").unwrap(),
        memory_bytes: SqlU64::new(1024 * 1024 * 1024).unwrap(),
        driver: BoundedText::new("test").unwrap(),
    };
    assert_eq!(kv_target(&p), Ok(Some(1)));
    p.cache_target_bytes = Some(SqlU64::new(CHUNKED_PREFILL_MAX_KV_BYTES + 1).unwrap());
    assert!(kv_target(&p).is_err());
    p.cache_target_bytes = Some(SqlU64::new(256).unwrap());
    p.scoped_prefix_cache = true;
    assert!(kv_target(&p).is_err());
}
