use metron_protocol::{coding::*, *};

fn task() -> CodingTaskData {
    CodingTaskData {
        schema_version: 3,
        family: CodingFamily::CodingV1,
        task_id: TaskId::new("calc").unwrap(),
        principal_id: PrincipalId::new("operator").unwrap(),
        project_id: BoundedText::new("calculator").unwrap(),
        objective: BoundedText::new("Implement a checked Rust calculator").unwrap(),
        acceptance_digest: content_digest(b"operator acceptance"),
        tool_catalog_digest: content_digest(b"catalog"),
        model_profile_digest: content_digest(b"profile"),
        memory_collection: Some(BoundedText::new("project-notes").unwrap()),
        budget: BudgetRequest {
            max_model_requests: SqlU64::new(32).unwrap(),
            max_tool_calls: SqlU64::new(64).unwrap(),
            max_spend_micros_usd: SqlU64::new(0).unwrap(),
            max_elapsed_ms: None,
        },
        max_decision_steps: 64,
    }
}

#[test]
fn task_identity_binds_acceptance_tools_model_and_memory_without_legacy_changes() {
    let original = CodingTask::new(task()).unwrap();
    let bytes = canonical_bytes(&original, MAX_ENVELOPE).unwrap();
    let decoded: CodingTask = parse_envelope(&bytes).unwrap();
    assert_eq!(original.digest().unwrap(), decoded.digest().unwrap());
    for field in ["acceptance", "tools", "model", "memory"] {
        let mut changed = task();
        match field {
            "acceptance" => changed.acceptance_digest = content_digest(b"other"),
            "tools" => changed.tool_catalog_digest = content_digest(b"other"),
            "model" => changed.model_profile_digest = content_digest(b"other"),
            "memory" => changed.memory_collection = None,
            _ => unreachable!(),
        }
        assert_ne!(
            original.digest().unwrap(),
            CodingTask::new(changed).unwrap().digest().unwrap()
        );
    }
    for version in [0, 1, 2, 4] {
        let mut changed = task();
        changed.schema_version = version;
        assert!(CodingTask::new(changed).is_err());
    }
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["family"] = serde_json::json!("sum-v1");
    assert!(parse_envelope::<CodingTask>(&serde_json::to_vec(&value).unwrap()).is_err());
    // The legacy envelope version remains exactly its original domain.
    assert!(serde_json::from_str::<SchemaVersion>("3").is_err());
}

#[test]
fn paths_reject_lexical_escape_but_allow_valid_unicode_and_nested_files() {
    for path in [
        "",
        "/etc/passwd",
        "../x",
        "a/../x",
        "a/./x",
        "a//x",
        "x/",
        ".git/config",
        ".jj/repo",
        "a\\b",
        "a\0b",
        "a\nb",
    ] {
        assert!(WorkspacePath::new(path).is_err(), "{path:?}");
        assert!(serde_json::from_value::<WorkspacePath>(serde_json::json!(path)).is_err());
    }
    for path in ["Cargo.toml", "src/main.rs", "資料/é.rs", ".config/file"] {
        assert_eq!(WorkspacePath::new(path).unwrap().as_str(), path);
    }
}

#[test]
fn untrusted_actions_reject_extra_fields_multiple_objects_and_semantic_overruns() {
    for text in [
        r#"{"tool":"finish","summary":"done","verified":true}"#,
        r#"{"tool":"jj_diff"}{"tool":"jj_snapshot"}"#,
        r#"{"tool":"read","path":"x","first_line":0,"line_count":1}"#,
        r#"{"tool":"read","path":"x","first_line":1,"line_count":201}"#,
        r#"{"tool":"read","path":"x","first_line":4294967295,"line_count":2}"#,
        r#"{"tool":"memory_search","query":"x","limit":0}"#,
        r#"{"tool":"run","program":"sh","args":["-c","anything"],"directory":null}"#,
    ] {
        assert!(parse_coding_action(text.as_bytes()).is_err(), "{text}");
    }
    let finish =
        parse_coding_action(br#"{"tool":"finish","summary":"request independent verification"}"#)
            .unwrap();
    assert!(matches!(finish, CodingAction::Finish { .. }));
    assert!(
        parse_coding_action(
            br#"{"tool":"read","path":"x","first_line":4294967295,"line_count":1}"#
        )
        .is_ok()
    );
    let digest = finish.digest().unwrap();
    assert_eq!(
        digest,
        parse_coding_action(&canonical_bytes(&finish, MAX_ENVELOPE).unwrap())
            .unwrap()
            .digest()
            .unwrap()
    );
    assert!(parse_coding_action(&vec![b' '; MAX_ENVELOPE + 1]).is_err());
}
