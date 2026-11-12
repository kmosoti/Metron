use metron_protocol::*;

fn local_profile() -> LocalProfile {
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
fn coding_recipe_hash_and_role_are_bound() {
    let bytes = format!("{LOCAL_CODING_SYSTEM}\n{LOCAL_CODING_SCHEMA}");
    assert_eq!(
        content_digest(bytes.as_bytes()).as_str(),
        LOCAL_CODING_RECIPE_SHA
    );
    assert_eq!(local_coding_schema()["anyOf"].as_array().unwrap().len(), 13);
    let mut profile = local_profile();
    profile.prompt_recipe_digest = Digest::new(LOCAL_CODING_RECIPE_SHA).unwrap();
    let context = "task";
    let mut request = WorkerRequest::Generate {
        dispatch_id: content_digest(b"dispatch"),
        request_digest: content_digest(b"request"),
        task_id: TaskId::new("task").unwrap(),
        principal_id: PrincipalId::new("principal").unwrap(),
        action_digest: content_digest(b"action"),
        input_digest: content_digest(context.as_bytes()),
        role: LocalRole::Reasoner,
        output_schema: LocalOutputSchema::Text,
        context: BoundedText::new(context).unwrap(),
        profile: Box::new(profile),
        artifact_path: BoundedText::new("/model.gguf").unwrap(),
        cohort: Box::new(SubmissionCohort {
            id: BoundedText::new("cohort").unwrap(),
            size: 1,
            position: 0,
        }),
    };
    assert!(request.validate().is_ok());
    if let WorkerRequest::Generate {
        role,
        output_schema,
        ..
    } = &mut request
    {
        *role = LocalRole::Solver;
        *output_schema = LocalOutputSchema::Sum;
    }
    assert!(request.validate().is_err());
    if let WorkerRequest::Generate {
        role,
        output_schema,
        ..
    } = &mut request
    {
        *role = LocalRole::ToolSelector;
        *output_schema = LocalOutputSchema::ToolSelection;
    }
    assert!(request.validate().is_err());
    if let WorkerRequest::Generate {
        role,
        output_schema,
        profile,
        ..
    } = &mut request
    {
        *role = LocalRole::Reasoner;
        *output_schema = LocalOutputSchema::Text;
        profile.prompt_recipe_digest = Digest::new(LOCAL_TEAM_RECIPE_SHA).unwrap();
    }
    assert!(request.validate().is_ok());
    if let WorkerRequest::Generate {
        role,
        output_schema,
        profile,
        ..
    } = &mut request
    {
        *role = LocalRole::Solver;
        *output_schema = LocalOutputSchema::Sum;
        profile.prompt_recipe_digest = content_digest(b"original recipe");
    }
    assert!(request.validate().is_ok());
}

#[test]
fn malformed_coding_output_remains_unrepaired_text() {
    use metron_protocol::coding::CodingAction;
    let raw = "{\"tool\":\"finish\",\"summary\":";
    assert_eq!(
        parse_local_response(raw.as_bytes(), LocalOutputSchema::Text).unwrap(),
        LocalResponse::Text
    );
    assert!(serde_json::from_str::<CodingAction>(raw).is_err());
    assert!(
        serde_json::from_str::<CodingAction>(r#"{"tool":"finish","summary":"done","extra":true}"#)
            .is_err()
    );
    let action: CodingAction =
        serde_json::from_str(r#"{"tool":"finish","summary":"done"}"#).unwrap();
    assert!(action.validate().is_ok());
}

#[test]
fn longer_coding_deadline_is_bounded_and_does_not_relax_legacy_profiles() {
    let mut profile = local_profile();
    profile.prompt_recipe_digest = Digest::new(LOCAL_CODING_RECIPE_SHA).unwrap();
    profile.deadline_ms = 300_000;
    assert!(profile.validate().is_ok());
    profile.deadline_ms = 300_001;
    assert!(profile.validate().is_err());
    profile.prompt_recipe_digest = Digest::new(LOCAL_TEAM_RECIPE_SHA).unwrap();
    profile.deadline_ms = 60_001;
    assert!(profile.validate().is_err());
}

#[test]
fn coding_v2_recipe_is_frozen_and_binds_role_and_profile_limits() {
    assert_eq!(
        content_digest(format!("{LOCAL_CODING_SYSTEM}\n{LOCAL_CODING_SCHEMA}").as_bytes()).as_str(),
        "bd9a20e8b360d48ec4715eefb7823a6a86219a1bad9b04a33bbcfb2e2876190a"
    );
    assert_eq!(
        content_digest(format!("{LOCAL_CODING_V2_SYSTEM}\n{LOCAL_CODING_V2_SCHEMA}").as_bytes())
            .as_str(),
        LOCAL_CODING_V2_RECIPE_SHA
    );
    assert_eq!(
        LOCAL_CODING_V2_RECIPE_SHA,
        "aea699410fcda3c36aaa4367cb21fce162896d89e845a39b631d5150d1709681"
    );
    assert!(is_local_coding_recipe(LOCAL_CODING_RECIPE_SHA));
    assert!(is_local_coding_recipe(LOCAL_CODING_V2_RECIPE_SHA));
    assert!(!is_local_coding_recipe(LOCAL_TEAM_RECIPE_SHA));
    assert_eq!(
        local_coding_recipe(LOCAL_CODING_V2_RECIPE_SHA).unwrap().0,
        LOCAL_CODING_V2_SYSTEM
    );
    assert_eq!(
        local_coding_recipe(LOCAL_CODING_RECIPE_SHA).unwrap().1,
        local_coding_schema()
    );
    assert!(local_coding_recipe("unknown").is_none());
    let mut profile = local_profile();
    profile.prompt_recipe_digest = Digest::new(LOCAL_CODING_V2_RECIPE_SHA).unwrap();
    profile.deadline_ms = 300_000;
    assert!(profile.validate().is_ok());
    profile.deadline_ms += 1;
    assert!(profile.validate().is_err());
    profile.deadline_ms = 60_000;
    let context = "task";
    let mut request = WorkerRequest::Generate {
        dispatch_id: content_digest(b"dispatch"),
        request_digest: content_digest(b"request"),
        task_id: TaskId::new("task").unwrap(),
        principal_id: PrincipalId::new("principal").unwrap(),
        action_digest: content_digest(b"action"),
        input_digest: content_digest(context.as_bytes()),
        role: LocalRole::Reasoner,
        output_schema: LocalOutputSchema::Text,
        context: BoundedText::new(context).unwrap(),
        profile: Box::new(profile),
        artifact_path: BoundedText::new("/model.gguf").unwrap(),
        cohort: Box::new(SubmissionCohort {
            id: BoundedText::new("cohort").unwrap(),
            size: 1,
            position: 0,
        }),
    };
    assert!(request.validate().is_ok());
    if let WorkerRequest::Generate {
        role,
        output_schema,
        ..
    } = &mut request
    {
        *role = LocalRole::Solver;
        *output_schema = LocalOutputSchema::Sum;
    }
    assert!(request.validate().is_err());
    if let WorkerRequest::Generate {
        role,
        output_schema,
        ..
    } = &mut request
    {
        *role = LocalRole::ToolSelector;
        *output_schema = LocalOutputSchema::ToolSelection;
    }
    assert!(request.validate().is_err());
}

#[test]
fn coding_v2_schema_tracks_public_action_bounds() {
    let schema = local_coding_recipe(LOCAL_CODING_V2_RECIPE_SHA).unwrap().1;
    let branches = schema["anyOf"].as_array().unwrap();
    let props = |tool: &str| {
        branches
            .iter()
            .find(|v| v["properties"]["tool"]["const"] == tool)
            .unwrap()["properties"]
            .clone()
    };
    assert_eq!(props("plan")["steps"]["minItems"], 1);
    assert_eq!(props("plan")["steps"]["maxItems"], 12);
    assert_eq!(props("read")["first_line"]["minimum"], 1);
    assert_eq!(props("read")["line_count"]["minimum"], 1);
    assert_eq!(props("read")["line_count"]["maximum"], 200);
    assert_eq!(props("memory_search")["limit"]["minimum"], 1);
    assert_eq!(props("memory_search")["limit"]["maximum"], 20);
    let path_pattern = r"^[^/\\\x00\n\r]+(/[^/\\\x00\n\r]+)*$";
    for (tool, key) in [
        ("read", "path"),
        ("edit", "path"),
        ("list", "directory"),
        ("search", "directory"),
        ("run", "directory"),
    ] {
        assert_eq!(props(tool)[key]["pattern"], path_pattern);
    }
    for (tool, key) in [
        ("edit", "expected_digest"),
        ("memory_put", "source_evidence"),
    ] {
        assert_eq!(props(tool)[key]["pattern"], "^[0-9a-f]{64}$");
    }
    for action in [
        r#"{"tool":"read","path":"/workspace","first_line":1,"line_count":1}"#,
        r#"{"tool":"read","path":"file","first_line":0,"line_count":1}"#,
        r#"{"tool":"read","path":"file","first_line":1,"line_count":201}"#,
        r#"{"tool":"memory_search","query":"x","limit":21}"#,
        r#"{"tool":"plan","steps":[]}"#,
    ] {
        assert!(
            metron_protocol::coding::parse_coding_action(action.as_bytes()).is_err(),
            "{action}"
        );
    }
    assert!(
        metron_protocol::coding::parse_coding_action(
            br#"{"tool":"read","path":"src/main.rs","first_line":1,"line_count":200}"#
        )
        .is_ok()
    );
}
