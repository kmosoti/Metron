use super::experiment::{
    ControlledExperimentFixtureV1, ExperimentAttemptResultV1, ExperimentRequest,
    ExperimentTransportConfig, FrozenCorpusVectorV1, evaluate_promotion_gate,
    load_and_verify_corpus_bytes, run_experiment, seal_draft_with_corpus_vectors,
    verify_native_promotion_receipt,
};
use super::io::{
    collect_workspace_snapshot, inspect_predecessor_journal_read_only, open_external_nofollow,
    read_external_bounded_bytes, read_open_file_bounded,
};
use super::propose::{UNCERTAIN_TEST_SENTINEL, derive_team_text_profile};
use super::run;
use super::runner::{
    CodingAttemptRequest, CustomRunnerFactory, execute_contract_coding_attempt,
    resume_contract_coding_attempt,
};
use super::types::*;
use super::validation::{
    HostAuthContext, HostAuthSource, build_contract_report, expected_check_specs_for_bundle,
    validate_against_auth, verify_report_structural_only,
};
use super::verifier::{
    check_interface_lexical_tokens, strip_rust_comments_and_literals, validate_cargo_metadata_json,
};
use crate::{
    coding_acceptance::{CargoStage, EvaluatorRunner, RunOutput, RunnerError},
    coding_run,
    coding_tools::CodingTools,
};
use metron_harness::{
    driver::DriverOutcome,
    orchestrator::{self, CodingVerifier, DriveError, NoObserver},
    policy::{AuthorizedEffect, EffectDriver},
};
use metron_inference_local::comparison::{ComparisonCase, ComparisonResult, ComparisonSession};
use metron_protocol::{
    ActualUsage, ApprovedRequest, AuditSnapshot, BoundedText, BudgetRequest,
    CodingVerificationSelection, DecimalUsage, Digest, EngineMode, Genesis, LocalOutputSchema,
    LocalProfile, LocalRole, MAX_INTERNAL, MAX_LOCAL_CONTEXT, MAX_TEXT, PreparedIntent,
    PrincipalId, SqlU64, StoreOperation, StorePort, StoreResult, Task, TaskId, Transition,
    UsageVector, WorkerId, WriteOutcome,
    coding::{CodingFamily, CodingPolicy, CodingTask, CodingTaskData, CodingTool, WorkspacePath},
    coding_runtime::{CodingObservation, CodingStep, CodingToolOutput, ToolStatus},
    content_digest,
    domain::{Receipt, ReceiptData, ReceiptOutput},
    store::{AdmissionCheck, DispatchStatus, RuntimeSnapshot},
    structural_digest,
    toolkit::{ToolCatalog, ToolId, ToolPin},
};
use metron_store_sqlite::SqliteStore;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, symlink},
    path::{Path, PathBuf},
};

fn temp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("metron-contract-m1-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
    dir
}

fn write_file_bytes(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    f.write_all(bytes).unwrap();
    f.sync_all().unwrap();
}

fn base_new_draft(req: &str) -> ContractDraftV1 {
    ContractDraftV1 {
        schema_version: 1,
        contract_id: BoundedText::new("case-add").unwrap(),
        workflow: ContractWorkflow::New,
        raw_request: BoundedText::new(req).unwrap(),
        raw_request_sha256: content_digest(req.as_bytes()),
        sources: vec![ContractSourceEntryV1 {
            id: BoundedText::new("src-request").unwrap(),
            claimed_kind: SourceKind::OperatorRequest,
            host_kind: None,
            label: BoundedText::new("request.txt").unwrap(),
            content_sha256: content_digest(req.as_bytes()),
            host_authenticated: false,
        }],
        baseline: ContractBaselineV1 {
            workspace_label: Some(BoundedText::new("empty-new").unwrap()),
            files: vec![],
            local_rust_package_count: 1,
            selected_cli_binary: BoundedText::new("metron-case").unwrap(),
            selected_cli_target_count: 1,
        },
        compatibility: ContractCompatibilityV1 {
            approved_compatibility: vec![],
            intended_delta: vec![BoundedText::new("implement new sum binary").unwrap()],
            preserved_files: vec![],
            allowed_edit_paths: vec![WorkspacePath::new("src/main.rs").unwrap()],
            forbidden_files: vec![WorkspacePath::new(".env").unwrap()],
        },
        requirements: ContractRequirementsV1 {
            mandatory_obligations: vec![ContractObligationV1 {
                id: BoundedText::new("r1").unwrap(),
                clause: BoundedText::new("sum two signed integers in [-1000,1000]").unwrap(),
                checker: BoundedText::new(CHECKER_CLI_SELECTED_VECTORS_V1).unwrap(),
                source_ref: BoundedText::new("src-request").unwrap(),
                approved_exception: false,
            }],
            assumptions: vec![],
            resolutions: vec![],
            unknowns: vec![],
        },
        checker_policy: ContractCheckerPolicyV1 {
            metadata_checker: BoundedText::new(CHECKER_METADATA_SNAPSHOT_LOCAL_PATH_V1).unwrap(),
            build_checker: BoundedText::new(CHECKER_BUILD_CARGO_OFFLINE_LOCKED_V1).unwrap(),
            test_checker: BoundedText::new(CHECKER_TEST_CARGO_TEST_OFFLINE_LOCKED_V1).unwrap(),
            cli_checker: Some(BoundedText::new(CHECKER_CLI_SELECTED_VECTORS_V1).unwrap()),
            max_local_packages: 2,
            fixed_files: vec![],
            interface_checks: vec![],
            cli_vectors: vec![SealedCliVectorV1 {
                args: vec![
                    BoundedUtf8::new("2").unwrap(),
                    BoundedUtf8::new("3").unwrap(),
                ],
                expected_exit_code: 0,
                expected_stdout: BoundedUtf8::new("5\n").unwrap(),
            }],
        },
        lineage: None,
        untrusted_claims: None,
    }
}

fn base_existing_workspace_and_draft(root: &Path, req: &str) -> (PathBuf, ContractDraftV1) {
    let ws = root.join("workspace");
    fs::DirBuilder::new().mode(0o700).create(&ws).unwrap();
    let cargo_toml = b"[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";
    let cargo_lock = b"# lock v4\nversion = 4\n";
    let main_rs = b"fn main() { println!(\"v1\"); }\n";
    write_file_bytes(&ws.join("Cargo.toml"), cargo_toml);
    write_file_bytes(&ws.join("Cargo.lock"), cargo_lock);
    write_file_bytes(&ws.join("src/main.rs"), main_rs);

    let draft = ContractDraftV1 {
        schema_version: 1,
        contract_id: BoundedText::new("case-existing").unwrap(),
        workflow: ContractWorkflow::Existing,
        raw_request: BoundedText::new(req).unwrap(),
        raw_request_sha256: content_digest(req.as_bytes()),
        sources: vec![
            ContractSourceEntryV1 {
                id: BoundedText::new("src-request").unwrap(),
                claimed_kind: SourceKind::OperatorRequest,
                host_kind: None,
                label: BoundedText::new("request.txt").unwrap(),
                content_sha256: content_digest(req.as_bytes()),
                host_authenticated: false,
            },
            ContractSourceEntryV1 {
                id: BoundedText::new("src-file:src/main.rs").unwrap(),
                claimed_kind: SourceKind::RepositoryFile,
                host_kind: None,
                label: BoundedText::new("src/main.rs").unwrap(),
                content_sha256: content_digest(main_rs),
                host_authenticated: false,
            },
        ],
        baseline: ContractBaselineV1 {
            workspace_label: Some(BoundedText::new("existing-demo").unwrap()),
            files: vec![
                BaselineFileRefV1 {
                    path: WorkspacePath::new("Cargo.lock").unwrap(),
                    sha256: content_digest(cargo_lock),
                    preserved: true,
                },
                BaselineFileRefV1 {
                    path: WorkspacePath::new("Cargo.toml").unwrap(),
                    sha256: content_digest(cargo_toml),
                    preserved: true,
                },
                BaselineFileRefV1 {
                    path: WorkspacePath::new("src/main.rs").unwrap(),
                    sha256: content_digest(main_rs),
                    preserved: false,
                },
            ],
            local_rust_package_count: 1,
            selected_cli_binary: BoundedText::new("demo").unwrap(),
            selected_cli_target_count: 1,
        },
        compatibility: ContractCompatibilityV1 {
            approved_compatibility: vec![BoundedText::new("preserve --version v1").unwrap()],
            intended_delta: vec![BoundedText::new("fix integer addition sign").unwrap()],
            preserved_files: vec![
                WorkspacePath::new("Cargo.lock").unwrap(),
                WorkspacePath::new("Cargo.toml").unwrap(),
            ],
            allowed_edit_paths: vec![WorkspacePath::new("src/main.rs").unwrap()],
            forbidden_files: vec![],
        },
        requirements: ContractRequirementsV1 {
            mandatory_obligations: vec![ContractObligationV1 {
                id: BoundedText::new("r1").unwrap(),
                clause: BoundedText::new("fix subtraction bug to compute sum").unwrap(),
                checker: BoundedText::new(CHECKER_CLI_SELECTED_VECTORS_V1).unwrap(),
                source_ref: BoundedText::new("src-request").unwrap(),
                approved_exception: false,
            }],
            assumptions: vec![],
            resolutions: vec![],
            unknowns: vec![],
        },
        checker_policy: ContractCheckerPolicyV1 {
            metadata_checker: BoundedText::new(CHECKER_METADATA_SNAPSHOT_LOCAL_PATH_V1).unwrap(),
            build_checker: BoundedText::new(CHECKER_BUILD_CARGO_OFFLINE_LOCKED_V1).unwrap(),
            test_checker: BoundedText::new(CHECKER_TEST_CARGO_TEST_OFFLINE_LOCKED_V1).unwrap(),
            cli_checker: Some(BoundedText::new(CHECKER_CLI_SELECTED_VECTORS_V1).unwrap()),
            max_local_packages: 2,
            fixed_files: vec![FixedFileCheckV1 {
                id: BoundedText::new("lock-fixed").unwrap(),
                path: WorkspacePath::new("Cargo.lock").unwrap(),
                expected_sha256: content_digest(cargo_lock),
            }],
            interface_checks: vec![],
            cli_vectors: vec![SealedCliVectorV1 {
                args: vec![
                    BoundedUtf8::new("1").unwrap(),
                    BoundedUtf8::new("2").unwrap(),
                ],
                expected_exit_code: 0,
                expected_stdout: BoundedUtf8::new("3\n").unwrap(),
            }],
        },
        lineage: None,
        untrusted_claims: None,
    };
    (ws, draft)
}

#[test]
fn empty_cli_args_and_empty_or_lf_stdout_roundtrip() {
    let root = temp_root("empty-strings");
    let ws = root.join("workspace");
    fs::DirBuilder::new().mode(0o700).create(&ws).unwrap();
    let req = "Accept single empty argument, return LF or empty stdout cleanly.";
    let req_path = root.join("request.txt");
    write_file_bytes(&req_path, req.as_bytes());

    let mut draft = base_new_draft(req);
    draft.checker_policy.cli_vectors = vec![
        SealedCliVectorV1 {
            args: vec![BoundedUtf8::new("").unwrap()],
            expected_exit_code: 0,
            expected_stdout: BoundedUtf8::new("\n").unwrap(),
        },
        SealedCliVectorV1 {
            args: vec![],
            expected_exit_code: 2,
            expected_stdout: BoundedUtf8::new("").unwrap(),
        },
    ];
    let draft_path = root.join("draft.json");
    write_file_bytes(&draft_path, &serde_json::to_vec(&draft).unwrap());

    // Roundtrip serde with "" arg, "" stdout, "\n" stdout, plus NUL/overflow rejection.
    assert!(BoundedUtf8::<16>::new("").is_ok());
    assert!(BoundedUtf8::<16>::new("\n").is_ok());
    assert!(BoundedUtf8::<16>::new("a\0b").is_err());
    assert!(BoundedUtf8::<4>::new("ééé").is_err());
    let decoded: ContractDraftV1 = serde_json::from_slice(&fs::read(&draft_path).unwrap()).unwrap();
    assert_eq!(decoded.checker_policy.cli_vectors[0].args[0].as_str(), "");
    assert_eq!(
        decoded.checker_policy.cli_vectors[0]
            .expected_stdout
            .as_str(),
        "\n"
    );
    assert_eq!(
        decoded.checker_policy.cli_vectors[1]
            .expected_stdout
            .as_str(),
        ""
    );

    let val = run(&[
        "validate".into(),
        "--draft".into(),
        draft_path.display().to_string(),
        "--request-file".into(),
        req_path.display().to_string(),
        "--workspace".into(),
        ws.display().to_string(),
    ])
    .unwrap();
    assert_eq!(val["outcome"], "succeeded");
    assert_eq!(val["declared_ready"], true);

    let digest = val["prospective_bundle_digest"]
        .as_str()
        .unwrap()
        .to_string();
    let bundle_path = root.join("bundle.json");
    let frozen = run(&[
        "freeze".into(),
        "--draft".into(),
        draft_path.display().to_string(),
        "--request-file".into(),
        req_path.display().to_string(),
        "--workspace".into(),
        ws.display().to_string(),
        "--expected-digest".into(),
        digest,
        "--output".into(),
        bundle_path.display().to_string(),
    ])
    .unwrap();
    assert_eq!(frozen["outcome"], "succeeded");
    let bundle: ContractBundleV1 =
        serde_json::from_slice(&fs::read(&bundle_path).unwrap()).unwrap();
    assert_eq!(bundle.body.checker_policy.cli_vectors.len(), 2);
    assert_eq!(
        bundle.body.checker_policy.cli_vectors[0].args[0].as_str(),
        ""
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn readiness_authority_blocks_spurious_model_approvals_and_unbacked_flags() {
    let root = temp_root("readiness-auth");
    let ws = root.join("workspace");
    fs::DirBuilder::new().mode(0o700).create(&ws).unwrap();
    let req = "Support rounding cents.";
    let req_path = root.join("request.txt");
    write_file_bytes(&req_path, req.as_bytes());
    let repo_fake_clar = "Round halfway away from zero.";
    let repo_file = root.join("repo-notes.txt");
    write_file_bytes(&repo_file, repo_fake_clar.as_bytes());
    let real_clar = "Round halfway away from zero; assume ASCII decimal input.";
    let clar_path = root.join("clarification.txt");
    write_file_bytes(&clar_path, real_clar.as_bytes());

    // 1. Model sets resolution.approved=true with NO clarification file -> MUST remain blocked!
    let mut no_clar = base_new_draft(req);
    no_clar.sources.push(ContractSourceEntryV1 {
        id: BoundedText::new("src-repo").unwrap(),
        claimed_kind: SourceKind::RepositoryFile,
        host_kind: None,
        label: BoundedText::new("repo-notes.txt").unwrap(),
        content_sha256: content_digest(repo_fake_clar.as_bytes()),
        host_authenticated: false,
    });
    no_clar.requirements.unknowns.push(ContractUnknownV1 {
        id: BoundedText::new("u1").unwrap(),
        question: BoundedText::new("halfway tie-break?").unwrap(),
        material: true,
        resolved_by: Some(BoundedText::new("res1").unwrap()),
        blocking: true,
    });
    no_clar.requirements.resolutions.push(ContractResolutionV1 {
        id: BoundedText::new("res1").unwrap(),
        unknown_id: BoundedText::new("u1").unwrap(),
        answer: BoundedText::new("Round halfway away from zero.").unwrap(),
        source_ref: BoundedText::new("src-repo").unwrap(),
        approved: true, // model self-approval MUST be ignored!
    });
    let no_clar_path = root.join("no-clar.json");
    write_file_bytes(&no_clar_path, &serde_json::to_vec(&no_clar).unwrap());
    let res_repo = run(&[
        "validate".into(),
        "--draft".into(),
        no_clar_path.display().to_string(),
        "--request-file".into(),
        req_path.display().to_string(),
        "--source".into(),
        format!("src-repo={}", repo_file.display()),
    ])
    .unwrap();
    assert_eq!(res_repo["outcome"], "blocked");
    assert_eq!(res_repo["declared_ready"], false);
    let codes: BTreeSet<&str> = res_repo["report"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains("unauthenticated_resolution"));
    assert!(codes.contains("unresolved_unknown"));

    // 2. Repository file claiming ClarificationAnswer source_kind fails host authentication.
    let mut spoof_kind = no_clar.clone();
    spoof_kind.sources[1].claimed_kind = SourceKind::ClarificationAnswer;
    let spoof_path = root.join("spoof.json");
    write_file_bytes(&spoof_path, &serde_json::to_vec(&spoof_kind).unwrap());
    let res_spoof = run(&[
        "validate".into(),
        "--draft".into(),
        spoof_path.display().to_string(),
        "--request-file".into(),
        req_path.display().to_string(),
        "--source".into(),
        format!("src-repo={}", repo_file.display()),
    ])
    .unwrap();
    assert_eq!(res_spoof["outcome"], "blocked");
    assert_eq!(res_spoof["structurally_valid"], false);

    // 3. Setting both `material: false` and `blocking: false` on unresolved unknown STILL blocks!
    let mut bypassed_flags = base_new_draft(req);
    bypassed_flags
        .requirements
        .unknowns
        .push(ContractUnknownV1 {
            id: BoundedText::new("u-quiet").unwrap(),
            question: BoundedText::new("silent detail").unwrap(),
            material: false,
            resolved_by: None,
            blocking: false,
        });
    let (flags_rep, _) = validate_against_auth(&bypassed_flags, None).unwrap();
    assert!(flags_rep.structurally_valid);
    assert!(!flags_rep.declared_ready);
    assert!(
        flags_rep
            .findings
            .iter()
            .any(|f| f.code.as_str() == "unresolved_unknown")
    );

    // 4. Material assumption with `approved: true` backed only by OperatorRequest stays blocked
    //    until backed by ClarificationAnswer or TrustedSpec containing exact text.
    let mut model_assumption = base_new_draft(req);
    model_assumption
        .requirements
        .assumptions
        .push(ContractAssumptionV1 {
            id: BoundedText::new("a1").unwrap(),
            text: BoundedText::new("assume ASCII decimal input").unwrap(),
            material: true,
            source_ref: BoundedText::new("src-request").unwrap(),
            approved: true,
        });
    let assumption_path = root.join("assumption.json");
    write_file_bytes(
        &assumption_path,
        &serde_json::to_vec(&model_assumption).unwrap(),
    );
    let res_assump_blocked = run(&[
        "validate".into(),
        "--draft".into(),
        assumption_path.display().to_string(),
        "--request-file".into(),
        req_path.display().to_string(),
    ])
    .unwrap();
    assert_eq!(res_assump_blocked["outcome"], "blocked");
    assert_eq!(res_assump_blocked["declared_ready"], false);
    assert!(
        res_assump_blocked["report"]["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "unsupported_material_assumption")
    );

    // Now back BOTH resolution & material assumption with host-authenticated clarification bytes -> READY!
    let mut resolved = base_new_draft(req);
    resolved.sources.push(ContractSourceEntryV1 {
        id: BoundedText::new("src-clarification").unwrap(),
        claimed_kind: SourceKind::ClarificationAnswer,
        host_kind: None,
        label: BoundedText::new("clarification.txt").unwrap(),
        content_sha256: content_digest(real_clar.as_bytes()),
        host_authenticated: false,
    });
    resolved.requirements.unknowns.push(ContractUnknownV1 {
        id: BoundedText::new("u1").unwrap(),
        question: BoundedText::new("halfway tie-break?").unwrap(),
        material: true,
        resolved_by: Some(BoundedText::new("res1").unwrap()),
        blocking: true,
    });
    resolved
        .requirements
        .resolutions
        .push(ContractResolutionV1 {
            id: BoundedText::new("res1").unwrap(),
            unknown_id: BoundedText::new("u1").unwrap(),
            answer: BoundedText::new("Round halfway away from zero").unwrap(),
            source_ref: BoundedText::new("src-clarification").unwrap(),
            approved: false, // Notice: host clarification text authorizes regardless of draft `approved`!
        });
    resolved
        .requirements
        .assumptions
        .push(ContractAssumptionV1 {
            id: BoundedText::new("a1").unwrap(),
            text: BoundedText::new("assume ASCII decimal input").unwrap(),
            material: true,
            source_ref: BoundedText::new("src-clarification").unwrap(),
            approved: false,
        });
    let resolved_path = root.join("resolved.json");
    write_file_bytes(&resolved_path, &serde_json::to_vec(&resolved).unwrap());
    let res_ok = run(&[
        "validate".into(),
        "--draft".into(),
        resolved_path.display().to_string(),
        "--request-file".into(),
        req_path.display().to_string(),
        "--clarification-file".into(),
        clar_path.display().to_string(),
        "--workspace".into(),
        ws.display().to_string(),
    ])
    .unwrap();
    assert_eq!(res_ok["outcome"], "succeeded");
    assert_eq!(res_ok["declared_ready"], true);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn checker_slot_substitution_and_missing_required_payload_rejected() {
    // Role swap: valid registry checker placed in wrong slot must fail!
    let mut swapped = base_new_draft("req");
    swapped.checker_policy.metadata_checker =
        BoundedText::new(CHECKER_BUILD_CARGO_OFFLINE_LOCKED_V1).unwrap();
    swapped.checker_policy.build_checker =
        BoundedText::new(CHECKER_METADATA_SNAPSHOT_LOCAL_PATH_V1).unwrap();
    let (swap_rep, _) = validate_against_auth(&swapped, None).unwrap();
    assert!(!swap_rep.structurally_valid);
    assert!(
        swap_rep
            .findings
            .iter()
            .any(|f| f.code.as_str() == "checker_slot_mismatch")
    );

    // Obligation pointing to fixed-file-v1 with empty fixed_files MUST fail!
    let mut missing_payload = base_new_draft("req");
    missing_payload.requirements.mandatory_obligations[0].checker =
        BoundedText::new(CHECKER_FIXED_FILE_V1).unwrap();
    let (miss_rep, _) = validate_against_auth(&missing_payload, None).unwrap();
    assert!(!miss_rep.structurally_valid);
    assert!(
        miss_rep
            .findings
            .iter()
            .any(|f| f.code.as_str() == "missing_checker_payload")
    );

    // Obligation pointing to source-literal-v1 with empty substrings MUST fail!
    let mut empty_iface = base_new_draft("req");
    empty_iface.requirements.mandatory_obligations[0].checker =
        BoundedText::new(CHECKER_SOURCE_LITERAL_V1).unwrap();
    empty_iface
        .checker_policy
        .interface_checks
        .push(InterfaceCheckV1 {
            id: BoundedText::new("iface1").unwrap(),
            path: WorkspacePath::new("src/main.rs").unwrap(),
            mode: InterfaceCheckModeV1::SourceLexicalTokens,
            required_substrings: vec![],
            forbidden_substrings: vec![],
        });
    let (iface_rep, _) = validate_against_auth(&empty_iface, None).unwrap();
    assert!(!iface_rep.structurally_valid);
    assert!(
        iface_rep
            .findings
            .iter()
            .any(|f| f.code.as_str() == "missing_checker_payload")
    );
}

#[test]
fn same_text_in_compat_and_delta_not_blocked_by_string_heuristic() {
    let root = temp_root("compat-delta-text");
    let req = "Keep CLI output format clean while adding feature.";
    let (ws, mut draft) = base_existing_workspace_and_draft(&root, req);
    let _ = ws;
    // Free-text string matching should NOT trigger synthetic contradiction blocking.
    let shared_phrase = BoundedText::new("--version prints exact schema").unwrap();
    draft
        .compatibility
        .approved_compatibility
        .push(shared_phrase.clone());
    draft.compatibility.intended_delta.push(shared_phrase);
    let (rep, _) = validate_against_auth(&draft, None).unwrap();
    assert!(rep.structurally_valid);
    assert!(rep.declared_ready);

    // Real typed scope contradiction MUST still fail!
    draft
        .compatibility
        .allowed_edit_paths
        .push(WorkspacePath::new("Cargo.lock").unwrap());
    let (conflict_rep, _) = validate_against_auth(&draft, None).unwrap();
    assert!(!conflict_rep.structurally_valid);
    let codes: BTreeSet<&str> = conflict_rep
        .findings
        .iter()
        .map(|f| f.code.as_str())
        .collect();
    assert!(codes.contains("scope_conflict"));
    assert!(codes.contains("fixed_file_edit_conflict"));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn predecessor_typed_dispatched_held_and_coherent_wal_identity() {
    let root = temp_root("wal-predecessor");
    let pred_run = root.join("pred-run");
    let pred_ws = root.join("pred-ws");
    fs::DirBuilder::new().mode(0o700).create(&pred_run).unwrap();
    fs::DirBuilder::new().mode(0o700).create(&pred_ws).unwrap();

    let req = "Supersede active dispatched task coherently across WAL.";
    let req_path = root.join("request.txt");
    write_file_bytes(&req_path, req.as_bytes());
    let pred_draft = base_new_draft(req);
    let pred_draft_path = root.join("pred-draft.json");
    write_file_bytes(&pred_draft_path, &serde_json::to_vec(&pred_draft).unwrap());
    let pred_val = run(&[
        "validate".into(),
        "--draft".into(),
        pred_draft_path.display().to_string(),
        "--request-file".into(),
        req_path.display().to_string(),
    ])
    .unwrap();
    let pred_digest = Digest::new(pred_val["prospective_bundle_digest"].as_str().unwrap()).unwrap();
    let pred_bundle_path = root.join("pred-bundle.json");
    run(&[
        "freeze".into(),
        "--draft".into(),
        pred_draft_path.display().to_string(),
        "--request-file".into(),
        req_path.display().to_string(),
        "--expected-digest".into(),
        pred_digest.as_str().to_string(),
        "--output".into(),
        pred_bundle_path.display().to_string(),
    ])
    .unwrap();

    let pred_task_id = TaskId::new("pred-wal-task").unwrap();
    let principal = PrincipalId::new("operator").unwrap();
    let pred_task = CodingTask::new(CodingTaskData {
        schema_version: 3,
        family: CodingFamily::CodingV1,
        task_id: pred_task_id.clone(),
        principal_id: principal.clone(),
        project_id: BoundedText::new("contracts").unwrap(),
        objective: BoundedText::new("predecessor wal test").unwrap(),
        acceptance_digest: pred_digest.clone(),
        tool_catalog_digest: content_digest(b"catalog"),
        model_profile_digest: content_digest(b"coding-scripted/v1"),
        memory_collection: None,
        budget: BudgetRequest {
            max_model_requests: SqlU64::new(4).unwrap(),
            max_tool_calls: SqlU64::new(4).unwrap(),
            max_spend_micros_usd: SqlU64::new(0).unwrap(),
            max_elapsed_ms: None,
        },
        max_decision_steps: 12,
    })
    .unwrap();
    let pred_policy = CodingPolicy {
        schema_version: 1,
        principal_id: principal.clone(),
        project_id: BoundedText::new("contracts").unwrap(),
        workspace_root: BoundedText::new(pred_ws.display().to_string()).unwrap(),
        memory_collection: None,
        allowed_tools: vec![CodingTool::Read, CodingTool::Finish],
        allowed_programs: vec![],
        require_mutation_approval: false,
        max_file_bytes: 65536,
        max_output_bytes: 4096,
        command_timeout_ms: 1000,
        command_memory_bytes: SqlU64::new(256 * 1024 * 1024).unwrap(),
        max_processes: 8,
    };
    let genesis = Genesis {
        mode: EngineMode::Agent,
        engine_version: BoundedText::new("coding-agent/v3.10").unwrap(),
        require_publication_approval: false,
        policy_digest: pred_policy.digest(&pred_task).unwrap(),
        local_profile: None,
        coding_policy: Some(Box::new(pred_policy.clone())),
    };
    let task_enum = Task::new_coding(pred_task.clone()).unwrap();
    let engine = crate::cognition(&genesis, &task_enum).unwrap();

    let sqlite_path = pred_run.join("runtime.sqlite");
    let mut store = SqliteStore::create(&sqlite_path).unwrap();
    store
        .apply(
            coding_run::command(StoreOperation::CreateTask {
                task: task_enum,
                genesis: genesis.clone(),
            })
            .unwrap(),
        )
        .unwrap();

    // Baseline replay_head and raw DB bytes before live WAL commit.
    let (_fd0, insp0) = inspect_predecessor_journal_read_only(&pred_run, None).unwrap();
    assert!(!insp0.unknown_hold);
    assert_eq!(insp0.open_reservations, 0);
    let main_db_sha_before = content_digest(&fs::read(&sqlite_path).unwrap());

    // Advance one coding transition + PrepareIntent + Claim + Admit WITHOUT any disk markers.
    // While `store` remains open in WAL mode, runtime.sqlite main bytes stay unchanged and
    // updates reside in runtime.sqlite-wal with exact DispatchStatus::Dispatched + ReservationStatus::Held.
    let (proj_before, pending) = {
        let mut snap = store.audit().unwrap();
        (
            snap.projection(&pred_task_id).unwrap(),
            snap.pending_inbox(&pred_task_id).unwrap().unwrap(),
        )
    };
    let genesis_digest = proj_before.state.genesis_digest.clone();
    let tr = engine
        .transition(&proj_before.state, &pending.event)
        .unwrap();
    let msg = match &tr {
        Transition::Decision { intent, .. } => intent.clone(),
        other => panic!("expected decision transition, got {other:?}"),
    };
    let prepared = PreparedIntent::materialize(&msg, &principal, &genesis_digest).unwrap();
    let iid = prepared.intent_id.clone();
    store
        .apply(
            coding_run::command(StoreOperation::CommitTransition {
                task_id: pred_task_id.clone(),
                expected_revision: proj_before.state.revision,
                inbox_id: pending.id,
                inbox_digest: pending.digest,
                transition: tr,
            })
            .unwrap(),
        )
        .unwrap();
    store
        .apply(
            coding_run::command(StoreOperation::PrepareIntent {
                task_id: pred_task_id.clone(),
                prepared: prepared.clone(),
            })
            .unwrap(),
        )
        .unwrap();
    let worker = WorkerId::new("worker-1").unwrap();
    let claim_res = store
        .apply(
            coding_run::command(StoreOperation::Claim {
                task_id: pred_task_id.clone(),
                intent_id: iid.clone(),
                worker: worker.clone(),
                expected_epoch: SqlU64::new(0).unwrap(),
            })
            .unwrap(),
        )
        .unwrap();
    let lease_epoch = match claim_res {
        WriteOutcome::Committed(reply) => match reply.result {
            StoreResult::Claimed { lease_epoch, .. } => lease_epoch,
            other => panic!("expected Claimed result, got {other:?}"),
        },
        other => panic!("expected Committed Claim outcome, got {other:?}"),
    };
    // Before Admit: intent is Leased (unadmitted pending intent == 1, unknown_hold == false).
    let (_fd_leased, insp_leased) = inspect_predecessor_journal_read_only(&pred_run, None).unwrap();
    assert!(!insp_leased.unknown_hold);
    assert_eq!(insp_leased.open_reservations, 0);
    assert_eq!(insp_leased.unadmitted_pending_intents, 1);

    let (tick, grant_revision) = {
        let mut snap = store.audit().unwrap();
        (
            snap.projection(&pred_task_id).unwrap().tick,
            snap.grant(&principal).unwrap().revision,
        )
    };
    let reservation_id = structural_digest("dispatch-id/v1", &[iid.as_str().as_bytes()]).unwrap();
    store
        .apply(
            coding_run::command(StoreOperation::Admit {
                check: AdmissionCheck {
                    intent_id: iid,
                    prepared,
                    worker,
                    lease_epoch,
                    expected_tick: tick,
                    grant_revision,
                    approval_id: None,
                    reservation_id,
                },
            })
            .unwrap(),
        )
        .unwrap();

    // Confirm raw SQLite main file missed the WAL-only commit while SqliteStore::open_read_only sees it.
    let main_db_sha_during_wal = content_digest(&fs::read(&sqlite_path).unwrap());
    assert_eq!(main_db_sha_before, main_db_sha_during_wal);
    assert!(pred_run.join("runtime.sqlite-wal").exists());

    // Confirm no directory marker files exist, yet typed inspection catches Dispatched + Held and updated replay_head!
    let (_fd1, insp1) = inspect_predecessor_journal_read_only(&pred_run, None).unwrap();
    assert!(insp1.unknown_hold);
    assert_eq!(insp1.open_reservations, 1);
    assert_ne!(insp1.replay_head, insp0.replay_head);

    let mut verify_snap = store.audit().unwrap();
    let intents = verify_snap.intents(&pred_task_id, None).unwrap();
    assert_eq!(intents.records[0].status, DispatchStatus::Dispatched);

    // Lineage with stale replay_head (insp0) or dropped unknown_hold fails; coherent insp1 succeeds!
    let mut stale_lineage = base_new_draft(req);
    stale_lineage.lineage = Some(ContractLineageV1 {
        revision: 2,
        task_id: TaskId::new("rev-task-2").unwrap(),
        predecessor_bundle_digest: pred_digest.clone(),
        predecessor_task_id: pred_task_id.clone(),
        predecessor_replay_head: Some(insp0.replay_head),
        predecessor_unknown_hold: false,
        predecessor_open_reservations: 0,
        predecessor_unadmitted_pending_intents: 1,
        asserts_unknown_reservations_cleared: false,
    });
    let stale_path = root.join("stale-lineage.json");
    write_file_bytes(&stale_path, &serde_json::to_vec(&stale_lineage).unwrap());
    let stale_val = run(&[
        "validate".into(),
        "--draft".into(),
        stale_path.display().to_string(),
        "--request-file".into(),
        req_path.display().to_string(),
        "--predecessor-bundle".into(),
        pred_bundle_path.display().to_string(),
        "--predecessor-run-dir".into(),
        pred_run.display().to_string(),
    ])
    .unwrap();
    assert_eq!(stale_val["outcome"], "blocked");
    let codes: BTreeSet<&str> = stale_val["report"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains("predecessor_replay_head_mismatch"));
    assert!(codes.contains("predecessor_unknown_hold_dropped"));
    assert!(codes.contains("predecessor_reservation_count_dropped"));

    let mut coherent = stale_lineage;
    coherent.lineage.as_mut().unwrap().predecessor_replay_head = Some(insp1.replay_head);
    coherent.lineage.as_mut().unwrap().predecessor_unknown_hold = true;
    coherent
        .lineage
        .as_mut()
        .unwrap()
        .predecessor_open_reservations = 1;
    coherent
        .lineage
        .as_mut()
        .unwrap()
        .predecessor_unadmitted_pending_intents = 0;
    let coherent_path = root.join("coherent-lineage.json");
    write_file_bytes(&coherent_path, &serde_json::to_vec(&coherent).unwrap());
    let ok_val = run(&[
        "validate".into(),
        "--draft".into(),
        coherent_path.display().to_string(),
        "--request-file".into(),
        req_path.display().to_string(),
        "--predecessor-bundle".into(),
        pred_bundle_path.display().to_string(),
        "--predecessor-run-dir".into(),
        pred_run.display().to_string(),
    ])
    .unwrap();
    assert_eq!(ok_val["outcome"], "succeeded");

    drop(verify_snap);
    drop(store);
    let _ = fs::remove_dir_all(&root);
}

struct TestScriptDriver {
    tools: CodingTools,
    script: VecDeque<String>,
}

impl EffectDriver for TestScriptDriver {
    type Outcome = DriverOutcome;
    fn execute(&mut self, effect: AuthorizedEffect) -> DriverOutcome {
        let ApprovedRequest::Coding { step, .. } = &effect.request().request else {
            panic!("unexpected request");
        };
        let prepared = effect.request();
        let (output, m, t) = match step.as_ref() {
            CodingStep::Model { input_digest, .. } => {
                let reply = self.script.pop_front().expect("script reply");
                (
                    ReceiptOutput::Coding {
                        observation: Box::new(CodingObservation::ScriptedModel {
                            input_digest: input_digest.clone(),
                            content: BoundedText::new(reply).unwrap(),
                        }),
                    },
                    1,
                    0,
                )
            }
            CodingStep::Tool { action, .. } => {
                let res = match self.tools.execute(action, &prepared.action.dispatch_id) {
                    crate::coding_tools::ToolOutcome::Known(out) => out,
                    crate::coding_tools::ToolOutcome::Uncertain(_) => CodingToolOutput {
                        tool: action.tool(),
                        status: ToolStatus::Failed,
                        body: BoundedText::new("uncertain").unwrap(),
                        truncated: false,
                        exit_code: None,
                        files: vec![],
                        process_tree_closed: true,
                    },
                };
                (
                    ReceiptOutput::Coding {
                        observation: Box::new(CodingObservation::Tool {
                            output: res,
                            memory_selection: None,
                        }),
                    },
                    0,
                    1,
                )
            }
            CodingStep::Freeze { .. } => {
                let artifact = self.tools.freeze(&prepared.action.dispatch_id).unwrap();
                (
                    ReceiptOutput::Coding {
                        observation: Box::new(CodingObservation::Artifact { artifact }),
                    },
                    0,
                    1,
                )
            }
        };
        DriverOutcome::Receipt(Box::new(
            Receipt::new(ReceiptData {
                schema_version: effect.schema_version(),
                producer_receipt_id: metron_protocol::ProducerReceiptId::new(
                    prepared.action.dispatch_id.as_str(),
                )
                .unwrap(),
                task_id: prepared.action.task_id.clone(),
                intent_id: prepared.intent_id.clone(),
                dispatch_id: prepared.action.dispatch_id.clone(),
                action_digest: prepared.action_digest.clone(),
                usage: ActualUsage::Known {
                    usage: UsageVector {
                        model_requests: DecimalUsage::new(m.to_string()).unwrap(),
                        tool_calls: DecimalUsage::new(t.to_string()).unwrap(),
                        spend_micros_usd: DecimalUsage::new("0").unwrap(),
                    },
                },
                output,
            })
            .unwrap(),
        ))
    }
}

struct TestContractVerifier {
    bundle: ContractBundleV1,
    checker_impl_digest: Digest,
    run_dir: PathBuf,
}

impl CodingVerifier for TestContractVerifier {
    fn verify(
        &mut self,
        projection: &metron_protocol::store::Projection,
    ) -> Result<(CodingVerificationSelection, bool), DriveError> {
        let task = projection.task.coding().ok_or(DriveError::Integrity)?;
        let publication = projection
            .verification
            .publication
            .as_ref()
            .ok_or(DriveError::Integrity)?;
        let ReceiptOutput::Coding { observation } = &publication.data().output else {
            return Err(DriveError::Integrity);
        };
        let CodingObservation::Artifact { artifact } = observation.as_ref() else {
            return Err(DriveError::Integrity);
        };
        let specs = expected_check_specs_for_bundle(&self.bundle);
        let checks = specs
            .into_iter()
            .map(|(id, checker)| ContractCheckResultV1 {
                id: BoundedText::new(id).unwrap(),
                checker: BoundedText::new(checker).unwrap(),
                status: ContractCheckStatus::Passed,
                output_digest: Some(content_digest(b"ok")),
            })
            .collect();
        let report = build_contract_report(
            &self.bundle,
            task.digest().unwrap(),
            artifact.tree_digest.clone(),
            self.checker_impl_digest.clone(),
            checks,
            ContractVerifierCostV1 {
                cargo_invocations: 3,
                cli_invocations: self.bundle.body.checker_policy.cli_vectors.len() as u32,
                stdout_bytes: 16,
            },
        )
        .map_err(|_| DriveError::Integrity)?;
        write_file_bytes(
            &self.run_dir.join(format!(
                "contract-report-{}.json",
                report.report_digest.as_str()
            )),
            &serde_json::to_vec(&report).unwrap(),
        );
        Ok((
            CodingVerificationSelection {
                evidence_revision: projection.verification.evidence_revision,
                plan_evidence: projection
                    .state
                    .coding_context
                    .as_ref()
                    .and_then(|c| c.plan_evidence.clone())
                    .ok_or(DriveError::Integrity)?,
                model_evidence: projection
                    .verification
                    .model_evidence
                    .clone()
                    .ok_or(DriveError::Integrity)?,
                artifact_evidence: projection
                    .verification
                    .publication_evidence
                    .clone()
                    .ok_or(DriveError::Integrity)?,
                artifact_digest: artifact.tree_digest.clone(),
                acceptance_digest: self.bundle.bundle_digest.clone(),
                evaluator_digest: report.body.evaluator_digest.clone(),
                report_digest: report.report_digest,
            },
            true,
        ))
    }
}

#[test]
fn report_authority_requires_exact_check_set_no_wall_clock_and_verified_run_dir() {
    let root = temp_root("report-authority");
    let ws = root.join("workspace");
    let run_dir = root.join("run");
    fs::DirBuilder::new().mode(0o700).create(&ws).unwrap();
    fs::DirBuilder::new().mode(0o700).create(&run_dir).unwrap();

    let req = "Build adder with preserved Cargo.lock.";
    let req_path = root.join("request.txt");
    write_file_bytes(&req_path, req.as_bytes());
    let mut draft = base_new_draft(req);
    draft.checker_policy.fixed_files.push(FixedFileCheckV1 {
        id: BoundedText::new("cfg").unwrap(),
        path: WorkspacePath::new("src/main.rs").unwrap(),
        expected_sha256: content_digest(b"fn main(){}"),
    });
    draft.compatibility.allowed_edit_paths.clear();
    let draft_path = root.join("draft.json");
    write_file_bytes(&draft_path, &serde_json::to_vec(&draft).unwrap());

    let val = run(&[
        "validate".into(),
        "--draft".into(),
        draft_path.display().to_string(),
        "--request-file".into(),
        req_path.display().to_string(),
    ])
    .unwrap();
    let digest = val["prospective_bundle_digest"]
        .as_str()
        .unwrap()
        .to_string();
    let bundle_path = root.join("bundle.json");
    run(&[
        "freeze".into(),
        "--draft".into(),
        draft_path.display().to_string(),
        "--request-file".into(),
        req_path.display().to_string(),
        "--expected-digest".into(),
        digest,
        "--output".into(),
        bundle_path.display().to_string(),
    ])
    .unwrap();
    let bundle: ContractBundleV1 =
        serde_json::from_slice(&fs::read(&bundle_path).unwrap()).unwrap();
    let bundle_file_sha = content_digest(&fs::read(&bundle_path).unwrap());

    let catalog = ToolCatalog {
        schema_version: 1,
        tools: vec![ToolPin {
            tool: ToolId::Cargo,
            executable: BoundedText::new("/unavailable-test-tool").unwrap(),
            sha256: content_digest(b"unused"),
            version_prefix: BoundedText::new("unused").unwrap(),
        }],
    };
    let checker_impl_digest = contract_verifier_implementation_digest(&catalog).unwrap();
    let expected_eval = expected_contract_evaluator_digest(&bundle, &checker_impl_digest).unwrap();

    // 1. ReportBody JSON excludes wall_ms completely (no wall-clock field in report hash).
    let expected_specs = expected_check_specs_for_bundle(&bundle);
    assert_eq!(expected_specs.len(), 6); // metadata, build, test, fixed:cfg, forbidden:.env, cli:0
    let ok_checks: Vec<_> = expected_specs
        .iter()
        .map(|(id, chk)| ContractCheckResultV1 {
            id: BoundedText::new(id.clone()).unwrap(),
            checker: BoundedText::new(*chk).unwrap(),
            status: ContractCheckStatus::Passed,
            output_digest: Some(content_digest(b"ok")),
        })
        .collect();
    let struct_report = build_contract_report(
        &bundle,
        content_digest(b"fake-task"),
        content_digest(b"fake-tree"),
        checker_impl_digest.clone(),
        ok_checks.clone(),
        ContractVerifierCostV1 {
            cargo_invocations: 3,
            cli_invocations: 1,
            stdout_bytes: 5,
        },
    )
    .unwrap();
    let report_json = serde_json::to_string(&struct_report).unwrap();
    assert!(!report_json.contains("wall_ms"));

    // 2. Omitted check-set (single check subset) is rejected cleanly even if rehashed!
    let forged_subset_body = ContractReportBodyV1 {
        schema_version: 1,
        bundle_digest: bundle.bundle_digest.clone(),
        task_digest: content_digest(b"fake-task"),
        tree_digest: content_digest(b"fake-tree"),
        checker_implementation_digest: checker_impl_digest.clone(),
        evaluator_digest: expected_eval.clone(),
        checks: vec![ok_checks[0].clone()],
        verifier_cost: ContractVerifierCostV1 {
            cargo_invocations: 1,
            cli_invocations: 0,
            stdout_bytes: 0,
        },
        accepted: true,
    };
    let forged_subset = ContractReportV1 {
        schema_version: 1,
        report_digest: report_body_digest(&forged_subset_body).unwrap(),
        body: forged_subset_body,
    };
    assert!(verify_report_structural_only(&forged_subset, &bundle).is_err());

    // 3. Rehashed structurally consistent report WITH fake evaluator_digest OR WITHOUT --run-dir cannot pass audit!
    let mut bad_eval_body = struct_report.body.clone();
    bad_eval_body.evaluator_digest = content_digest(b"forged-eval");
    let bad_eval = ContractReportV1 {
        schema_version: 1,
        report_digest: report_body_digest(&bad_eval_body).unwrap(),
        body: bad_eval_body,
    };
    assert!(verify_report_structural_only(&bad_eval, &bundle).is_err());

    let forged_report_path = root.join("forged-struct-report.json");
    write_file_bytes(
        &forged_report_path,
        &serde_json::to_vec(&struct_report).unwrap(),
    );
    let audit_without_run = run(&[
        "audit".into(),
        "--bundle".into(),
        bundle_path.display().to_string(),
        "--report".into(),
        forged_report_path.display().to_string(),
    ])
    .unwrap();
    assert_eq!(audit_without_run["outcome"], "blocked");
    assert_eq!(audit_without_run["independent_acceptance_verified"], false);

    // 4. Execute a real scripted coding task persisting contract-settings.json, committed
    //    RecordCodingVerification, CodingArtifact, and ContractReportV1, and verify both positive
    //    audit AND forged rehashed report rejection against run_dir!
    let principal = PrincipalId::new("operator").unwrap();
    let task = CodingTask::new(CodingTaskData {
        schema_version: 3,
        family: CodingFamily::CodingV1,
        task_id: TaskId::new("real-report-task").unwrap(),
        principal_id: principal.clone(),
        project_id: BoundedText::new("contracts").unwrap(),
        objective: bundle.objective.clone(),
        acceptance_digest: bundle.bundle_digest.clone(),
        tool_catalog_digest: catalog.digest().unwrap(),
        model_profile_digest: content_digest(b"coding-scripted/v1"),
        memory_collection: None,
        budget: BudgetRequest {
            max_model_requests: SqlU64::new(8).unwrap(),
            max_tool_calls: SqlU64::new(8).unwrap(),
            max_spend_micros_usd: SqlU64::new(0).unwrap(),
            max_elapsed_ms: None,
        },
        max_decision_steps: 16,
    })
    .unwrap();
    let policy = CodingPolicy {
        schema_version: 1,
        principal_id: principal,
        project_id: BoundedText::new("contracts").unwrap(),
        workspace_root: BoundedText::new(ws.display().to_string()).unwrap(),
        memory_collection: None,
        allowed_tools: vec![CodingTool::Edit, CodingTool::Finish],
        allowed_programs: vec![],
        require_mutation_approval: false,
        max_file_bytes: 65536,
        max_output_bytes: 4096,
        command_timeout_ms: 1000,
        command_memory_bytes: SqlU64::new(256 * 1024 * 1024).unwrap(),
        max_processes: 8,
    };
    let settings = ContractSettingsV1 {
        schema_version: 1,
        task: task.clone(),
        policy: policy.clone(),
        catalog: catalog.clone(),
        bundle_digest: bundle.bundle_digest.clone(),
        bundle_file_sha256: bundle_file_sha,
        checker_implementation_digest: checker_impl_digest.clone(),
        host_executable_sha256: current_host_executable_digest().unwrap(),
        expected_evaluator_digest: expected_eval,
        evidence_origin: EvidenceOriginV1::ScriptedTestDriver,
        dev_checks_enabled: false,
        architecture_scaffold: false,
        prep_requests_admitted: 0,
        remaining_coding_model_requests: 8,
        profile: None,
        worker: None,
        artifact: None,
        predecessor_run_dir: None,
        predecessor_task_id: None,
    };
    write_file_bytes(
        &run_dir.join("contract-settings.json"),
        &metron_protocol::canonical_bytes(&settings, MAX_INTERNAL).unwrap(),
    );

    let genesis = Genesis {
        mode: EngineMode::Agent,
        engine_version: BoundedText::new("coding-agent/v3.10").unwrap(),
        require_publication_approval: false,
        policy_digest: policy.digest(&task).unwrap(),
        local_profile: None,
        coding_policy: Some(Box::new(policy.clone())),
    };
    let mut store = SqliteStore::create(run_dir.join("runtime.sqlite")).unwrap();
    let task_enum = Task::new_coding(task.clone()).unwrap();
    let engine = crate::cognition(&genesis, &task_enum).unwrap();
    store
        .apply(
            coding_run::command(StoreOperation::CreateTask {
                task: task_enum,
                genesis,
            })
            .unwrap(),
        )
        .unwrap();

    let tools = CodingTools::new(&task, &policy, &catalog, &ws, &run_dir).unwrap();
    let mut driver = TestScriptDriver {
        tools,
        script: VecDeque::from(vec![
            r#"{"tool":"plan","steps":["write main.rs"]}"#.to_string(),
            r#"{"tool":"edit","path":"src/main.rs","expected_digest":null,"content":"fn main(){}"}"#.to_string(),
            r#"{"tool":"finish","summary":"done"}"#.to_string(),
        ]),
    };
    let mut verifier = TestContractVerifier {
        bundle: bundle.clone(),
        checker_impl_digest,
        run_dir: run_dir.clone(),
    };
    orchestrator::drive_coding_with_driver(
        &mut store,
        &task.data().task_id,
        engine.as_ref(),
        &mut NoObserver,
        &mut driver,
        &mut verifier,
    )
    .unwrap();
    drop(driver);
    drop(store);

    // Forged report (valid internal report_digest + fake task/tree digests) MUST fail against run_dir!
    let forged_res = run(&[
        "audit".into(),
        "--bundle".into(),
        bundle_path.display().to_string(),
        "--report".into(),
        forged_report_path.display().to_string(),
        "--run-dir".into(),
        run_dir.display().to_string(),
    ])
    .unwrap();
    assert_eq!(forged_res["outcome"], "integrity_failure");

    // Genuine persisted ContractReportV1 passes full independent audit against run_dir!
    let real_report_path = fs::read_dir(&run_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("contract-report-"))
        })
        .unwrap();
    let real_audit = run(&[
        "audit".into(),
        "--bundle".into(),
        bundle_path.display().to_string(),
        "--report".into(),
        real_report_path.display().to_string(),
        "--run-dir".into(),
        run_dir.display().to_string(),
    ])
    .unwrap();
    assert_eq!(real_audit["outcome"], "succeeded");
    assert_eq!(real_audit["independent_acceptance_verified"], true);
    assert_eq!(real_audit["accepted"], true);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn descriptor_io_rejects_ancestor_symlink_growing_file_hardlink_and_wide_empty_dirs() {
    let root = temp_root("io-hardening");

    // 1. Ancestor symlink rejected by openat2 BENEATH | NO_SYMLINKS on both read & write!
    let real_parent = root.join("real_parent");
    let sub = real_parent.join("sub");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&real_parent)
        .unwrap();
    fs::DirBuilder::new().mode(0o700).create(&sub).unwrap();
    let target = sub.join("req.txt");
    write_file_bytes(&target, b"hello");
    let sym_parent = root.join("sym_parent");
    symlink(&real_parent, &sym_parent).unwrap();

    assert!(open_external_nofollow(&sym_parent.join("sub/req.txt"), false).is_err());
    assert!(
        super::io::write_external_new_json(
            &sym_parent.join("sub/out.json"),
            None,
            &[],
            &serde_json::json!({"ok":true}),
        )
        .is_err()
    );

    // 2. Growing file after open cannot bypass bounded descriptor read.
    let grow_path = root.join("grow.txt");
    write_file_bytes(&grow_path, b"short");
    let (pinned, _) = open_external_nofollow(&grow_path, false).unwrap();
    write_file_bytes(&grow_path, b"grew past the 8 byte cap cleanly");
    assert!(read_open_file_bounded(pinned, 8).is_err());

    // 3. Wide empty directories (> 1024 entries) rejected by coding_tools descriptor snapshot.
    let wide_ws = root.join("wide-ws");
    fs::DirBuilder::new().mode(0o700).create(&wide_ws).unwrap();
    for idx in 0..1025 {
        fs::create_dir(wide_ws.join(format!("d{idx:04}"))).unwrap();
    }
    let wide_err = collect_workspace_snapshot(&wide_ws).unwrap_err();
    assert!(wide_err.contains("entry limit"));

    // 4. Hardlinked file (nlink > 1) rejected by coding_tools descriptor snapshot.
    let nlink_ws = root.join("nlink-ws");
    fs::DirBuilder::new().mode(0o700).create(&nlink_ws).unwrap();
    let original = nlink_ws.join("a.rs");
    write_file_bytes(&original, b"fn main() {}");
    fs::hard_link(&original, nlink_ws.join("b.rs")).unwrap();
    let nlink_err = collect_workspace_snapshot(&nlink_ws).unwrap_err();
    assert!(nlink_err.contains("link count"));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn utf8_multibyte_overflow_and_unknown_checker_rejected() {
    let multi = "é".repeat(520);
    assert!(BoundedText::<MAX_TEXT>::new(multi).is_err());

    let mut over = base_new_draft("req");
    for i in 0..5 {
        over.requirements
            .mandatory_obligations
            .push(ContractObligationV1 {
                id: BoundedText::new(format!("multibyte-{i}")).unwrap(),
                clause: BoundedText::new(format!("clause-{i}: {}", "é".repeat(110))).unwrap(),
                checker: BoundedText::new(CHECKER_CLI_SELECTED_VECTORS_V1).unwrap(),
                source_ref: BoundedText::new("src-request").unwrap(),
                approved_exception: false,
            });
    }
    let (rep, bundle) = validate_against_auth(&over, None).unwrap();
    assert!(bundle.is_none());
    assert!(!rep.structurally_valid);
    assert!(rep.prospective_bundle_digest.is_none());
    assert!(rep.objective_bytes.unwrap() > MAX_TEXT);
    assert!(
        rep.findings
            .iter()
            .any(|f| f.code.as_str() == "objective_overflow")
    );

    let mut bad_checker = base_new_draft("req");
    bad_checker.requirements.mandatory_obligations[0].checker =
        BoundedText::new("arbitrary-shell-v9").unwrap();
    let (bad_rep, _) = validate_against_auth(&bad_checker, None).unwrap();
    assert!(!bad_rep.structurally_valid);
    assert!(
        bad_rep
            .findings
            .iter()
            .any(|f| f.code.as_str() == "unknown_checker")
    );
}

fn sample_coding_profile_v10() -> LocalProfile {
    let artifact_dig =
        Digest::new("3793e1e6e2266919d6e015287221b41100175a98eddaba92e111aed3b05c93c4").unwrap();
    LocalProfile {
        schema_version: metron_protocol::SchemaVersion::V2,
        auxiliary_repository: BoundedText::new("LiquidAI/LFM3-1.7B").unwrap(),
        auxiliary_revision: BoundedText::new("1da06bec6a819da10b54c67e5c90972b46d81871").unwrap(),
        artifact_repository: BoundedText::new("LiquidAI/LFM3-1.7B-GGUF").unwrap(),
        artifact_revision: BoundedText::new("cd0c74c575b5c416f3011b5b4c298077f26ca8c2").unwrap(),
        artifact_name: BoundedText::new("LFM3-1.7B-Q4_K_M.gguf").unwrap(),
        artifact_digest: artifact_dig.clone(),
        artifact_bytes: SqlU64::new(1099627808).unwrap(),
        embedded_tokenizer_digest: metron_protocol::gguf_lfm3_corrected_tokenizer_digest(
            &artifact_dig,
        )
        .unwrap(),
        template_digest: content_digest(b"chatml"),
        prompt_recipe_digest: Digest::new(metron_protocol::LOCAL_CODING_V10_RECIPE_SHA).unwrap(),
        backend_revision: BoundedText::new("fb4cdc3730dc64b366724e7637a205a8b7747499").unwrap(),
        backend_build_digest: content_digest(b"worker-bin"),
        cpu_identity: BoundedText::new("test-cpu").unwrap(),
        cpu_features_digest: content_digest(b"avx2"),
        device: metron_protocol::LocalDevice::Cpu,
        cpu_threads: 4,
        context_tokens: 4096,
        max_input_tokens: 3584,
        max_output_tokens: 512,
        temperature_milli: 100,
        top_k: 50,
        repeat_penalty_milli: 1100,
        max_sequences: 1,
        queue_capacity: 1,
        queue_wait_ms: 1000,
        deadline_ms: 120_000,
        chunked_prefill: true,
        scoped_prefix_cache: true,
        cache_target_bytes: None,
        strict_memory_cap_bytes: None,
    }
}

#[test]
fn outside_sensitive_file_hardlinked_into_workspace_rejected_by_nlink_check() {
    let root = temp_root("nlink-outside-secret");
    let ws = root.join("workspace");
    let outside = root.join("outside");
    fs::DirBuilder::new().mode(0o700).create(&ws).unwrap();
    fs::DirBuilder::new().mode(0o700).create(&outside).unwrap();

    let outside_secret = outside.join("secret.txt");
    write_file_bytes(&outside_secret, b"sensitive operator secret");
    fs::hard_link(&outside_secret, ws.join("aliased.txt")).unwrap();

    // Even when workspace_dir is None or different dir, read_external_bounded_bytes rejects nlink != 1.
    let err = read_external_bounded_bytes(
        &outside_secret,
        Some(&open_external_nofollow(&ws, true).unwrap().0),
        4096,
    )
    .unwrap_err();
    assert!(err.contains("hardlinks"));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn team_profile_derivation_comparison_unknown_hold_and_propose_stages() {
    let coding = sample_coding_profile_v10();
    let (team, src_dig, team_dig) = derive_team_text_profile(&coding).unwrap();
    assert_eq!(team.context_tokens, 4096);
    assert_eq!(team.max_input_tokens, 3584);
    assert_eq!(team.max_output_tokens, 512);
    assert_eq!(
        team.prompt_recipe_digest.as_str(),
        metron_protocol::LOCAL_TEAM_RECIPE_SHA
    );
    assert_eq!(
        team.embedded_tokenizer_digest,
        metron_protocol::gguf_embedded_tokenizer_digest(&team.artifact_digest).unwrap()
    );
    assert_ne!(src_dig, team_dig);

    let mut bad_out = coding.clone();
    bad_out.max_output_tokens = 256;
    assert!(derive_team_text_profile(&bad_out).is_err());

    let root = temp_root("propose-flow");
    let cmp_dir = root.join("cmp-unknown");
    fs::DirBuilder::new().mode(0o700).create(&cmp_dir).unwrap();
    let session_dig = content_digest(b"cmp-unknown-session");
    let mut cmp = ComparisonSession::create(
        &cmp_dir,
        session_dig,
        TaskId::new("cmp-u-1").unwrap(),
        PrincipalId::new("op").unwrap(),
        team,
        3,
    )
    .unwrap();
    let permits = cmp
        .admit(&[ComparisonCase {
            id: BoundedText::new("c0").unwrap(),
            role: LocalRole::Reasoner,
            output_schema: LocalOutputSchema::Text,
            context: BoundedText::<MAX_LOCAL_CONTEXT>::new("hello").unwrap(),
        }])
        .unwrap();
    let d_id = permits[0].request().dispatch_id.clone();
    let settled = cmp
        .settle(&[ComparisonResult {
            dispatch_id: d_id,
            observation: None,
            rejection: None,
        }])
        .unwrap();
    assert!(settled[0].is_none());
    // Unknown settle retains reservation/session hold and blocks subsequent admit!
    assert!(
        cmp.admit(&[ComparisonCase {
            id: BoundedText::new("c1").unwrap(),
            role: LocalRole::Reasoner,
            output_schema: LocalOutputSchema::Text,
            context: BoundedText::<MAX_LOCAL_CONTEXT>::new("retry").unwrap(),
        }])
        .is_err()
    );

    // Propose CLI: invalid stage 0 JSON -> Revision stage 1 succeeds; recorded source != NativeTeamSession.
    let req_path = root.join("req.txt");
    write_file_bytes(&req_path, b"Create CLI metron-case adding two numbers.");
    let bad_then_good = vec![
        "not valid json at all".to_string(),
        serde_json::json!({
            "intended_delta": [],
            "approved_compatibility": [],
            "allowed_edit_paths": [],
            "preserved_files": [],
            "mandatory_obligations": [
                {"id": "ob1", "clause": "add two ints", "checker": "cargo-test-offline-locked-v1"}
            ],
            "assumptions": [],
            "unknowns": [],
            "resolutions": []
        })
        .to_string(),
    ];
    let rep_path = root.join("replies.json");
    write_file_bytes(&rep_path, &serde_json::to_vec(&bad_then_good).unwrap());
    let out_dir = root.join("propose-ok");
    let res = run(&[
        "propose".into(),
        "--request-file".into(),
        req_path.display().to_string(),
        "--contract-id".into(),
        "c-prop-1".into(),
        "--output-dir".into(),
        out_dir.display().to_string(),
        "--recorded-replies".into(),
        rep_path.display().to_string(),
    ])
    .unwrap();
    assert_eq!(res["outcome"], "succeeded");
    assert_eq!(res["evidence_origin"], "imported_recorded_replies");
    assert_eq!(res["ledger"]["admitted_prep_requests"], 2);
    assert_eq!(res["ledger"]["stages"][0]["parse_succeeded"], false);
    assert!(out_dir.join("stage-0.json").exists());
    assert!(out_dir.join("stage-1.json").exists());

    // Uncertain outcome creates propose-in-flight.json + propose-hold.json and refuses retry!
    let unc_path = root.join("uncertain.json");
    write_file_bytes(
        &unc_path,
        &serde_json::to_vec(&vec![UNCERTAIN_TEST_SENTINEL]).unwrap(),
    );
    let hold_dir = root.join("propose-hold");
    let hold_res = run(&[
        "propose".into(),
        "--request-file".into(),
        req_path.display().to_string(),
        "--contract-id".into(),
        "c-prop-hold".into(),
        "--output-dir".into(),
        hold_dir.display().to_string(),
        "--recorded-replies".into(),
        unc_path.display().to_string(),
    ])
    .unwrap();
    assert_eq!(hold_res["outcome"], "unknown_outcome_blocked");
    assert_eq!(hold_res["ledger"]["unknown_hold"], true);
    assert!(hold_dir.join("propose-in-flight.json").exists());
    assert!(hold_dir.join("propose-hold.json").exists());
    assert!(
        run(&[
            "propose".into(),
            "--request-file".into(),
            req_path.display().to_string(),
            "--contract-id".into(),
            "c-prop-hold".into(),
            "--output-dir".into(),
            hold_dir.display().to_string(),
            "--recorded-replies".into(),
            rep_path.display().to_string(),
        ])
        .is_err()
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn verifier_metadata_scope_and_comment_stripped_lexical_checks() {
    let artifact = metron_protocol::coding_runtime::CodingArtifact {
        tree_digest: content_digest(b"tree"),
        relative_root: WorkspacePath::new("snapshot").unwrap(),
        files: vec![
            metron_protocol::coding_runtime::CodingFile {
                path: WorkspacePath::new("Cargo.toml").unwrap(),
                digest: content_digest(b"[package]\nname=\"metron-case\"\n"),
                bytes: SqlU64::new(30).unwrap(),
            },
            metron_protocol::coding_runtime::CodingFile {
                path: WorkspacePath::new("crates/helper/Cargo.toml").unwrap(),
                digest: content_digest(b"[package]\nname=\"helper\"\n"),
                bytes: SqlU64::new(25).unwrap(),
            },
            metron_protocol::coding_runtime::CodingFile {
                path: WorkspacePath::new("src/main.rs").unwrap(),
                digest: content_digest(b"fn main() {}"),
                bytes: SqlU64::new(12).unwrap(),
            },
            metron_protocol::coding_runtime::CodingFile {
                path: WorkspacePath::new("crates/helper/src/lib.rs").unwrap(),
                digest: content_digest(b"pub fn helper() {}"),
                bytes: SqlU64::new(18).unwrap(),
            },
        ],
    };
    let ok_meta = serde_json::json!({
        "packages": [
            {
                "name": "metron-case",
                "source": null,
                "manifest_path": "/workspace/Cargo.toml",
                "targets": [{"name": "metron-case", "kind": ["bin"], "src_path": "/workspace/src/main.rs"}],
                "dependencies": [{"name": "helper", "source": null, "registry": null, "path": "crates/helper"}]
            },
            {
                "name": "helper",
                "source": null,
                "manifest_path": "/workspace/crates/helper/Cargo.toml",
                "targets": [{"name": "helper", "kind": ["lib"], "src_path": "/workspace/crates/helper/src/lib.rs"}],
                "dependencies": []
            }
        ]
    })
    .to_string();
    assert!(
        validate_cargo_metadata_json(
            ok_meta.as_bytes(),
            &artifact,
            "metron-case",
            2,
            &[Path::new("/workspace")],
        )
        .is_ok()
    );

    // Reject registry source, escaping dep path, and > 2 packages.
    let reg_meta = serde_json::json!({
        "packages": [{
            "name": "metron-case",
            "source": null,
            "manifest_path": "/workspace/Cargo.toml",
            "targets": [{"name": "metron-case", "kind": ["bin"], "src_path": "/workspace/src/main.rs"}],
            "dependencies": [{"name": "serde", "source": "registry+https://crates.io", "registry": null, "path": null}]
        }]
    })
    .to_string();
    assert!(
        validate_cargo_metadata_json(
            reg_meta.as_bytes(),
            &artifact,
            "metron-case",
            2,
            &[Path::new("/workspace")],
        )
        .is_err()
    );

    let esc_meta = serde_json::json!({
        "packages": [{
            "name": "metron-case",
            "source": null,
            "manifest_path": "/workspace/Cargo.toml",
            "targets": [{"name": "metron-case", "kind": ["bin"], "src_path": "/workspace/src/main.rs"}],
            "dependencies": [{"name": "evil", "source": null, "registry": null, "path": "../../outside"}]
        }]
    })
    .to_string();
    assert!(
        validate_cargo_metadata_json(
            esc_meta.as_bytes(),
            &artifact,
            "metron-case",
            2,
            &[Path::new("/workspace")],
        )
        .is_err()
    );

    // Fake comment or string literal declarations cannot pass lexical source check!
    let fake_src = r#"
        // pub fn solve(x: i32) -> i32 { x }
        /* pub fn solve(x: i32) -> i32 { x } */
        fn main() { let _ = "pub fn solve(x: i32) -> i32"; }
    "#;
    let stripped = strip_rust_comments_and_literals(fake_src);
    assert!(!stripped.contains("solve"));

    let iface_req = InterfaceCheckV1 {
        id: BoundedText::new("if1").unwrap(),
        path: WorkspacePath::new("src/main.rs").unwrap(),
        mode: InterfaceCheckModeV1::SourceLexicalTokens,
        required_substrings: vec![BoundedText::new("pub fn solve").unwrap()],
        forbidden_substrings: vec![],
    };
    assert!(check_interface_lexical_tokens(fake_src, &iface_req).is_err());

    let real_src = "pub fn solve(x: i32) -> i32 { x }\nfn main() {}\n";
    assert!(check_interface_lexical_tokens(real_src, &iface_req).is_ok());

    let mut semantic_draft = base_new_draft("Check semantic rejection");
    semantic_draft
        .checker_policy
        .interface_checks
        .push(InterfaceCheckV1 {
            id: BoundedText::new("sem").unwrap(),
            path: WorkspacePath::new("src/main.rs").unwrap(),
            mode: InterfaceCheckModeV1::SemanticRustApi,
            required_substrings: vec![BoundedText::new("solve").unwrap()],
            forbidden_substrings: vec![],
        });
    let (rep, _) = validate_against_auth(&semantic_draft, None).unwrap();
    assert!(!rep.structurally_valid);
    assert!(
        rep.findings
            .iter()
            .any(|f| f.code.as_str() == "unsupported_semantic_api_check")
    );
}

struct MockEvalRunner {
    bin_name: String,
    vectors: Vec<(Vec<String>, i32, String)>,
    fail_on_wrong_marker: bool,
    solved: bool,
}

impl EvaluatorRunner for MockEvalRunner {
    fn cargo(&mut self, stage: CargoStage) -> Result<RunOutput, RunnerError> {
        let stdout = match stage {
            CargoStage::Metadata => serde_json::json!({
                "packages": [{
                    "name": self.bin_name,
                    "source": null,
                    "manifest_path": "/workspace/Cargo.toml",
                    "targets": [{"name": self.bin_name, "src_path": "/workspace/src/main.rs", "kind": ["bin"]}],
                    "dependencies": []
                }]
            })
            .to_string()
            .into_bytes(),
            CargoStage::Build | CargoStage::Test => b"ok\n".to_vec(),
        };
        Ok(RunOutput {
            exit_code: Some(0),
            stdout,
            stderr: Vec::new(),
            process_tree_closed: true,
        })
    }

    fn candidate(&mut self, _bin_name: &str, args: &[&str]) -> Result<RunOutput, RunnerError> {
        if self.fail_on_wrong_marker && !self.solved {
            return Ok(RunOutput {
                exit_code: Some(1),
                stdout: b"wrong\n".to_vec(),
                stderr: Vec::new(),
                process_tree_closed: true,
            });
        }
        let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        if let Some((_, code, out)) = self.vectors.iter().find(|(a, _, _)| *a == args_owned) {
            return Ok(RunOutput {
                exit_code: Some(*code),
                stdout: out.as_bytes().to_vec(),
                stderr: Vec::new(),
                process_tree_closed: true,
            });
        }
        Ok(RunOutput {
            exit_code: Some(2),
            stdout: Vec::new(),
            stderr: Vec::new(),
            process_tree_closed: true,
        })
    }
}

#[test]
fn contract_driver_runner_denies_out_of_scope_edits_dev_runs_and_tampered_resume() {
    let root = temp_root("runner-driver");
    let ws = root.join("workspace");
    let out_dir = root.join("artifacts");
    fs::DirBuilder::new().mode(0o700).create(&ws).unwrap();
    fs::DirBuilder::new().mode(0o700).create(&out_dir).unwrap();

    let toml_bytes = b"[package]\nname = \"adder\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";
    let lock_bytes = b"# lock\nversion = 4\n";
    let main_bytes = b"fn main() {}\n";
    write_file_bytes(&ws.join("Cargo.toml"), toml_bytes);
    write_file_bytes(&ws.join("Cargo.lock"), lock_bytes);
    write_file_bytes(&ws.join("src/main.rs"), main_bytes);

    let mut draft = base_new_draft("Fix src/main.rs while preserving Cargo.lock.");
    draft.workflow = ContractWorkflow::Existing;
    draft.baseline.selected_cli_binary = BoundedText::new("adder").unwrap();
    draft.compatibility.intended_delta = vec![BoundedText::new("update main.rs only").unwrap()];
    draft.compatibility.preserved_files = vec![WorkspacePath::new("Cargo.lock").unwrap()];
    draft.compatibility.allowed_edit_paths = vec![WorkspacePath::new("src/main.rs").unwrap()];
    draft.compatibility.forbidden_files = vec![];
    draft.baseline.files = vec![
        BaselineFileRefV1 {
            path: WorkspacePath::new("Cargo.lock").unwrap(),
            sha256: content_digest(lock_bytes),
            preserved: true,
        },
        BaselineFileRefV1 {
            path: WorkspacePath::new("Cargo.toml").unwrap(),
            sha256: content_digest(toml_bytes),
            preserved: false,
        },
        BaselineFileRefV1 {
            path: WorkspacePath::new("src/main.rs").unwrap(),
            sha256: content_digest(main_bytes),
            preserved: false,
        },
    ];
    let (_, bundle_opt) = validate_against_auth(&draft, None).unwrap();
    let bundle = bundle_opt.unwrap();
    let bundle_path = out_dir.join("bundle.json");
    write_file_bytes(
        &bundle_path,
        &metron_protocol::canonical_bytes(&bundle, MAX_INTERNAL).unwrap(),
    );

    let catalog = ToolCatalog {
        schema_version: 1,
        tools: vec![ToolPin {
            tool: ToolId::Cargo,
            executable: BoundedText::new("/unavailable-test-tool").unwrap(),
            sha256: content_digest(b"unused"),
            version_prefix: BoundedText::new("unused").unwrap(),
        }],
    };
    let run_dir = out_dir.join("run-1");
    let solved_code = "fn main() { println!(\"5\"); }\n";
    let vectors = vec![(vec!["2".into(), "3".into()], 0, "5\n".into())];
    let runner_factory: CustomRunnerFactory = Box::new(move |artifact, _| {
        let solved = artifact.files.iter().any(|f| {
            f.path.as_str() == "src/main.rs" && f.digest == content_digest(solved_code.as_bytes())
        });
        Ok(Box::new(MockEvalRunner {
            bin_name: "adder".into(),
            vectors: vectors.clone(),
            fail_on_wrong_marker: true,
            solved,
        }))
    });

    let outcome = execute_contract_coding_attempt(CodingAttemptRequest {
        task_id: TaskId::new("runner-task-1").unwrap(),
        bundle_path,
        workspace_path: ws.clone(),
        run_dir: run_dir.clone(),
        catalog: catalog.clone(),
        dev_checks_enabled: false,
        architecture_scaffold: false,
        prep_requests_admitted: 1,
        total_model_cap: 12,
        max_tool_calls: 16,
        evidence_origin: EvidenceOriginV1::ScriptedTestDriver,
        profile_path: None,
        worker_path: None,
        artifact_path: None,
        scripted_replies: Some(vec![
            serde_json::json!({"tool": "plan", "steps": ["edit src/main.rs only", "finish"]}).to_string(),
            serde_json::json!({"tool": "edit", "path": "Cargo.lock", "expected_digest": null, "content": "corrupt"}).to_string(),
            serde_json::json!({"tool": "run", "program": "cargo", "args": ["check"]}).to_string(),
            serde_json::json!({"tool": "edit", "path": "src/main.rs", "expected_digest": content_digest(main_bytes).as_str(), "content": solved_code}).to_string(),
            serde_json::json!({"tool": "finish", "summary": "updated"}).to_string(),
        ]),
        predecessor_run_dir: None,
        predecessor_task_id: None,
        custom_verifier_runner: Some(runner_factory),
        allowed_metadata_roots: vec![PathBuf::from("/workspace")],
    })
    .unwrap();

    assert_eq!(outcome.outcome, "completed");
    assert!(outcome.accepted);
    assert_eq!(outcome.denied_scope_edits, 1);
    assert_eq!(outcome.denied_dev_runs, 0);
    assert!(outcome.report_verified);
    assert_eq!(fs::read(ws.join("Cargo.lock")).unwrap(), lock_bytes);
    assert_eq!(
        fs::read(ws.join("src/main.rs")).unwrap(),
        solved_code.as_bytes()
    );

    // Tamper checker_implementation_digest in contract-settings.json -> resume refuses cleanly!
    let settings_path = run_dir.join("contract-settings.json");
    let mut settings: ContractSettingsV1 =
        serde_json::from_slice(&fs::read(&settings_path).unwrap()).unwrap();
    settings.checker_implementation_digest = content_digest(b"tampered-impl");
    write_file_bytes(
        &settings_path,
        &metron_protocol::canonical_bytes(&settings, MAX_INTERNAL).unwrap(),
    );
    let err = resume_contract_coding_attempt(
        &run_dir,
        Some(&catalog),
        None,
        None,
        vec![PathBuf::from("/workspace")],
    )
    .unwrap_err();
    assert!(
        err.contains("checker_implementation_digest")
            || err.contains("checker implementation")
            || err.contains("mismatch"),
        "unexpected tamper error: {err}"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn current_host_executable_digest_matches_pinned_proc_self_exe_and_caches_stably() {
    let first = current_host_executable_digest().unwrap();
    let second = current_host_executable_digest().unwrap();
    assert_eq!(first, second);

    let direct_hex = {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            let file = fs::File::open("/proc/self/exe").unwrap();
            let fd_path = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
            let hex = metron_inference_local::artifact::sha256_file(&fd_path).unwrap();
            drop(file);
            hex
        }
        #[cfg(not(target_os = "linux"))]
        {
            let exe = std::env::current_exe().unwrap();
            metron_inference_local::artifact::sha256_file(&exe).unwrap()
        }
    };
    let direct_digest = Digest::new(direct_hex).unwrap();
    assert_eq!(first, direct_digest);

    let catalog = ToolCatalog {
        schema_version: 1,
        tools: vec![ToolPin {
            tool: ToolId::Cargo,
            executable: BoundedText::new("/unavailable-test-tool").unwrap(),
            sha256: content_digest(b"unused"),
            version_prefix: BoundedText::new("unused").unwrap(),
        }],
    };
    let verifier_digest = contract_verifier_implementation_digest(&catalog).unwrap();
    assert_eq!(
        verifier_digest,
        contract_verifier_implementation_digest_with_host(&catalog, &first).unwrap()
    );
    let other_build_digest = content_digest(b"different-host-executable-image");
    assert_ne!(first, other_build_digest);
    assert_ne!(
        verifier_digest,
        contract_verifier_implementation_digest_with_host(&catalog, &other_build_digest).unwrap()
    );
}

fn synthetic_attempt(
    case_id: &str,
    slice: CorpusSliceV1,
    arm: ExperimentArmV1,
    accepted: bool,
    raw_dig: Digest,
) -> ExperimentAttemptResultV1 {
    ExperimentAttemptResultV1 {
        case_id: BoundedText::new(case_id).unwrap(),
        slice,
        arm,
        raw_output_digest: raw_dig,
        clarified_output_digest: None,
        raw_declared_ready: true,
        raw_readiness_matched: true,
        prep_unknown_hold: false,
        prep_quality_failure: false,
        prep_requests_admitted: 1,
        coding_model_requests_used: 2,
        total_admitted_tool_effects: 2,
        accepted,
        authority_fault: false,
        oracle_leak: false,
        bundle_digest: None,
    }
}

fn synthetic_annotations(
    campaign_id: &str,
    corpus_sha: &Digest,
    config_dig: &Digest,
    attempts: &[ExperimentAttemptResultV1],
    reviewer: &str,
) -> ContractSemanticAnnotationSetV1 {
    ContractSemanticAnnotationSetV1 {
        schema_version: CONTRACT_SCHEMA_VERSION,
        campaign_id: BoundedText::new(campaign_id).unwrap(),
        corpus_sha256: corpus_sha.clone(),
        config_digest: config_dig.clone(),
        annotations: attempts
            .iter()
            .map(|a| CaseSemanticAnnotationV1 {
                case_id: a.case_id.clone(),
                arm: a.arm,
                stage: AnnotationStageV1::Raw,
                raw_output_digest: a.raw_output_digest.clone(),
                clause_complete: true,
                silent_material_assumptions: 0,
                unjustified_block: false,
                critical_false_ready: false,
                reviewer: BoundedText::new(reviewer).unwrap(),
                notes: BoundedText::new("reviewed").unwrap(),
            })
            .collect(),
    }
}

#[test]
fn experiment_corpus_diagnostic_gate_promotion_gate_and_run_lock() {
    let corpus_bytes = metron_testkit::coding::independent_corpus_json().as_bytes();
    let freeze_bytes = metron_testkit::coding::independent_corpus_freeze_json().as_bytes();
    let (corpus, _freeze, corpus_sha) =
        load_and_verify_corpus_bytes(corpus_bytes, freeze_bytes).unwrap();
    assert_eq!(corpus_sha.as_str(), EXPECTED_INDEPENDENT_CORPUS_SHA256);
    assert_eq!(corpus.cases.len(), 12);
    assert_eq!(corpus.diagnostic_cases.len(), 4);

    let mut tampered = corpus_bytes.to_vec();
    tampered[0] = b' ';
    assert!(load_and_verify_corpus_bytes(&tampered, freeze_bytes).is_err());

    // Diagnostic 0/2 gate skips 48-attempt run!
    let root = temp_root("exp-diag-gate");
    let c_path = root.join("corpus.json");
    let f_path = root.join("freeze.json");
    write_file_bytes(&c_path, corpus_bytes);
    write_file_bytes(&f_path, freeze_bytes);
    let catalog = ToolCatalog {
        schema_version: 1,
        tools: vec![ToolPin {
            tool: ToolId::Cargo,
            executable: BoundedText::new("/unavailable-test-tool").unwrap(),
            sha256: content_digest(b"unused"),
            version_prefix: BoundedText::new("unused").unwrap(),
        }],
    };
    let mut fail_diag_scripts = BTreeMap::new();
    for d in &corpus.diagnostic_cases {
        // Empty finish without editing solved marker causes diagnostic failure (0/2)!
        fail_diag_scripts.insert(
            d.id.clone(),
            vec![serde_json::json!({"tool": "finish", "summary": "fail"}).to_string()],
        );
    }
    let exp_out = root.join("exp-diag-block");
    let diag_report = run_experiment(ExperimentRequest {
        campaign_id: "diag-fail-test".into(),
        corpus_path: c_path.clone(),
        corpus_freeze_path: f_path.clone(),
        output_dir: exp_out.clone(),
        catalog: catalog.clone(),
        annotations_path: None,
        transport: ExperimentTransportConfig::Controlled {
            fixture: ControlledExperimentFixtureV1 {
                diagnostic_scripts: fail_diag_scripts,
                arm_scripts: BTreeMap::new(),
            },
            evidence_origin: EvidenceOriginV1::ImportedRecordedReplies,
            custom_runner_factory: Some(Box::new(|| {
                Box::new(|_, _| {
                    Ok(Box::new(MockEvalRunner {
                        bin_name: "metron-case".into(),
                        vectors: vec![],
                        fail_on_wrong_marker: true,
                        solved: false,
                    }))
                })
            })),
        },
    })
    .unwrap();
    assert!(!diag_report.diagnostic_gate.passed);
    assert_eq!(diag_report.attempts.len(), 48);
    assert!(diag_report.attempts.iter().all(|a| {
        !a.accepted
            && a.coding_model_requests_used == 0
            && a.total_admitted_tool_effects == 0
            && a.bundle_digest.is_none()
    }));
    assert!(exp_out.join("campaign-config.json").is_file());
    assert!(exp_out.join("annotation-template.json").is_file());
    assert_eq!(
        diag_report.promotion_receipt.decision.verdict,
        PromotionVerdictV1::DiagnosticGateBlocked
    );

    // Promotion gate truth table: PromotedTypedOnly, PromotedTypedPlusArchitecture, architecture_only_finding,
    // regression block, critical_false_ready block, missing/non-native annotation block.
    let cfg_dig = content_digest(b"cfg");
    let build_matrix = |pp: (bool, bool), pa: (bool, bool), tp: (bool, bool), ta: (bool, bool)| {
        [
            (ExperimentArmV1::ProsePlain, pp),
            (ExperimentArmV1::ProseArchitecture, pa),
            (ExperimentArmV1::TypedPlain, tp),
            (ExperimentArmV1::TypedArchitecture, ta),
        ]
        .into_iter()
        .flat_map(|(arm, accepted)| {
            corpus.cases.iter().map(move |case| {
                let slice = if case.workflow == ContractWorkflow::New {
                    CorpusSliceV1::New
                } else {
                    CorpusSliceV1::Existing
                };
                synthetic_attempt(
                    &case.id,
                    slice,
                    arm,
                    if slice == CorpusSliceV1::New {
                        accepted.0
                    } else {
                        accepted.1
                    },
                    content_digest(format!("{}::{arm:?}", case.id).as_bytes()),
                )
            })
        })
        .collect::<Vec<_>>()
    };

    // 1. TypedPlain beats ProsePlain (+1/+1) -> PromotedTypedOnly when TypedArch adds nothing over TypedPlain.
    let a1 = build_matrix((false, false), (false, false), (true, true), (true, true));
    let s1 = synthetic_annotations("camp1", &corpus_sha, &cfg_dig, &a1, "Sol");
    let r1 = evaluate_promotion_gate(
        "camp1",
        &corpus_sha,
        &cfg_dig,
        EvidenceOriginV1::NativeTeamSession,
        true,
        &a1,
        Some(&s1),
    )
    .unwrap();
    assert_eq!(r1.decision.verdict, PromotionVerdictV1::PromotedTypedOnly);
    assert_eq!(r1.decision.selected_arm, Some(ExperimentArmV1::TypedPlain));

    // 2. Non-native origin rejected EVEN WITH perfect annotations!
    let r_nonnative = evaluate_promotion_gate(
        "camp1",
        &corpus_sha,
        &cfg_dig,
        EvidenceOriginV1::ImportedRecordedReplies,
        true,
        &a1,
        Some(&s1),
    )
    .unwrap();
    assert_eq!(
        r_nonnative.decision.verdict,
        PromotionVerdictV1::BlockedInvalidAnnotationsOrOrigin
    );

    // 3. Mismatched annotation digest blocked!
    let mut s_bad_dig = s1.clone();
    s_bad_dig.annotations[0].raw_output_digest = content_digest(b"wrong");
    let r_bad_ann = evaluate_promotion_gate(
        "camp1",
        &corpus_sha,
        &cfg_dig,
        EvidenceOriginV1::NativeTeamSession,
        true,
        &a1,
        Some(&s_bad_dig),
    )
    .unwrap();
    assert_eq!(
        r_bad_ann.decision.verdict,
        PromotionVerdictV1::BlockedInvalidAnnotationsOrOrigin
    );

    // 4. architecture_only_finding: ProseArchitecture beats ProsePlain, Typed adds nothing!
    let a_arch = build_matrix((false, false), (true, true), (false, false), (true, true));
    let s_arch = synthetic_annotations("camp1", &corpus_sha, &cfg_dig, &a_arch, "Sol");
    let r_arch = evaluate_promotion_gate(
        "camp1",
        &corpus_sha,
        &cfg_dig,
        EvidenceOriginV1::NativeTeamSession,
        true,
        &a_arch,
        Some(&s_arch),
    )
    .unwrap();
    assert_eq!(
        r_arch.decision.verdict,
        PromotionVerdictV1::NegativeDoNotPromote
    );
    assert!(r_arch.decision.architecture_only_finding);

    // 5. critical_false_ready blocks promotion!
    let mut s_crit = s1.clone();
    s_crit
        .annotations
        .iter_mut()
        .find(|a| a.arm == ExperimentArmV1::TypedPlain)
        .unwrap()
        .critical_false_ready = true;
    let r_crit = evaluate_promotion_gate(
        "camp1",
        &corpus_sha,
        &cfg_dig,
        EvidenceOriginV1::NativeTeamSession,
        true,
        &a1,
        Some(&s_crit),
    )
    .unwrap();
    assert_eq!(
        r_crit.decision.verdict,
        PromotionVerdictV1::NegativeDoNotPromote
    );

    // 6. CLI `run` & `resume` blocked both without receipt and with non-native receipt, while verified native receipt passes verify_native_promotion_receipt!
    let receipt_path = root.join("native-receipt.json");
    let ann_path = root.join("native-ann.json");
    write_file_bytes(
        &receipt_path,
        &metron_protocol::canonical_bytes(&r1, MAX_INTERNAL).unwrap(),
    );
    write_file_bytes(
        &ann_path,
        &metron_protocol::canonical_bytes(&s1, MAX_INTERNAL).unwrap(),
    );
    let (_vr, selected) = verify_native_promotion_receipt(&receipt_path, &ann_path, None).unwrap();
    assert_eq!(selected, ExperimentArmV1::TypedPlain);

    let nonnative_path = root.join("nonnative-receipt.json");
    write_file_bytes(
        &nonnative_path,
        &metron_protocol::canonical_bytes(&r_nonnative, MAX_INTERNAL).unwrap(),
    );
    assert!(verify_native_promotion_receipt(&nonnative_path, &ann_path, None).is_err());

    let run_missing = run(&["run".into()]).unwrap();
    assert_eq!(run_missing["outcome"], "blocked");
    let resume_nonnative = run(&[
        "resume".into(),
        "--promotion-receipt".into(),
        nonnative_path.display().to_string(),
        "--annotations".into(),
        ann_path.display().to_string(),
        "--run-dir".into(),
        root.join("missing-run").display().to_string(),
    ])
    .unwrap();
    assert_eq!(resume_nonnative["outcome"], "blocked");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn seal_draft_keeps_oracles_private_and_full_matrix_architecture_promotion() {
    let corpus_bytes = metron_testkit::coding::independent_corpus_json().as_bytes();
    let freeze_bytes = metron_testkit::coding::independent_corpus_freeze_json().as_bytes();
    let (corpus, _freeze, corpus_sha) =
        load_and_verify_corpus_bytes(corpus_bytes, freeze_bytes).unwrap();
    assert_eq!(corpus.diagnostic_cases.len(), 4);
    assert_eq!(corpus.cases.len(), 12);

    // 1. Exercise seal_draft_with_corpus_vectors with HostAuthContext & HostAuthSource and verify
    //    sealed CLI vectors are attached to checker_policy without entering bundle.objective.
    let new_case = corpus
        .cases
        .iter()
        .find(|c| c.workflow == ContractWorkflow::New && !c.sealed_cli_vectors.is_empty())
        .unwrap();
    let mut auth_sources = BTreeMap::new();
    auth_sources.insert(
        "src-request".to_string(),
        HostAuthSource {
            kind: SourceKind::OperatorRequest,
            sha256: new_case.raw_request_sha256.clone(),
            label: format!("case:{}", new_case.id),
            utf8_text: Some(new_case.raw_request.clone()),
        },
    );
    let auth = HostAuthContext {
        raw_request: new_case.raw_request.clone(),
        raw_request_sha256: new_case.raw_request_sha256.clone(),
        sources_by_id: auth_sources,
        workspace_snapshot: Some(BTreeMap::new()),
        predecessor_bundle_digest: None,
        predecessor_inspection: None,
    };
    let mut draft = base_new_draft(&new_case.raw_request);
    draft.baseline.selected_cli_binary = BoundedText::new(&new_case.selected_binary).unwrap();
    let sentinel_vec = FrozenCorpusVectorV1 {
        args: vec!["--oracle-sentinel-flag".into()],
        exit_code: 0,
        stdout: "ORACLE_VECTOR_STDOUT_SENTINEL_987\n".into(),
    };
    let mut vectors_with_sentinel = new_case.sealed_cli_vectors.clone();
    vectors_with_sentinel.push(sentinel_vec);
    let (seal_report, sealed_bundle) =
        seal_draft_with_corpus_vectors(&draft, &auth, &vectors_with_sentinel).unwrap();
    assert!(seal_report.structurally_valid);
    assert!(seal_report.declared_ready);
    assert_eq!(
        sealed_bundle
            .body
            .checker_policy
            .cli_checker
            .as_ref()
            .map(BoundedText::as_str),
        Some(CHECKER_CLI_SELECTED_VECTORS_V1)
    );
    assert_eq!(
        sealed_bundle.body.checker_policy.cli_vectors.len(),
        vectors_with_sentinel.len()
    );
    assert!(
        sealed_bundle
            .objective
            .as_str()
            .contains(sealed_bundle.bundle_digest.as_str())
    );
    assert!(
        !sealed_bundle
            .objective
            .as_str()
            .contains("--oracle-sentinel-flag")
    );
    assert!(
        !sealed_bundle
            .objective
            .as_str()
            .contains("ORACLE_VECTOR_STDOUT_SENTINEL_987")
    );

    // Real-effect composition is covered by the bounded contract driver test above.
    // Promotion arithmetic uses the complete frozen matrix without repeating effects.
    let root = temp_root("exp-pure-48-matrix");

    // 3. Verify PromotedTypedPlusArchitecture verdict in evaluate_promotion_gate + verify_native_promotion_receipt
    let cfg_dig = content_digest(b"cfg-arch");
    let mut arch_attempts = Vec::new();
    let specs = [
        (ExperimentArmV1::ProsePlain, (false, false), (false, false)),
        (
            ExperimentArmV1::ProseArchitecture,
            (false, false),
            (false, false),
        ),
        (ExperimentArmV1::TypedPlain, (true, false), (true, false)),
        (
            ExperimentArmV1::TypedArchitecture,
            (true, true),
            (true, true),
        ),
    ];
    for (arm_idx, (arm, new_ok, ext_ok)) in specs.into_iter().enumerate() {
        let mut new_index = 0;
        let mut existing_index = 0;
        for case in &corpus.cases {
            let (slice, index, accepted) = if case.workflow == ContractWorkflow::New {
                let index = new_index;
                new_index += 1;
                (
                    CorpusSliceV1::New,
                    index,
                    if index % 2 == 0 { new_ok.0 } else { new_ok.1 },
                )
            } else {
                let index = existing_index;
                existing_index += 1;
                (
                    CorpusSliceV1::Existing,
                    index,
                    if index % 2 == 0 { ext_ok.0 } else { ext_ok.1 },
                )
            };
            arch_attempts.push(synthetic_attempt(
                &case.id,
                slice,
                arm,
                accepted,
                content_digest(format!("{}-{arm_idx}-{index}", case.id).as_bytes()),
            ));
        }
    }
    assert_eq!(arch_attempts.len(), 48);
    let arch_ann = synthetic_annotations("camp-arch", &corpus_sha, &cfg_dig, &arch_attempts, "Sol");
    let arch_receipt = evaluate_promotion_gate(
        "camp-arch",
        &corpus_sha,
        &cfg_dig,
        EvidenceOriginV1::NativeTeamSession,
        true,
        &arch_attempts,
        Some(&arch_ann),
    )
    .unwrap();
    assert_eq!(
        arch_receipt.decision.verdict,
        PromotionVerdictV1::PromotedTypedPlusArchitecture
    );
    assert_eq!(
        arch_receipt.decision.selected_arm,
        Some(ExperimentArmV1::TypedArchitecture)
    );
    assert!(arch_receipt.decision.eligible_typed_plain);
    assert!(arch_receipt.decision.eligible_typed_architecture);

    let receipt_path = root.join("arch-receipt.json");
    let ann_path = root.join("arch-annotations.json");
    write_file_bytes(&receipt_path, &serde_json::to_vec(&arch_receipt).unwrap());
    write_file_bytes(&ann_path, &serde_json::to_vec(&arch_ann).unwrap());
    let (verified_receipt, selected_arm) =
        verify_native_promotion_receipt(&receipt_path, &ann_path, None).unwrap();
    assert_eq!(
        verified_receipt.decision.verdict,
        PromotionVerdictV1::PromotedTypedPlusArchitecture
    );
    assert_eq!(selected_arm, ExperimentArmV1::TypedArchitecture);

    let _ = fs::remove_dir_all(&root);
}

#[test]
#[ignore = "requires explicit disk-backed METRON_CONTRACT_SMOKE_ROOT and pinned METRON_CONTRACT_SMOKE_CATALOG"]
fn real_tools_contract_composition_smoke() {
    let parent = PathBuf::from(
        std::env::var_os("METRON_CONTRACT_SMOKE_ROOT").expect("set disk-backed smoke root"),
    );
    let catalog_path = PathBuf::from(
        std::env::var_os("METRON_CONTRACT_SMOKE_CATALOG").expect("set pinned tool catalog"),
    );
    let root = parent.join(format!("real-tools-{}", std::process::id()));
    fs::create_dir_all(&parent).unwrap();
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let ws = root.join("workspace");
    fs::DirBuilder::new().mode(0o700).create(&ws).unwrap();
    let files: [(&str, &[u8]); 3] = [
        (
            "Cargo.toml",
            b"[package]\nname = \"metron-case\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        ),
        (
            "Cargo.lock",
            b"version = 4\n\n[[package]]\nname = \"metron-case\"\nversion = \"0.1.0\"\n",
        ),
        ("src/main.rs", b"fn main() {}\n"),
    ];
    for (path, bytes) in files {
        write_file_bytes(&ws.join(path), bytes);
    }
    let mut draft = base_new_draft(
        "Print the integer sum 5 for CLI arguments 2 3. Preserve Cargo.toml and Cargo.lock.",
    );
    draft.workflow = ContractWorkflow::Existing;
    draft.compatibility.approved_compatibility =
        vec![BoundedText::new("Preserve Cargo.toml and Cargo.lock.").unwrap()];
    draft.compatibility.intended_delta =
        vec![BoundedText::new("Print the integer sum 5 for CLI arguments 2 3.").unwrap()];
    draft.compatibility.preserved_files = vec![
        WorkspacePath::new("Cargo.toml").unwrap(),
        WorkspacePath::new("Cargo.lock").unwrap(),
    ];
    draft.compatibility.forbidden_files.clear();
    draft.baseline.files = files
        .into_iter()
        .map(|(path, bytes)| BaselineFileRefV1 {
            path: WorkspacePath::new(path).unwrap(),
            sha256: content_digest(bytes),
            preserved: path != "src/main.rs",
        })
        .collect();
    draft
        .baseline
        .files
        .sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
    let request_text = draft.raw_request.as_str().to_owned();
    let auth = HostAuthContext {
        raw_request_sha256: content_digest(request_text.as_bytes()),
        raw_request: request_text.clone(),
        sources_by_id: BTreeMap::from([(
            "src-request".to_owned(),
            HostAuthSource {
                kind: SourceKind::OperatorRequest,
                sha256: content_digest(request_text.as_bytes()),
                label: "real-tool smoke request".to_owned(),
                utf8_text: Some(request_text),
            },
        )]),
        workspace_snapshot: Some(
            draft
                .baseline
                .files
                .iter()
                .map(|f| (f.path.as_str().to_owned(), f.sha256.clone()))
                .collect(),
        ),
        predecessor_bundle_digest: None,
        predecessor_inspection: None,
    };
    let (validation, bundle) = validate_against_auth(&draft, Some(&auth)).unwrap();
    let bundle = bundle.unwrap_or_else(|| panic!("smoke contract invalid: {validation:?}"));
    let bundle_path = root.join("bundle.json");
    write_file_bytes(
        &bundle_path,
        &metron_protocol::canonical_bytes(&bundle, MAX_INTERNAL).unwrap(),
    );
    let catalog: ToolCatalog = serde_json::from_slice(&fs::read(catalog_path).unwrap()).unwrap();
    let solved = "fn main() { let sum: i64 = std::env::args().skip(1).map(|s| s.parse::<i64>().unwrap()).sum(); println!(\"{sum}\"); }\n";
    let outcome = execute_contract_coding_attempt(CodingAttemptRequest {
        task_id: TaskId::new("real-tools-smoke").unwrap(), bundle_path, workspace_path: ws.clone(),
        run_dir: root.join("run"), catalog, dev_checks_enabled: false, architecture_scaffold: false,
        prep_requests_admitted: 0, total_model_cap: 12, max_tool_calls: 8,
        evidence_origin: EvidenceOriginV1::ImportedRecordedReplies,
        profile_path: None, worker_path: None, artifact_path: None,
        scripted_replies: Some(vec![
            serde_json::json!({"tool":"plan", "steps":["implement sum", "finish"]}).to_string(),
            serde_json::json!({"tool":"edit", "path":"src/main.rs", "expected_digest":content_digest(files[2].1), "content":solved}).to_string(),
            serde_json::json!({"tool":"finish", "summary":"implemented sum"}).to_string(),
        ]),
        predecessor_run_dir: None, predecessor_task_id: None, custom_verifier_runner: None,
        allowed_metadata_roots: vec![PathBuf::from("/workspace")],
    }).unwrap();
    eprintln!("retained real-tools smoke evidence: {}", root.display());
    assert!(
        outcome.accepted,
        "real-tools verifier rejected: {outcome:?}"
    );
    assert!(outcome.report_verified);
    for (path, bytes) in &files[..2] {
        assert_eq!(fs::read(ws.join(path)).unwrap(), *bytes);
    }
}

#[test]
fn objective_projection_retains_complete_sources_and_exact_mandatory_bytes() {
    let (_, bundle) = validate_against_auth(&base_new_draft("seed"), None).unwrap();
    let mut body = bundle.unwrap().body;
    body.compatibility = ContractCompatibilityV1 {
        approved_compatibility: vec![],
        intended_delta: vec![],
        preserved_files: vec![],
        allowed_edit_paths: vec![],
        forbidden_files: vec![],
    };
    body.requirements.mandatory_obligations.clear();
    let corpus: super::experiment::FrozenCorpusFileV1 =
        serde_json::from_str(metron_testkit::coding::independent_corpus_json()).unwrap();
    for case in corpus.cases.iter().chain(&corpus.diagnostic_cases) {
        let mut candidate = body.clone();
        candidate.raw_request = BoundedText::new(&case.raw_request).unwrap();
        candidate.raw_request_sha256 = case.raw_request_sha256.clone();
        if let Some(clarification) = &case.frozen_clarification {
            candidate.sources.push(ContractSourceEntryV1 {
                id: BoundedText::new("src-clarification").unwrap(),
                claimed_kind: SourceKind::ClarificationAnswer,
                host_kind: Some(SourceKind::ClarificationAnswer),
                label: BoundedText::new("answer").unwrap(),
                content_sha256: content_digest(clarification.as_bytes()),
                host_authenticated: true,
            });
            let mut rest = clarification.as_str();
            let mut index = 0;
            while !rest.is_empty() {
                let end = rest.floor_char_boundary(rest.len().min(256));
                candidate
                    .requirements
                    .mandatory_obligations
                    .push(ContractObligationV1 {
                        id: BoundedText::new(format!("host-clar-1-{index}")).unwrap(),
                        clause: BoundedText::new(&rest[..end]).unwrap(),
                        checker: BoundedText::new(CHECKER_TEST_CARGO_TEST_OFFLINE_LOCKED_V1)
                            .unwrap(),
                        source_ref: BoundedText::new("src-clarification").unwrap(),
                        approved_exception: false,
                    });
                rest = &rest[end..];
                index += 1;
            }
        }
        let digest = bundle_body_digest(&candidate).unwrap();
        let objective = build_contract_objective(&digest, &candidate)
            .unwrap_or_else(|e| panic!("{}: {e:?}", case.id));
        assert!(objective.as_str().contains(&case.raw_request));
        assert!(objective.as_str().contains(digest.as_str()));
        assert!(objective.as_str().contains(PUBLIC_HARNESS_PREAMBLE));
        if let Some(answer) = &case.frozen_clarification {
            assert!(objective.as_str().contains(answer));
        }
        assert!(objective.as_str().len() <= MAX_TEXT);
        candidate.compatibility.approved_compatibility = case
            .approved_compatibility
            .iter()
            .map(|s| BoundedText::new(s).unwrap())
            .collect();
        candidate.compatibility.intended_delta = case
            .intended_delta
            .iter()
            .map(|s| BoundedText::new(s).unwrap())
            .collect();
        let digest = bundle_body_digest(&candidate).unwrap();
        match build_contract_objective(&digest, &candidate) {
            Ok(projected) => {
                for clause in candidate
                    .compatibility
                    .approved_compatibility
                    .iter()
                    .chain(&candidate.compatibility.intended_delta)
                {
                    assert!(projected.as_str().contains(clause.as_str()));
                }
            }
            Err((bytes, message)) => {
                assert!(bytes > MAX_TEXT);
                assert!(message.contains("without truncation"));
            }
        }
    }
    body.raw_request = BoundedText::new("é raw source with two  spaces\nand a newline.").unwrap();
    body.requirements
        .mandatory_obligations
        .push(ContractObligationV1 {
            id: BoundedText::new("extra").unwrap(),
            clause: BoundedText::new(" additional mandatory clause ").unwrap(),
            checker: BoundedText::new(CHECKER_TEST_CARGO_TEST_OFFLINE_LOCKED_V1).unwrap(),
            source_ref: BoundedText::new("src-request").unwrap(),
            approved_exception: false,
        });
    let digest = bundle_body_digest(&body).unwrap();
    let objective = build_contract_objective(&digest, &body).unwrap();
    assert!(objective.as_str().contains(body.raw_request.as_str()));
    assert!(
        objective
            .as_str()
            .contains("\n additional mandatory clause ")
    );
    body.requirements.assumptions.push(ContractAssumptionV1 {
        id: BoundedText::new("prepared-guidance").unwrap(),
        text: BoundedText::new("é".repeat(16)).unwrap(),
        material: false,
        source_ref: BoundedText::new("src-request").unwrap(),
        approved: false,
    });
    assert!(
        build_contract_objective(&digest, &body)
            .unwrap()
            .as_str()
            .contains(&"é".repeat(16))
    );
    body.requirements.assumptions[0].text = BoundedText::new("é".repeat(17)).unwrap();
    assert!(build_contract_objective(&digest, &body).is_err());
    body.requirements.assumptions[0].text = BoundedText::new("short").unwrap();
    body.requirements.assumptions[0].material = true;
    assert!(build_contract_objective(&digest, &body).is_err());
    body.requirements.assumptions.clear();
    body.raw_request = BoundedText::new("é".repeat(470)).unwrap();
    let (bytes, message) = build_contract_objective(&digest, &body).unwrap_err();
    assert!(bytes > MAX_TEXT);
    assert!(message.contains("without truncation"));
    body.sources.push(ContractSourceEntryV1 {
        id: BoundedText::new("missing-clarification").unwrap(),
        claimed_kind: SourceKind::ClarificationAnswer,
        host_kind: Some(SourceKind::ClarificationAnswer),
        label: BoundedText::new("missing").unwrap(),
        content_sha256: content_digest(b"exact bytes required"),
        host_authenticated: true,
    });
    assert!(
        build_contract_objective(&digest, &body)
            .unwrap_err()
            .1
            .contains("complete authenticated clarification")
    );
}
