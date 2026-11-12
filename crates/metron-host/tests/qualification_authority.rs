use metron_inference_local::qualification::{
    QualificationResult, QualificationSession, SessionKind, SessionSources,
};
use metron_protocol::{
    BoundedText, Digest, LocalProfile, SchemaVersion, SqlU64, content_digest,
    inference::LocalDevice,
};
const RECIPE: &[u8] = include_bytes!("../../metron-testkit/fixtures/prompt-recipe-v1.json");
const CALIBRATION: &[u8] = include_bytes!("../../metron-testkit/fixtures/calibration-v1.json");

fn profile() -> LocalProfile {
    LocalProfile {
        schema_version: SchemaVersion::V2,
        auxiliary_repository: BoundedText::new("LiquidAI/LFM3-1.7B").unwrap(),
        auxiliary_revision: BoundedText::new("1da06bec6a819da10b54c67e5c90972b46d81871").unwrap(),
        artifact_repository: BoundedText::new("LiquidAI/LFM3-1.7B-GGUF").unwrap(),
        artifact_revision: BoundedText::new("cd0c74c575b5c416f3011b5b4c298077f26ca8c2").unwrap(),
        artifact_name: BoundedText::new("LFM3-1.7B-Q4_K_M.gguf").unwrap(),
        artifact_digest: Digest::new(
            "3793e1e6e2266919d6e015287221b41100175a98eddaba92e111aed3b05c93c4",
        )
        .unwrap(),
        artifact_bytes: SqlU64::new(1099627808).unwrap(),
        embedded_tokenizer_digest: metron_protocol::inference::gguf_embedded_tokenizer_digest(
            &Digest::new("3793e1e6e2266919d6e015287221b41100175a98eddaba92e111aed3b05c93c4")
                .unwrap(),
        )
        .unwrap(),
        template_digest: content_digest(b"template"),
        prompt_recipe_digest: content_digest(RECIPE),
        backend_revision: BoundedText::new("fb4cdc3730dc64b366724e7637a205a8b7747499").unwrap(),
        backend_build_digest: content_digest(b"build"),
        cpu_identity: BoundedText::new("test-cpu").unwrap(),
        cpu_features_digest: content_digest(b"features"),
        device: LocalDevice::Cpu,
        cpu_threads: 2,
        context_tokens: 2048,
        max_input_tokens: 1536,
        max_output_tokens: 512,
        temperature_milli: 100,
        top_k: 50,
        repeat_penalty_milli: 1100,
        max_sequences: 2,
        queue_capacity: 8,
        queue_wait_ms: 5000,
        deadline_ms: 60000,
        chunked_prefill: false,
        scoped_prefix_cache: false,
        cache_target_bytes: None,
        strict_memory_cap_bytes: None,
    }
}
#[test]
fn uncertain_and_restart_cannot_remint_authority() {
    let dir = std::env::temp_dir().join(format!("metron-qualification-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).unwrap();
    let id = content_digest(b"session");
    let mut session = QualificationSession::create(
        &dir,
        id.clone(),
        SessionKind::Cold,
        profile(),
        SessionSources {
            recipe: RECIPE,
            corpus: CALIBRATION,
            calibration: CALIBRATION,
            calibration_proof: None,
        },
    )
    .unwrap();
    assert!(session.admit(&[("made-up", 0, false)]).is_err());
    let permit = session
        .admit(&[("calibration-sum-02", 0, false)])
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(permit.request().context.as_str(), "{\"values\":[17,-4,9]}");
    assert!(session.admit(&[("calibration-tool-00", 0, false)]).is_err());
    session
        .settle(&[QualificationResult {
            dispatch_id: permit.request().dispatch_id.clone(),
            observation: None,
            rejection: None,
        }])
        .unwrap();
    assert!(session.admit(&[("calibration-tool-00", 0, false)]).is_err());
    drop(session);
    assert!(
        QualificationSession::create(
            &dir,
            id,
            SessionKind::Cold,
            profile(),
            SessionSources {
                recipe: RECIPE,
                corpus: CALIBRATION,
                calibration: CALIBRATION,
                calibration_proof: None
            }
        )
        .is_err()
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn warm_session_requires_settled_warmup_and_rejects_foreign_ids() {
    let dir = std::env::temp_dir().join(format!("metron-warm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).unwrap();
    let mut session = QualificationSession::create(
        &dir,
        content_digest(b"warm-session"),
        SessionKind::Warm,
        profile(),
        SessionSources {
            recipe: RECIPE,
            corpus: CALIBRATION,
            calibration: CALIBRATION,
            calibration_proof: None,
        },
    )
    .unwrap();
    assert!(session.admit(&[("calibration-sum-02", 0, false)]).is_err());
    let permit = session
        .admit(&[("calibration-sum-02", 0, true)])
        .unwrap()
        .pop()
        .unwrap();
    let wrong = QualificationResult {
        dispatch_id: content_digest(b"wrong"),
        observation: None,
        rejection: None,
    };
    assert!(session.settle(&[wrong]).is_err());
    let correct = QualificationResult {
        dispatch_id: permit.request().dispatch_id.clone(),
        observation: None,
        rejection: None,
    };
    assert!(session.settle(&[correct]).is_err());
    assert!(session.admit(&[("calibration-tool-00", 0, false)]).is_err());
    drop(session);
    std::fs::remove_dir_all(dir).unwrap();
}
