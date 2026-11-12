#![forbid(unsafe_code)]
use metron_inference_local::comparison::{ComparisonCase, ComparisonResult, ComparisonSession};
use metron_inference_local::{
    artifact::sha256_file,
    driver::{LocalInferenceDriver, QualificationOutcome, WorkerEvent},
};
use metron_protocol::{
    BoundedText, Digest, LOCAL_TEAM_RECIPE_SHA, LocalDevice, LocalOutputSchema, LocalProfile,
    LocalRole, PrincipalId, SchemaVersion, SqlU64, TaskId, content_digest,
    gguf_embedded_tokenizer_digest,
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    directory: PathBuf,
    driver: LocalInferenceDriver,
    session: ComparisonSession,
    events: Arc<Mutex<Vec<(WorkerEvent, Instant)>>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.driver.shutdown();
        let _ = fs::remove_dir_all(&self.directory);
    }
}
fn fixture(mode: &str, concurrency: u16) -> Fixture {
    let directory = std::env::temp_dir().join(format!(
        "metron-stream-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&directory).unwrap();
    let worker = directory.join("worker");
    let script = format!(
        r##"#!/usr/bin/python3
import json,struct,sys
mode={mode:?}
def read():
 h=sys.stdin.buffer.read(4)
 if len(h)!=4: sys.exit(0)
 return json.loads(sys.stdin.buffer.read(struct.unpack('>I',h)[0]))
def send(kind,r,**kw):
 b=json.dumps(dict(kind=kind,dispatch_id=r['dispatch_id'],**kw),separators=(',',':')).encode()
 sys.stdout.buffer.write(struct.pack('>I',len(b))+b); sys.stdout.buffer.flush()
def start(r):
 send('arrived',r,in_flight=(['f'*64] if mode=='arrival_unknown' else []))
 if mode=='arrival_duplicate': send('arrived',r,in_flight=[])
 send('tokenized',r,token_count=1,token_ids_digest='a'*64,rendered_digest='b'*64)
 send('started',r)
def delta(r,n,text,reasoning=False): send('delta',r,sequence=n,text=text,reasoning=reasoning)
def finish(r):
 o=dict(schema_version=2,profile_digest=PROFILE,request_digest=r['request_digest'],input_digest=r['input_digest'],content='22',reasoning=('r'*1200 if mode=='burst' and r['cohort']['position']==0 else None),prompt_tokens=1,completion_tokens=1,submission_cohort=r['cohort'],backend_batch=dict(kind='unknown'),response=dict(kind='text'))
 send('completed',r,observation=o,prompt_time_us=1,completion_time_us=1)
a=read(); start(a)
if mode=='no_stream': finish(a)
elif mode=='burst':
 for n in range(1200): delta(a,n,'r',True)
 b=read(); start(b); delta(a,1200,'22'); finish(a); delta(b,0,'22'); finish(b)
elif mode=='gap': delta(a,0,'2'); delta(a,2,'2')
elif mode=='mismatch':
 delta(a,0,'2'); delta(a,1,'3'); finish(a)
elif mode=='stagger':
 delta(a,0,'2'); b=read(); start(b); delta(a,1,'2'); finish(a); delta(b,0,'22'); finish(b)
else:
 delta(a,0,'2'); delta(a,1,'2'); finish(a)
while sys.stdin.buffer.read(1): pass
"##
    );
    // Replace the profile digest only after hashing the executable would be circular;
    // its exact value is supplied through a separate immutable fixture file.
    let script = script.replacen(
        "mode=",
        &format!(
            "PROFILE=open({:?}).read().strip()\nmode=",
            directory.join("profile-digest").to_str().unwrap()
        ),
        1,
    );
    fs::write(&worker, script).unwrap();
    fs::set_permissions(&worker, fs::Permissions::from_mode(0o700)).unwrap();
    let artifact_digest =
        Digest::new("3793e1e6e2266919d6e015287221b41100175a98eddaba92e111aed3b05c93c4").unwrap();
    let profile = LocalProfile {
        schema_version: SchemaVersion::V2,
        auxiliary_repository: BoundedText::new("LiquidAI/LFM3-1.7B").unwrap(),
        auxiliary_revision: BoundedText::new("1da06bec6a819da10b54c67e5c90972b46d81871").unwrap(),
        artifact_repository: BoundedText::new("LiquidAI/LFM3-1.7B-GGUF").unwrap(),
        artifact_revision: BoundedText::new("cd0c74c575b5c416f3011b5b4c298077f26ca8c2").unwrap(),
        artifact_name: BoundedText::new("LFM3-1.7B-Q4_K_M.gguf").unwrap(),
        artifact_bytes: SqlU64::new(1099627808).unwrap(),
        embedded_tokenizer_digest: gguf_embedded_tokenizer_digest(&artifact_digest).unwrap(),
        artifact_digest,
        template_digest: content_digest(b"template"),
        prompt_recipe_digest: Digest::new(LOCAL_TEAM_RECIPE_SHA).unwrap(),
        backend_revision: BoundedText::new("fb4cdc3730dc64b366724e7637a205a8b7747499").unwrap(),
        backend_build_digest: Digest::new(sha256_file(&worker).unwrap()).unwrap(),
        cpu_identity: BoundedText::new("fixture").unwrap(),
        cpu_features_digest: content_digest(b"fixture"),
        device: LocalDevice::Cpu,
        cpu_threads: 2,
        context_tokens: 8192,
        max_input_tokens: 8064,
        max_output_tokens: 128,
        temperature_milli: 100,
        top_k: 50,
        repeat_penalty_milli: 1100,
        max_sequences: concurrency,
        queue_capacity: 8,
        queue_wait_ms: 5000,
        deadline_ms: 5000,
        chunked_prefill: false,
        scoped_prefix_cache: false,
        cache_target_bytes: None,
        strict_memory_cap_bytes: None,
    };
    fs::write(
        directory.join("profile-digest"),
        profile.digest().unwrap().as_str(),
    )
    .unwrap();
    let session = ComparisonSession::create(
        &directory,
        content_digest(b"session"),
        TaskId::new("task").unwrap(),
        PrincipalId::new("principal").unwrap(),
        profile.clone(),
        8,
    )
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink_events = events.clone();
    let driver = LocalInferenceDriver::new(
        worker,
        directory.join("unused-model"),
        profile,
        Some(Arc::new(move |event| {
            sink_events
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .push((event, Instant::now()))
        })),
    )
    .unwrap();
    Fixture {
        directory,
        driver,
        session,
        events,
    }
}
fn case(id: &str) -> ComparisonCase {
    ComparisonCase {
        id: BoundedText::new(id).unwrap(),
        role: LocalRole::Reasoner,
        output_schema: LocalOutputSchema::Text,
        context: BoundedText::new("x ".repeat(5000)).unwrap(),
    }
}

#[test]
fn local_large_input_and_valid_stream_settle_one_receipt() {
    let mut f = fixture("valid", 1);
    let permits = f.session.admit(&[case("one")]).unwrap();
    let id = permits[0].request().dispatch_id.clone();
    let mut outcomes = f.driver.execute_comparison_cohort(permits, Duration::ZERO);
    let QualificationOutcome::Observed(observation) = outcomes.remove(0) else {
        panic!("expected observed stream")
    };
    let receipts = f
        .session
        .settle(&[ComparisonResult {
            dispatch_id: id,
            observation: Some(*observation),
            rejection: None,
        }])
        .unwrap();
    assert!(receipts[0].is_some());
    let events = f.events.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|(e, _)| matches!(e, WorkerEvent::Spawned { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|(e, _)| matches!(e, WorkerEvent::Delta { .. }))
            .count(),
        2
    );
}
#[test]
fn malformed_or_inconsistent_stream_is_uncertain_and_blocks_session() {
    for mode in [
        "gap",
        "mismatch",
        "no_stream",
        "arrival_unknown",
        "arrival_duplicate",
    ] {
        let mut f = fixture(mode, 1);
        let permits = f.session.admit(&[case("one")]).unwrap();
        let id = permits[0].request().dispatch_id.clone();
        assert!(matches!(
            f.driver
                .execute_comparison_cohort(permits, Duration::ZERO)
                .as_slice(),
            [QualificationOutcome::Uncertain]
        ));
        assert!(
            f.session
                .settle(&[ComparisonResult {
                    dispatch_id: id,
                    observation: None,
                    rejection: None
                }])
                .unwrap()[0]
                .is_none()
        );
        assert!(f.session.admit(&[case("two")]).is_err());
    }
}

#[test]
fn output_burst_cannot_block_a_later_large_request() {
    let mut f = fixture("burst", 2);
    let mut second = case("two");
    second.context = BoundedText::new("\0".repeat(60_000)).unwrap();
    let permits = f.session.admit(&[case("one"), second]).unwrap();
    let outcomes = f
        .driver
        .execute_comparison_cohort(permits, Duration::from_millis(100));
    assert!(
        outcomes
            .iter()
            .all(|o| matches!(o, QualificationOutcome::Observed(_)))
    );
    let events = f.events.lock().unwrap();
    assert!(
        events
            .iter()
            .filter(|(event, _)| matches!(event, WorkerEvent::Delta { .. }))
            .count()
            >= 1202
    );
}
#[test]
fn staggered_submission_does_not_delay_the_first_stream() {
    let mut f = fixture("stagger", 2);
    let permits = f.session.admit(&[case("one"), case("two")]).unwrap();
    let second = permits[1].request().dispatch_id.clone();
    let outcomes = f
        .driver
        .execute_comparison_cohort(permits, Duration::from_millis(500));
    assert!(
        outcomes
            .iter()
            .all(|o| matches!(o, QualificationOutcome::Observed(_)))
    );
    let events = f.events.lock().unwrap();
    let second_sent = events
        .iter()
        .find_map(|(e, t)| match e {
            WorkerEvent::Sent { dispatch_id } if dispatch_id == &second => Some(*t),
            _ => None,
        })
        .unwrap();
    let (wire, callback) = events
        .iter()
        .find_map(|(e, t)| match e {
            WorkerEvent::Delta { received_at, .. } => Some((*received_at, *t)),
            _ => None,
        })
        .unwrap();
    assert!(
        wire < second_sent,
        "first content arrived before the deliberately delayed second request"
    );
    assert!(
        callback < second_sent,
        "first stream is delivered while the later request is delayed"
    );
}
