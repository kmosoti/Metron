#![forbid(unsafe_code)]
use metron_inference_local::qualification::{QualificationSession, SessionKind, SessionSources};
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
    let script_mode = mode.strip_prefix("budget_").unwrap_or(mode);
    let script = format!(
        r##"#!/usr/bin/python3
import json,struct,sys,os,time
mode={script_mode:?}
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
    let profile = if mode.starts_with("budget_") {
        budget_profile(&worker, concurrency)
    } else {
        profile(&worker, concurrency)
    };
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
            .settle(
                &[metron_inference_local::qualification::QualificationResult {
                    dispatch_id,
                    observation: None,
                    rejection: None,
                }],
            )
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
    use metron_inference_local::chunk_prefill::{MAX_KV_TARGET_BYTES, kv_target};
    let mut p = profile(&std::env::current_exe().unwrap(), 2);
    assert_eq!(kv_target(&p), Ok(None));
    p.chunked_prefill = true;
    assert!(p.validate().is_ok());
    assert!(kv_target(&p).is_err());
    p.cache_target_bytes = Some(SqlU64::new(1).unwrap());
    assert!(p.validate().is_ok());
    assert!(kv_target(&p).is_err());
    p.device = LocalDevice::Cuda {
        uuid: BoundedText::new("GPU-test").unwrap(),
        model: BoundedText::new("test").unwrap(),
        memory_bytes: SqlU64::new(1024 * 1024 * 1024).unwrap(),
        driver: BoundedText::new("test").unwrap(),
    };
    assert_eq!(kv_target(&p), Ok(Some(1)));
    p.cache_target_bytes = Some(SqlU64::new(0).unwrap());
    assert!(p.validate().is_ok());
    assert!(kv_target(&p).is_err());
    p.cache_target_bytes = Some(SqlU64::new(MAX_KV_TARGET_BYTES).unwrap());
    assert_eq!(kv_target(&p), Ok(Some(MAX_KV_TARGET_BYTES as usize)));
    p.cache_target_bytes = Some(SqlU64::new(MAX_KV_TARGET_BYTES + 1).unwrap());
    assert!(p.validate().is_ok());
    assert!(kv_target(&p).is_err());
    p.cache_target_bytes = Some(SqlU64::new(256).unwrap());
    p.scoped_prefix_cache = true;
    assert!(p.validate().is_ok());
    assert!(kv_target(&p).is_err());
}

fn budget_profile(worker: &std::path::Path, concurrency: u16) -> LocalProfile {
    let mut p = profile(worker, concurrency);
    p.device = LocalDevice::Cuda {
        uuid: BoundedText::new("GPU-test").unwrap(),
        model: BoundedText::new("test").unwrap(),
        memory_bytes: SqlU64::new(1 << 30).unwrap(),
        driver: BoundedText::new("test").unwrap(),
    };
    p.chunked_prefill = true;
    p.cache_target_bytes = Some(SqlU64::new(128 << 20).unwrap());
    p
}
fn budget_policy(
    worker: &std::path::Path,
    concurrency: u16,
    pool: metron_inference_local::context_budget::DeviceBudget,
) -> metron_inference_local::context_budget::BudgetPolicy {
    use metron_inference_local::context_budget::*;
    let p = budget_profile(worker, concurrency);
    BudgetPolicy {
        pool,
        bounds: ResidentBounds {
            scope: ResourceScope {
                profile_digest: p.digest().unwrap(),
                device_uuid: "GPU-test".into(),
            },
            weights: Some(100),
            recurrent: Some(100),
            workspace: Some(100),
            runtime: Some(100),
            fixed_kv_pool_bytes: Some(128 << 20),
            kv_usable_tokens_lower_bound: Some(2048 * u64::from(concurrency)),
            margin: 100,
            provenance: "synthetic fake-worker bounds only".into(),
            disjoint_upper_bounds: true,
        },
        task_id: metron_protocol::TaskId::new("unused").unwrap(),
        principal_id: metron_protocol::PrincipalId::new("unused").unwrap(),
    }
}
#[test]
fn budget_scope_and_unknown_bounds_deny_before_spawn() {
    use metron_inference_local::context_budget::*;
    for unknown in [false, true] {
        let (path, driver, mut session, spawned) = fixture("budget_reject", 1);
        let pool = DeviceBudget::new("GPU-test".into(), 1 << 30, 0).unwrap();
        let mut policy = budget_policy(&path.join("fake-worker"), 1, pool.clone());
        let permits = session.admit(&[("calibration-sum-02", 0, false)]).unwrap();
        policy.task_id = permits[0].request().task_id.clone();
        if unknown {
            policy.principal_id = permits[0].request().principal_id.clone();
            policy.bounds.weights = None;
        }
        let mut driver = driver.with_resource_budget(policy);
        assert!(matches!(
            driver.execute_qualification_cohort(permits).as_slice(),
            [QualificationOutcome::Rejected(_)]
        ));
        assert!(!spawned.load(Ordering::Acquire));
        assert_eq!(pool.reserved_bytes(), Ok(0));
        drop(driver);
        drop(session);
        fs::remove_dir_all(path).unwrap();
    }
}
#[test]
fn budget_retains_warm_worker_residency_and_rechecks_scope() {
    use metron_inference_local::context_budget::*;
    let (path, driver, mut session, spawned) = fixture("budget_reject", 1);
    let pool = DeviceBudget::new("GPU-test".into(), 1 << 30, 0).unwrap();
    let mut policy = budget_policy(&path.join("fake-worker"), 1, pool.clone());
    let permits = session.admit(&[("calibration-sum-02", 0, false)]).unwrap();
    policy.task_id = permits[0].request().task_id.clone();
    policy.principal_id = permits[0].request().principal_id.clone();
    let first_dispatch = permits[0].request().dispatch_id.clone();
    let mut driver = driver.with_resource_budget(policy.clone());
    assert!(matches!(
        driver.execute_qualification_cohort(permits).as_slice(),
        [QualificationOutcome::Rejected(_)]
    ));
    assert!(spawned.load(Ordering::Acquire));
    assert_eq!(pool.reserved_bytes(), Ok((128 << 20) + 500));
    session
        .settle(
            &[metron_inference_local::qualification::QualificationResult {
                dispatch_id: first_dispatch,
                observation: None,
                rejection: Some(BoundedText::new("before submission").unwrap()),
            }],
        )
        .unwrap();
    // Changing policy on a warm process must not bypass the cohort gate.
    policy.principal_id = metron_protocol::PrincipalId::new("other").unwrap();
    driver = driver.with_resource_budget(policy);
    let permits = session.admit(&[("calibration-tool-00", 0, false)]).unwrap();
    assert!(matches!(
        driver.execute_qualification_cohort(permits).as_slice(),
        [QualificationOutcome::Rejected(_)]
    ));
    assert_eq!(pool.reserved_bytes(), Ok((128 << 20) + 500));
    driver.shutdown();
    assert_eq!(pool.reserved_bytes(), Ok(0));
    drop(driver);
    drop(session);
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn two_drivers_share_one_resident_envelope_atomically() {
    use metron_inference_local::context_budget::*;
    let envelope = (128 << 20) + 500;
    let pool = DeviceBudget::new("GPU-test".into(), envelope, 0).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let mut threads = Vec::new();
    for _ in 0..2 {
        let (path, driver, mut session, spawned) = fixture("budget_reject", 1);
        let permits = session.admit(&[("calibration-sum-02", 0, false)]).unwrap();
        let mut policy = budget_policy(&path.join("fake-worker"), 1, pool.clone());
        policy.task_id = permits[0].request().task_id.clone();
        policy.principal_id = permits[0].request().principal_id.clone();
        let mut driver = driver.with_resource_budget(policy);
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            let result = driver.execute_qualification_cohort(permits);
            assert!(matches!(
                result.as_slice(),
                [QualificationOutcome::Rejected(_)]
            ));
            (path, driver, session, spawned)
        }));
    }
    let workers = threads
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        workers
            .iter()
            .filter(|(_, _, _, spawned)| spawned.load(Ordering::Acquire))
            .count(),
        1
    );
    assert_eq!(pool.reserved_bytes(), Ok(envelope));
    for (path, driver, session, _) in workers {
        drop(driver);
        drop(session);
        fs::remove_dir_all(path).unwrap();
    }
    assert_eq!(pool.reserved_bytes(), Ok(0));
}

#[test]
fn component_upper_bounds_reject_overlap_unknown_overflow_and_too_small_pool() {
    use metron_inference_local::context_budget::*;
    let worker = std::env::current_exe().unwrap();
    let profile = budget_profile(&worker, 1);
    let pool = DeviceBudget::new("GPU-test".into(), 1 << 30, 0).unwrap();
    let mut policy = budget_policy(&worker, 1, pool);
    assert_eq!(
        resident_bytes(&profile, &policy.bounds),
        Ok((128 << 20) + 500)
    );
    policy.bounds.kv_usable_tokens_lower_bound = None;
    assert_eq!(
        resident_bytes(&profile, &policy.bounds),
        Err(BudgetError::Unqualified)
    );
    policy.bounds.kv_usable_tokens_lower_bound = Some(2047);
    assert_eq!(
        resident_bytes(&profile, &policy.bounds),
        Err(BudgetError::Capacity)
    );
    policy.bounds.kv_usable_tokens_lower_bound = Some(32768);
    assert_eq!(
        resident_bytes(&profile, &policy.bounds),
        Err(BudgetError::Capacity)
    );
    policy.bounds.kv_usable_tokens_lower_bound = Some(2048);
    let mut unaligned_profile = profile.clone();
    unaligned_profile.context_tokens = 2049;
    policy.bounds.scope.profile_digest = unaligned_profile.digest().unwrap();
    policy.bounds.kv_usable_tokens_lower_bound = Some(2049);
    assert_eq!(
        resident_bytes(&unaligned_profile, &policy.bounds),
        Err(BudgetError::Capacity)
    );
    policy.bounds.kv_usable_tokens_lower_bound = Some(2080);
    assert_eq!(
        resident_bytes(&unaligned_profile, &policy.bounds),
        Ok((128 << 20) + 500)
    );
    policy.bounds.scope.profile_digest = profile.digest().unwrap();
    policy.bounds.kv_usable_tokens_lower_bound = Some(2048);
    policy.bounds.weights = Some(u64::MAX);
    assert_eq!(
        resident_bytes(&profile, &policy.bounds),
        Err(BudgetError::Overflow)
    );
    policy.bounds.weights = None;
    assert_eq!(
        resident_bytes(&profile, &policy.bounds),
        Err(BudgetError::Unqualified)
    );
    policy.bounds.weights = Some(100);
    policy.bounds.disjoint_upper_bounds = false;
    assert_eq!(
        resident_bytes(&profile, &policy.bounds),
        Err(BudgetError::Unqualified)
    );
    policy.bounds.disjoint_upper_bounds = true;
    policy.bounds.fixed_kv_pool_bytes = Some(1);
    assert_eq!(
        resident_bytes(&profile, &policy.bounds),
        Err(BudgetError::Capacity)
    );
}
