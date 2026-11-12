//! Deterministic identities for one-shot local measurement sessions.
use metron_protocol::{Digest, LocalProfile, content_digest, structural_digest};
use std::{io, path::Path};

pub fn session_id(
    namespace: &Path,
    profile: &LocalProfile,
    phase: &str,
    sources: &[&[u8]],
) -> io::Result<Digest> {
    let profile_digest = profile.digest().map_err(io::Error::other)?;
    derive_id(namespace, &profile_digest, phase, sources)
}

fn derive_id(
    namespace: &Path,
    profile_digest: &Digest,
    phase: &str,
    sources: &[&[u8]],
) -> io::Result<Digest> {
    std::fs::create_dir_all(namespace)?;
    let canonical_namespace = std::fs::canonicalize(namespace)?;
    let namespace = canonical_namespace.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "session namespace must be UTF-8",
        )
    })?;
    let sources: Vec<_> = sources
        .iter()
        .map(|source| content_digest(source))
        .collect();
    let mut fields: Vec<&[u8]> = vec![
        namespace.as_bytes(),
        profile_digest.as_str().as_bytes(),
        phase.as_bytes(),
    ];
    fields.extend(sources.iter().map(|digest| digest.as_str().as_bytes()));
    structural_digest("host-measurement-session/v1", &fields).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::{derive_id, session_id};
    use metron_inference_local::comparison::{ComparisonCase, ComparisonResult, ComparisonSession};
    use metron_protocol::{
        BoundedText, Digest, LOCAL_TEAM_RECIPE_SHA, LocalDevice, LocalOutputSchema, LocalProfile,
        LocalRole, PrincipalId, SchemaVersion, SqlU64, TaskId, content_digest,
        gguf_embedded_tokenizer_digest,
    };
    use std::path::Path;
    fn local_profile() -> LocalProfile {
        LocalProfile {
            schema_version: SchemaVersion::V2,
            auxiliary_repository: BoundedText::new("LiquidAI/LFM3-1.7B").unwrap(),
            auxiliary_revision: BoundedText::new("1da06bec6a819da10b54c67e5c90972b46d81871")
                .unwrap(),
            artifact_repository: BoundedText::new("LiquidAI/LFM3-1.7B-GGUF").unwrap(),
            artifact_revision: BoundedText::new("cd0c74c575b5c416f3011b5b4c298077f26ca8c2")
                .unwrap(),
            artifact_name: BoundedText::new("LFM3-1.7B-Q4_K_M.gguf").unwrap(),
            artifact_digest: Digest::new(
                "3793e1e6e2266919d6e015287221b41100175a98eddaba92e111aed3b05c93c4",
            )
            .unwrap(),
            artifact_bytes: SqlU64::new(1099627808).unwrap(),
            embedded_tokenizer_digest: gguf_embedded_tokenizer_digest(
                &Digest::new("3793e1e6e2266919d6e015287221b41100175a98eddaba92e111aed3b05c93c4")
                    .unwrap(),
            )
            .unwrap(),
            template_digest: content_digest(b"template"),
            prompt_recipe_digest: content_digest(b"recipe"),
            backend_revision: BoundedText::new("fb4cdc3730dc64b366724e7637a205a8b7747499").unwrap(),
            backend_build_digest: content_digest(b"build"),
            cpu_identity: BoundedText::new("test-cpu").unwrap(),
            cpu_features_digest: content_digest(b"cpu-features"),
            device: LocalDevice::Cpu,
            cpu_threads: 2,
            context_tokens: 2048,
            max_input_tokens: 1536,
            max_output_tokens: 512,
            temperature_milli: 100,
            top_k: 50,
            repeat_penalty_milli: 1100,
            max_sequences: 1,
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
    fn identity_canonicalizes_aliases_and_binds_inputs() {
        let root =
            std::env::temp_dir().join(format!("metron-identity-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let directory = root.join("one");
        let alias = directory.join(".");
        let other = root.join("two");
        let profile = local_profile();
        let digest = profile.digest().unwrap();
        let id = |namespace: &Path, profile: &Digest, phase: &str, source: &[u8]| {
            derive_id(namespace, profile, phase, &[b"recipe", source]).unwrap()
        };
        let original = id(&directory, &digest, "warm", b"corpus");
        assert_eq!(original, id(&alias, &digest, "warm", b"corpus"));
        assert_ne!(original, id(&other, &digest, "warm", b"corpus"));
        assert_ne!(
            original,
            id(
                &directory,
                &content_digest(b"other profile"),
                "warm",
                b"corpus"
            )
        );
        assert_ne!(original, id(&directory, &digest, "cold-0", b"corpus"));
        assert_ne!(original, id(&directory, &digest, "warm", b"other corpus"));
        assert_ne!(
            original,
            derive_id(&directory, &digest, "warm", &[b"other recipe", b"corpus"]).unwrap()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn alias_cannot_reopen_uncertain_comparison_session() {
        let root =
            std::env::temp_dir().join(format!("metron-identity-collision-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let alias = root.join(".");
        let mut profile = local_profile();
        profile.prompt_recipe_digest = Digest::new(LOCAL_TEAM_RECIPE_SHA).unwrap();
        let id = session_id(&root, &profile, "comparison", &[b"recipe", b"cases"]).unwrap();
        let task = TaskId::new("task").unwrap();
        let principal = PrincipalId::new("principal").unwrap();
        let mut session = ComparisonSession::create(
            &root,
            id.clone(),
            task.clone(),
            principal.clone(),
            profile.clone(),
            1,
        )
        .unwrap();
        let permits = session
            .admit(&[ComparisonCase {
                id: BoundedText::new("case").unwrap(),
                role: LocalRole::Reasoner,
                output_schema: LocalOutputSchema::Text,
                context: BoundedText::new("hello").unwrap(),
            }])
            .unwrap();
        session
            .settle(&[ComparisonResult {
                dispatch_id: permits[0].request().dispatch_id.clone(),
                observation: None,
                rejection: None,
            }])
            .unwrap();
        drop(session);
        let alias_id = session_id(&alias, &profile, "comparison", &[b"recipe", b"cases"]).unwrap();
        assert_eq!(id, alias_id);
        assert!(ComparisonSession::create(&alias, alias_id, task, principal, profile, 1).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
