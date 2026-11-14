//! Frozen 2x2 independent-corpus experiment runner, separate diagnostic gate,
//! host-side semantic annotation binding, and promotion gate locking `run`/`resume`.

use super::io::{
    create_external_private_dir, open_external_nofollow, read_external_json,
    seed_candidate_workspace, unlink_pinned_leaf, write_external_new_json,
};
use super::propose::{
    NativePreparationCase, NativePreparationInputs, ProposeParams, ProposeTransportConfig,
    derive_team_text_profile, resolve_pinned_artifact, run_propose,
};
#[cfg(test)]
use super::runner::CustomRunnerFactory;
use super::runner::{
    CodingAttemptRequest, audit_retained_admissions, execute_contract_coding_attempt,
};
use super::types::*;
use super::validation::{
    HostAuthContext, HostAuthSource, validate_against_auth, validate_against_auth_with_mode,
    validate_contract_settings_historical, verify_report_against_run_dir_historical,
};
use metron_inference_local::{
    artifact::{sha256_file, verify_artifact},
    comparison::{
        ComparisonAuditExpectation, ComparisonAuditOutcome, ComparisonCase, ComparisonResult,
        ComparisonSession, audit_comparison_session,
    },
    driver::{LocalInferenceDriver, QualificationOutcome},
};
use metron_protocol::{
    BoundedText, Digest, LocalOutputSchema, LocalProfile, LocalRole, MAX_INTERNAL,
    MAX_LOCAL_CONTEXT, MAX_TEXT, PrincipalId, TaskId, canonical_bytes, coding::WorkspacePath,
    content_digest, parse_internal, structural_digest, toolkit::ToolCatalog,
};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::collections::VecDeque;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FrozenCorpusFreezeV1 {
    pub schema_version: u16,
    pub case_count: u32,
    pub separate_diagnostic_count: u32,
    pub pilot_attempt_count: u32,
    pub native_outputs_seen_before_freeze: bool,
    pub sha256: Digest,
    #[serde(default)]
    pub supersedes_sha256: Option<Digest>,
    pub file: String,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FrozenCorpusCapsV1 {
    pub preparation_requests: u8,
    pub total_model_requests: u32,
    pub admitted_tool_effects: u32,
    pub max_cli_vectors: usize,
    pub max_files: usize,
    pub max_local_packages: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FrozenCorpusVectorV1 {
    #[serde(default)]
    pub args: Vec<String>,
    pub exit_code: i32,
    pub stdout: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FrozenCorpusObligationV1 {
    pub id: String,
    pub clause: String,
    pub checker: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FrozenCorpusCheckerPolicyV1 {
    pub metadata: String,
    pub build: String,
    pub test: String,
    pub cli: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FrozenCorpusCaseV1 {
    pub id: String,
    pub workflow: ContractWorkflow,
    pub regime: String,
    pub raw_request: String,
    pub raw_request_sha256: Digest,
    #[serde(default)]
    pub approved_compatibility: Vec<String>,
    #[serde(default)]
    pub baseline_files: BTreeMap<String, String>,
    pub checker_policy: FrozenCorpusCheckerPolicyV1,
    pub clarified_expected_readiness: ExpectedReadinessV1,
    #[serde(default)]
    pub frozen_clarification: Option<String>,
    pub initial_expected_readiness: ExpectedReadinessV1,
    #[serde(default)]
    pub intended_delta: Vec<String>,
    pub mandatory_obligations: Vec<FrozenCorpusObligationV1>,
    #[serde(default)]
    pub material_unknown: Option<String>,
    #[serde(default)]
    pub sealed_cli_vectors: Vec<FrozenCorpusVectorV1>,
    pub selected_binary: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FrozenCorpusFileV1 {
    pub schema_version: u16,
    pub owner: String,
    pub purpose: String,
    pub arm_order: Vec<ExperimentArmV1>,
    pub caps: FrozenCorpusCapsV1,
    pub cases: Vec<FrozenCorpusCaseV1>,
    pub diagnostic_case_ids: Vec<String>,
    pub diagnostic_cases: Vec<FrozenCorpusCaseV1>,
}

#[cfg(test)]
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ControlledArmScriptV1 {
    #[serde(default)]
    pub raw_prep_replies: Vec<String>,
    #[serde(default)]
    pub clarified_prep_replies: Vec<String>,
    #[serde(default)]
    pub coding_replies: Vec<String>,
}

#[cfg(test)]
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ControlledExperimentFixtureV1 {
    #[serde(default)]
    pub diagnostic_scripts: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub arm_scripts: BTreeMap<String, ControlledArmScriptV1>,
}

pub(crate) fn arm_slug(arm: ExperimentArmV1) -> &'static str {
    match arm {
        ExperimentArmV1::ProsePlain => "prose_plain",
        ExperimentArmV1::ProseArchitecture => "prose_architecture",
        ExperimentArmV1::TypedPlain => "typed_plain",
        ExperimentArmV1::TypedArchitecture => "typed_architecture",
    }
}

pub(crate) enum ExperimentTransportConfig {
    Native {
        coding_profile_path: PathBuf,
        worker_path: PathBuf,
        artifact_path: PathBuf,
    },
    #[cfg(test)]
    Controlled {
        fixture: ControlledExperimentFixtureV1,
        evidence_origin: EvidenceOriginV1,
        custom_runner_factory: Option<Box<dyn FnMut() -> CustomRunnerFactory>>,
    },
}

pub(crate) struct ExperimentRequest {
    pub campaign_id: String,
    pub corpus_path: PathBuf,
    pub corpus_freeze_path: PathBuf,
    pub output_dir: PathBuf,
    pub catalog: ToolCatalog,
    pub annotations_path: Option<PathBuf>,
    pub transport: ExperimentTransportConfig,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DiagnosticAttemptResultV1 {
    pub case_id: BoundedText<128>,
    pub slice: CorpusSliceV1,
    pub accepted: bool,
    pub model_requests_used: u64,
    pub tool_calls_used: u64,
    pub outcome: BoundedText<64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DiagnosticGateReportV1 {
    pub schema_version: u16,
    pub passed: bool,
    pub new_accepted: u32,
    pub existing_accepted: u32,
    pub results: Vec<DiagnosticAttemptResultV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExperimentAttemptResultV1 {
    pub case_id: BoundedText<128>,
    pub slice: CorpusSliceV1,
    pub arm: ExperimentArmV1,
    pub raw_output_digest: Digest,
    #[serde(default)]
    pub clarified_output_digest: Option<Digest>,
    pub raw_declared_ready: bool,
    pub raw_readiness_matched: bool,
    pub prep_unknown_hold: bool,
    pub prep_quality_failure: bool,
    pub prep_requests_admitted: u32,
    pub coding_model_requests_used: u32,
    pub total_admitted_tool_effects: u32,
    pub accepted: bool,
    pub authority_fault: bool,
    pub oracle_leak: bool,
    pub bundle_digest: Option<Digest>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExperimentReportV1 {
    pub schema_version: u16,
    pub campaign_id: BoundedText<128>,
    pub corpus_sha256: Digest,
    pub config_digest: Digest,
    pub evidence_origin: EvidenceOriginV1,
    pub diagnostic_gate: DiagnosticGateReportV1,
    pub attempts: Vec<ExperimentAttemptResultV1>,
    pub promotion_receipt: ContractPromotionReceiptV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExperimentCampaignConfigV1 {
    pub schema_version: u16,
    pub campaign_id: BoundedText<128>,
    pub corpus_sha256: Digest,
    pub corpus_freeze_sha256: Digest,
    pub host_executable_sha256: Digest,
    pub checker_impl_digest: Digest,
    pub catalog_digest: Digest,
    pub public_harness_preamble: BoundedText<256>,
    pub objective_projection_recipe: BoundedText<64>,
    pub arm_order: Vec<ExperimentArmV1>,
    pub caps: FrozenCorpusCapsV1,
    pub evidence_origin: EvidenceOriginV1,
    #[serde(default)]
    pub coding_profile_digest: Option<Digest>,
    #[serde(default)]
    pub team_profile_digest: Option<Digest>,
    #[serde(default)]
    pub worker_sha256: Option<Digest>,
    #[serde(default)]
    pub artifact_sha256: Option<Digest>,
    pub config_digest: Digest,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProsePrepStageRecordV1 {
    pub stage_index: u8,
    pub stage: BoundedText<32>,
    pub outcome: BoundedText<32>,
    #[serde(default)]
    pub raw_output: Option<BoundedUtf8<MAX_INTERNAL>>,
    #[serde(default)]
    pub raw_output_sha256: Option<Digest>,
    #[serde(default)]
    pub protocol_valid: Option<bool>,
    #[serde(default)]
    pub protocol_error: Option<BoundedText<256>>,
    #[serde(default)]
    pub diagnostic: Option<BoundedText<256>>,
    #[serde(default)]
    pub declared_ready: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProsePrepLedgerV1 {
    pub schema_version: u16,
    pub case_id: BoundedText<128>,
    pub evidence_origin: EvidenceOriginV1,
    #[serde(default)]
    pub source_coding_profile_digest: Option<Digest>,
    #[serde(default)]
    pub team_profile_digest: Option<Digest>,
    pub max_prep_requests: u8,
    pub admitted_prep_requests: u32,
    pub raw_output_sha256: Digest,
    pub declared_ready: bool,
    pub unknown_hold: bool,
    pub quality_failure: bool,
    pub stages: Vec<ProsePrepStageRecordV1>,
}

fn validate_case_item(case: &FrozenCorpusCaseV1, caps: &FrozenCorpusCapsV1) -> Result<(), String> {
    if content_digest(case.raw_request.as_bytes()) != case.raw_request_sha256 {
        return Err(format!(
            "corpus case '{}' raw_request_sha256 mismatch",
            case.id
        ));
    }
    if case.baseline_files.len() > caps.max_files {
        return Err(format!(
            "corpus case '{}' exceeds max_files {}",
            case.id, caps.max_files
        ));
    }
    if case.sealed_cli_vectors.len() > caps.max_cli_vectors {
        return Err(format!(
            "corpus case '{}' exceeds max_cli_vectors {}",
            case.id, caps.max_cli_vectors
        ));
    }
    if case.checker_policy.metadata != CHECKER_METADATA_SNAPSHOT_LOCAL_PATH_V1
        || case.checker_policy.build != CHECKER_BUILD_CARGO_OFFLINE_LOCKED_V1
        || case.checker_policy.test != CHECKER_TEST_CARGO_TEST_OFFLINE_LOCKED_V1
        || case.checker_policy.cli != CHECKER_CLI_SELECTED_VECTORS_V1
    {
        return Err(format!(
            "corpus case '{}' uses unsupported checker_policy slot",
            case.id
        ));
    }
    Ok(())
}

pub(crate) fn load_and_verify_corpus_bytes(
    corpus_bytes: &[u8],
    freeze_bytes: &[u8],
) -> Result<(FrozenCorpusFileV1, FrozenCorpusFreezeV1, Digest), String> {
    let corpus_sha = content_digest(corpus_bytes);
    if corpus_sha.as_str() != EXPECTED_INDEPENDENT_CORPUS_SHA256 {
        return Err(format!(
            "independent corpus SHA-256 mismatch: expected {EXPECTED_INDEPENDENT_CORPUS_SHA256}, got {}",
            corpus_sha.as_str()
        ));
    }
    let freeze_sha = content_digest(freeze_bytes);
    if freeze_sha.as_str() != EXPECTED_INDEPENDENT_CORPUS_FREEZE_SHA256 {
        return Err(format!(
            "corpus.freeze.json SHA-256 mismatch: expected {EXPECTED_INDEPENDENT_CORPUS_FREEZE_SHA256}, got {}",
            freeze_sha.as_str()
        ));
    }
    let freeze: FrozenCorpusFreezeV1 =
        parse_internal(freeze_bytes).map_err(|e| format!("parse corpus.freeze.json: {e:?}"))?;
    if freeze.schema_version != 2 {
        return Err(format!(
            "unsupported corpus.freeze.json schema_version {} (expected 2)",
            freeze.schema_version
        ));
    }
    if freeze.native_outputs_seen_before_freeze {
        return Err("corpus.freeze.json invalid: native_outputs_seen_before_freeze is true".into());
    }
    if freeze.sha256 != corpus_sha {
        return Err("corpus.freeze.json sha256 does not match independent corpus bytes".into());
    }

    let corpus: FrozenCorpusFileV1 = parse_internal(corpus_bytes)
        .map_err(|e| format!("parse independent-corpus.json: {e:?}"))?;
    if corpus.schema_version != 2 {
        return Err(format!(
            "unsupported independent-corpus schema_version {} (expected 2)",
            corpus.schema_version
        ));
    }
    if corpus.caps.preparation_requests != MAX_PREP_REQUESTS
        || corpus.caps.total_model_requests != MAX_PILOT_TOTAL_MODEL_REQUESTS
        || corpus.caps.admitted_tool_effects != MAX_PILOT_ADMITTED_TOOL_EFFECTS
        || corpus.caps.max_cli_vectors != MAX_CLI_VECTORS
        || corpus.caps.max_files != MAX_BASELINE_FILES
        || corpus.caps.max_local_packages != MAX_LOCAL_RUST_PACKAGES
    {
        return Err("corpus caps mismatch frozen contract parameters".into());
    }
    if corpus.arm_order.len() != 4 || corpus.arm_order.iter().collect::<BTreeSet<_>>().len() != 4 {
        return Err("corpus arm_order must contain all 4 distinct experiment arms".into());
    }
    if corpus.cases.len() != freeze.case_count as usize || corpus.cases.len() != 12 {
        return Err(format!(
            "corpus expected 12 pilot cases matching freeze, got {}",
            corpus.cases.len()
        ));
    }
    if corpus.diagnostic_cases.len() != freeze.separate_diagnostic_count as usize
        || corpus.diagnostic_cases.len() != 4
        || corpus.diagnostic_case_ids.len() != 4
    {
        return Err("corpus expected 4 separate diagnostic cases".into());
    }
    if corpus.cases.len().saturating_mul(corpus.arm_order.len())
        != freeze.pilot_attempt_count as usize
    {
        return Err("corpus pilot_attempt_count mismatch".into());
    }

    let pilot_new = corpus
        .cases
        .iter()
        .filter(|c| c.workflow == ContractWorkflow::New)
        .count();
    let pilot_existing = corpus
        .cases
        .iter()
        .filter(|c| c.workflow == ContractWorkflow::Existing)
        .count();
    if pilot_new != 6 || pilot_existing != 6 {
        return Err(format!(
            "corpus pilot cases must split 6 new / 6 existing (got {pilot_new}/{pilot_existing})"
        ));
    }
    let diag_new = corpus
        .diagnostic_cases
        .iter()
        .filter(|c| c.workflow == ContractWorkflow::New)
        .count();
    let diag_existing = corpus
        .diagnostic_cases
        .iter()
        .filter(|c| c.workflow == ContractWorkflow::Existing)
        .count();
    if diag_new != 2 || diag_existing != 2 {
        return Err(format!(
            "corpus diagnostic cases must split 2 new / 2 existing (got {diag_new}/{diag_existing})"
        ));
    }

    let mut pilot_ids = BTreeSet::new();
    for c in &corpus.cases {
        if !pilot_ids.insert(c.id.clone()) {
            return Err(format!("duplicate pilot case id '{}'", c.id));
        }
        validate_case_item(c, &corpus.caps)?;
    }
    let mut diag_ids = BTreeSet::new();
    for (idx, c) in corpus.diagnostic_cases.iter().enumerate() {
        if corpus.diagnostic_case_ids[idx] != c.id {
            return Err("diagnostic_case_ids order/ids mismatch diagnostic_cases".into());
        }
        if !diag_ids.insert(c.id.clone()) {
            return Err(format!("duplicate diagnostic case id '{}'", c.id));
        }
        if pilot_ids.contains(&c.id) {
            return Err(format!(
                "diagnostic case '{}' is not disjoint from pilot cases",
                c.id
            ));
        }
        validate_case_item(c, &corpus.caps)?;
    }

    Ok((corpus, freeze, corpus_sha))
}

fn minimal_rust_scaffold_files(binary_name: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    map.insert(
        "Cargo.lock".into(),
        format!(
            "# This file is automatically @generated by Cargo.\nversion = 4\n\n[[package]]\nname = \"{binary_name}\"\nversion = \"0.1.0\"\n"
        ),
    );
    map.insert(
        "Cargo.toml".into(),
        format!("[package]\nname = \"{binary_name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n"),
    );
    map.insert("src/main.rs".into(), "fn main() {}\n".into());
    map
}

fn build_workspace_for_attempt(
    ws_path: &Path,
    forbidden: &[(&File, &str)],
    case: &FrozenCorpusCaseV1,
    architecture_scaffold: bool,
) -> Result<(File, PathBuf, BTreeMap<String, Digest>), String> {
    let files = if case.baseline_files.is_empty() && architecture_scaffold {
        minimal_rust_scaffold_files(&case.selected_binary)
    } else {
        case.baseline_files.clone()
    };
    let (_ws_fd, ws_abs) = seed_candidate_workspace(ws_path, forbidden, &files)?;
    super::io::collect_workspace_snapshot(&ws_abs).map(|(fd, snap)| (fd, ws_abs, snap))
}

fn build_host_auth_from_case(
    case: &FrozenCorpusCaseV1,
    clarification: Option<&str>,
    workspace_snapshot: BTreeMap<String, Digest>,
) -> HostAuthContext {
    let mut sources_by_id = BTreeMap::new();
    sources_by_id.insert(
        "src-request".to_string(),
        HostAuthSource {
            kind: SourceKind::OperatorRequest,
            sha256: case.raw_request_sha256.clone(),
            label: format!("corpus-case:{}", case.id),
            utf8_text: Some(case.raw_request.clone()),
        },
    );
    if let Some(clar_text) = clarification {
        sources_by_id.insert(
            "src-clarification".to_string(),
            HostAuthSource {
                kind: SourceKind::ClarificationAnswer,
                sha256: content_digest(clar_text.as_bytes()),
                label: format!("corpus-clarification:{}", case.id),
                utf8_text: Some(clar_text.to_string()),
            },
        );
    }
    for (rel, sha) in &workspace_snapshot {
        sources_by_id.insert(
            format!("src-file:{rel}"),
            HostAuthSource {
                kind: SourceKind::RepositoryFile,
                sha256: sha.clone(),
                label: rel.clone(),
                utf8_text: None,
            },
        );
    }
    HostAuthContext {
        raw_request: case.raw_request.clone(),
        raw_request_sha256: case.raw_request_sha256.clone(),
        sources_by_id,
        workspace_snapshot: Some(workspace_snapshot),
        predecessor_bundle_digest: None,
        predecessor_inspection: None,
    }
}

fn convert_sealed_vectors(
    vectors: &[FrozenCorpusVectorV1],
) -> Result<Vec<SealedCliVectorV1>, String> {
    let mut out = Vec::with_capacity(vectors.len());
    for v in vectors {
        let mut args = Vec::with_capacity(v.args.len());
        for a in &v.args {
            args.push(BoundedUtf8::<256>::new(a.clone())?);
        }
        out.push(SealedCliVectorV1 {
            args,
            expected_exit_code: v.exit_code,
            expected_stdout: BoundedUtf8::<MAX_TEXT>::new(v.stdout.clone())?,
        });
    }
    Ok(out)
}

/// Host-only sealing of a candidate-prepared `ContractDraftV1` with frozen oracle
/// `sealed_cli_vectors`, returning the validated execution `ContractBundleV1`.
/// Oracle vectors never enter model preparation prompts or candidate workspaces.
pub(crate) fn seal_draft_with_corpus_vectors(
    draft: &ContractDraftV1,
    auth: &HostAuthContext,
    sealed_vectors: &[FrozenCorpusVectorV1],
) -> Result<(ContractValidationReportV1, ContractBundleV1), String> {
    let mut sealed = draft.clone();
    if !sealed_vectors.is_empty() {
        sealed.checker_policy.cli_checker =
            Some(BoundedText::new(CHECKER_CLI_SELECTED_VECTORS_V1).map_err(|e| format!("{e:?}"))?);
        sealed.checker_policy.cli_vectors = convert_sealed_vectors(sealed_vectors)?;
    }
    let (report, bundle_opt) = validate_against_auth(&sealed, Some(auth))?;
    let bundle = bundle_opt.ok_or_else(|| {
        let errs: Vec<String> = report
            .findings
            .iter()
            .map(|f| format!("{}: {}", f.code.as_str(), f.message.as_str()))
            .collect();
        format!(
            "contract failed seal_draft_with_corpus_vectors: {}",
            errs.join("; ")
        )
    })?;
    Ok((report, bundle))
}

fn build_lossless_obligations(
    raw_request: &str,
    clarification: Option<&str>,
    checker: &str,
    clar_source_id: &str,
) -> Result<Vec<ContractObligationV1>, String> {
    let raw_clauses = split_text_lossless_clauses(raw_request, 256, MAX_REQUIREMENTS)?;
    let clar_clauses = match clarification {
        Some(c) if !c.trim().is_empty() => {
            split_text_lossless_clauses(c.trim(), 256, MAX_REQUIREMENTS)?
        }
        _ => Vec::new(),
    };
    if raw_clauses.len() + clar_clauses.len() > MAX_REQUIREMENTS {
        return Err(format!(
            "lossless source obligations exceed {MAX_REQUIREMENTS} clauses ({})",
            raw_clauses.len() + clar_clauses.len()
        ));
    }
    if raw_clauses.is_empty() {
        return Err("source raw_request produced zero obligation clauses".into());
    }
    let mut obligations = Vec::with_capacity(raw_clauses.len() + clar_clauses.len());
    for (idx, clause) in raw_clauses.into_iter().enumerate() {
        obligations.push(ContractObligationV1 {
            id: BoundedText::new(format!("src-r{}", idx + 1)).map_err(|e| format!("{e:?}"))?,
            clause: BoundedText::new(clause).map_err(|e| format!("{e:?}"))?,
            checker: BoundedText::new(checker).map_err(|e| format!("{e:?}"))?,
            source_ref: BoundedText::new("src-request").map_err(|e| format!("{e:?}"))?,
            approved_exception: false,
        });
    }
    for (idx, clause) in clar_clauses.into_iter().enumerate() {
        obligations.push(ContractObligationV1 {
            id: BoundedText::new(format!("clar-r{}", idx + 1)).map_err(|e| format!("{e:?}"))?,
            clause: BoundedText::new(clause).map_err(|e| format!("{e:?}"))?,
            checker: BoundedText::new(checker).map_err(|e| format!("{e:?}"))?,
            source_ref: BoundedText::new(clar_source_id).map_err(|e| format!("{e:?}"))?,
            approved_exception: false,
        });
    }
    Ok(obligations)
}

fn build_diagnostic_draft(
    case: &FrozenCorpusCaseV1,
    auth: &HostAuthContext,
) -> Result<ContractDraftV1, String> {
    let sources: Vec<ContractSourceEntryV1> = auth
        .sources_by_id
        .iter()
        .map(|(id, src)| {
            Ok(ContractSourceEntryV1 {
                id: BoundedText::new(id.clone()).map_err(|e| format!("{e:?}"))?,
                claimed_kind: src.kind,
                host_kind: Some(src.kind),
                label: BoundedText::new(
                    &src.label[..src.label.floor_char_boundary(src.label.len().min(256))],
                )
                .map_err(|e| format!("{e:?}"))?,
                content_sha256: src.sha256.clone(),
                host_authenticated: true,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let baseline_files: Vec<BaselineFileRefV1> = auth
        .workspace_snapshot
        .as_ref()
        .into_iter()
        .flatten()
        .map(|(p, sha)| {
            Ok(BaselineFileRefV1 {
                path: WorkspacePath::new(p).map_err(|e| format!("{e:?}"))?,
                sha256: sha.clone(),
                preserved: false,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let obligation_checker = if case.sealed_cli_vectors.is_empty() {
        CHECKER_TEST_CARGO_TEST_OFFLINE_LOCKED_V1
    } else {
        case.checker_policy.cli.as_str()
    };
    let obligations = build_lossless_obligations(
        &case.raw_request,
        case.frozen_clarification.as_deref(),
        obligation_checker,
        "src-clarification",
    )?;

    let approved_compatibility = case
        .approved_compatibility
        .iter()
        .map(|s| BoundedText::new(s.clone()).map_err(|e| format!("{e:?}")))
        .collect::<Result<Vec<_>, String>>()?;
    let intended_delta = case
        .intended_delta
        .iter()
        .map(|s| BoundedText::new(s.clone()).map_err(|e| format!("{e:?}")))
        .collect::<Result<Vec<_>, String>>()?;

    Ok(ContractDraftV1 {
        schema_version: CONTRACT_SCHEMA_VERSION,
        contract_id: BoundedText::new(format!("diag-{}", case.id)).map_err(|e| format!("{e:?}"))?,
        workflow: case.workflow,
        raw_request: BoundedText::new(case.raw_request.clone()).map_err(|e| format!("{e:?}"))?,
        raw_request_sha256: case.raw_request_sha256.clone(),
        sources,
        baseline: ContractBaselineV1 {
            workspace_label: Some(
                BoundedText::new(if case.workflow == ContractWorkflow::Existing {
                    "observed-workspace"
                } else {
                    "empty-new"
                })
                .map_err(|e| format!("{e:?}"))?,
            ),
            files: baseline_files,
            local_rust_package_count: MAX_LOCAL_RUST_PACKAGES as u8,
            selected_cli_target_count: MAX_SELECTED_CLI_TARGETS as u8,
            selected_cli_binary: BoundedText::new(case.selected_binary.clone())
                .map_err(|e| format!("{e:?}"))?,
        },
        compatibility: ContractCompatibilityV1 {
            approved_compatibility,
            intended_delta,
            preserved_files: Vec::new(),
            allowed_edit_paths: Vec::new(),
            forbidden_files: Vec::new(),
        },
        requirements: ContractRequirementsV1 {
            mandatory_obligations: obligations,
            assumptions: Vec::new(),
            resolutions: Vec::new(),
            unknowns: Vec::new(),
        },
        checker_policy: ContractCheckerPolicyV1 {
            metadata_checker: BoundedText::new(case.checker_policy.metadata.clone())
                .map_err(|e| format!("{e:?}"))?,
            build_checker: BoundedText::new(case.checker_policy.build.clone())
                .map_err(|e| format!("{e:?}"))?,
            test_checker: BoundedText::new(case.checker_policy.test.clone())
                .map_err(|e| format!("{e:?}"))?,
            cli_checker: Some(
                BoundedText::new(case.checker_policy.cli.clone()).map_err(|e| format!("{e:?}"))?,
            ),
            max_local_packages: MAX_LOCAL_RUST_PACKAGES as u8,
            fixed_files: Vec::new(),
            interface_checks: Vec::new(),
            cli_vectors: convert_sealed_vectors(&case.sealed_cli_vectors)?,
        },
        lineage: None,
        untrusted_claims: None,
    })
}

fn build_flat_prose_draft(
    contract_id: &str,
    case: &FrozenCorpusCaseV1,
    auth: &HostAuthContext,
    clarification: Option<&str>,
    prepared_summary: Option<&str>,
) -> Result<ContractDraftV1, String> {
    let sources: Vec<ContractSourceEntryV1> = auth
        .sources_by_id
        .iter()
        .map(|(id, src)| {
            Ok(ContractSourceEntryV1 {
                id: BoundedText::new(id.clone()).map_err(|e| format!("{e:?}"))?,
                claimed_kind: src.kind,
                host_kind: Some(src.kind),
                label: BoundedText::new(
                    &src.label[..src.label.floor_char_boundary(src.label.len().min(256))],
                )
                .map_err(|e| format!("{e:?}"))?,
                content_sha256: src.sha256.clone(),
                host_authenticated: true,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let baseline_files: Vec<BaselineFileRefV1> = auth
        .workspace_snapshot
        .as_ref()
        .into_iter()
        .flatten()
        .map(|(p, sha)| {
            Ok(BaselineFileRefV1 {
                path: WorkspacePath::new(p).map_err(|e| format!("{e:?}"))?,
                sha256: sha.clone(),
                preserved: false,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let obligations = build_lossless_obligations(
        &case.raw_request,
        clarification,
        CHECKER_TEST_CARGO_TEST_OFFLINE_LOCKED_V1,
        "src-clarification",
    )?;
    let intended_delta = if case.workflow == ContractWorkflow::Existing {
        vec![obligations[0].clause.clone()]
    } else {
        Vec::new()
    };
    let mut assumptions = Vec::new();
    if let Some(summary) = prepared_summary
        && !summary.trim().is_empty()
    {
        let trimmed = summary.trim();
        if trimmed.len() > 32 {
            return Err("prepared guidance exceeds 32 UTF-8 bytes".into());
        }
        let cut = trimmed;
        assumptions.push(ContractAssumptionV1 {
            id: BoundedText::new("prepared-guidance").map_err(|e| format!("{e:?}"))?,
            text: BoundedText::new(cut).map_err(|e| format!("{e:?}"))?,
            material: false,
            source_ref: BoundedText::new(if clarification.is_some() {
                "src-clarification"
            } else {
                "src-request"
            })
            .map_err(|e| format!("{e:?}"))?,
            approved: false,
        });
    }

    Ok(ContractDraftV1 {
        schema_version: CONTRACT_SCHEMA_VERSION,
        contract_id: BoundedText::new(contract_id).map_err(|e| format!("{e:?}"))?,
        workflow: case.workflow,
        raw_request: BoundedText::new(case.raw_request.clone()).map_err(|e| format!("{e:?}"))?,
        raw_request_sha256: case.raw_request_sha256.clone(),
        sources,
        baseline: ContractBaselineV1 {
            workspace_label: Some(
                BoundedText::new(if case.workflow == ContractWorkflow::Existing {
                    "observed-workspace"
                } else {
                    "empty-new"
                })
                .map_err(|e| format!("{e:?}"))?,
            ),
            files: baseline_files,
            local_rust_package_count: MAX_LOCAL_RUST_PACKAGES as u8,
            selected_cli_target_count: MAX_SELECTED_CLI_TARGETS as u8,
            selected_cli_binary: BoundedText::new(case.selected_binary.clone())
                .map_err(|e| format!("{e:?}"))?,
        },
        compatibility: ContractCompatibilityV1 {
            approved_compatibility: Vec::new(),
            intended_delta,
            preserved_files: Vec::new(),
            allowed_edit_paths: Vec::new(),
            forbidden_files: Vec::new(),
        },
        requirements: ContractRequirementsV1 {
            mandatory_obligations: obligations,
            assumptions,
            resolutions: Vec::new(),
            unknowns: Vec::new(),
        },
        checker_policy: ContractCheckerPolicyV1 {
            metadata_checker: BoundedText::new(CHECKER_METADATA_SNAPSHOT_LOCAL_PATH_V1)
                .map_err(|e| format!("{e:?}"))?,
            build_checker: BoundedText::new(CHECKER_BUILD_CARGO_OFFLINE_LOCKED_V1)
                .map_err(|e| format!("{e:?}"))?,
            test_checker: BoundedText::new(CHECKER_TEST_CARGO_TEST_OFFLINE_LOCKED_V1)
                .map_err(|e| format!("{e:?}"))?,
            cli_checker: None,
            max_local_packages: MAX_LOCAL_RUST_PACKAGES as u8,
            fixed_files: Vec::new(),
            interface_checks: Vec::new(),
            cli_vectors: Vec::new(),
        },
        lineage: None,
        untrusted_claims: None,
    })
}

struct ProsePrepOutcome {
    raw_output_digest: Digest,
    prepared_summary: Option<String>,
    raw_responses: Vec<String>,
    declared_ready: bool,
    unknown_hold: bool,
    quality_failure: bool,
    admitted_requests: u32,
}

fn raw_preparation_cap(case: &FrozenCorpusCaseV1, cap: u8) -> u8 {
    if case.frozen_clarification.is_some() {
        cap.min(1)
    } else {
        cap
    }
}

fn parse_prose_readiness_protocol(raw: &str) -> Result<Result<String, String>, String> {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("READY:") {
        let body = rest.trim();
        if body.len() > 32 {
            return Err(
                "READY: response must include optional guidance of 0..=32 UTF-8 bytes".into(),
            );
        }
        return Ok(Ok(body.to_string()));
    }
    if let Some(rest) = trimmed.strip_prefix("BLOCKED:") {
        let body = rest.trim();
        if body.is_empty() {
            return Err("BLOCKED: response must state missing detail".into());
        }
        return Ok(Err(body.to_string()));
    }
    Err("invalid prose readiness protocol: response must start with `READY:` or `BLOCKED:`".into())
}

/// Execute bounded prose readiness preparation (`Proposal`, `Revision`, `Review` within shared
/// `max_stages <= 3` cap) using `ComparisonSession` (`LOCAL_TEAM_RECIPE_SHA`), enforcing strict
/// `READY:`/`BLOCKED:` protocol and matching typed stage/retry parity without silent truncation.
#[allow(clippy::too_many_arguments)]
fn run_prose_prep(
    prep_dir: &Path,
    workspace_fd: Option<&File>,
    forbidden: &[(&File, &str)],
    case: &FrozenCorpusCaseV1,
    auth: &HostAuthContext,
    architecture_scaffold: bool,
    max_stages: u8,
    evidence_origin: EvidenceOriginV1,
    transport: &ExperimentTransportConfig,
) -> Result<ProsePrepOutcome, String> {
    if !(1..=MAX_PREP_REQUESTS).contains(&max_stages) {
        return Err(format!(
            "prose prep max_stages must be 1..={MAX_PREP_REQUESTS}, got {max_stages}"
        ));
    }
    let (_dir_fd, dir_abs) = create_external_private_dir(prep_dir, workspace_fd, forbidden)?;
    let harness_preamble = if case.selected_binary == "metron-case" {
        std::borrow::Cow::Borrowed(PUBLIC_HARNESS_PREAMBLE)
    } else {
        std::borrow::Cow::Owned(format!(
            "Final checks use offline+locked Cargo metadata/test/build. Prepare Cargo.lock with normal development tools. Selected CLI: {}.",
            case.selected_binary
        ))
    };
    let arch_note = if architecture_scaffold {
        "architecture_scaffold=on: identify workspace modules, cargo dependencies, and implementation plan."
    } else {
        "architecture_scaffold=off: summarize exact behavioral obligations and unresolved ambiguities faithfully."
    };

    enum ProseActiveDriver {
        Native {
            session: Box<ComparisonSession>,
            driver: Box<LocalInferenceDriver>,
        },
        #[cfg(test)]
        Controlled { replies: VecDeque<String> },
    }

    let (mut active, source_coding_profile_digest, team_profile_digest) = match transport {
        ExperimentTransportConfig::Native {
            coding_profile_path,
            worker_path,
            artifact_path,
        } => {
            let (_, coding_profile, _) =
                read_external_json::<LocalProfile>(coding_profile_path, workspace_fd)?;
            let (team_profile, source_prof_dig, team_prof_dig) =
                derive_team_text_profile(&coding_profile)?;
            let (_, worker_abs) = open_external_nofollow(worker_path, false)?;
            let (_, artifact_abs) = open_external_nofollow(artifact_path, false)?;
            verify_artifact(&artifact_abs, resolve_pinned_artifact(&team_profile)?)
                .map_err(|e| format!("verify native artifact: {e}"))?;

            let session_digest = structural_digest(
                "prose-prep-session/v2",
                &[case.id.as_bytes(), dir_abs.to_string_lossy().as_bytes()],
            )
            .map_err(|e| format!("{e:?}"))?;
            let task_id = TaskId::new(format!("prose-{}", &session_digest.as_str()[..16]))
                .map_err(|e| format!("{e:?}"))?;
            let principal =
                PrincipalId::new("contract-prose-prep").map_err(|e| format!("{e:?}"))?;

            write_external_new_json(
                &dir_abs.join("native-preparation.json"),
                workspace_fd,
                forbidden,
                &NativePreparationInputs {
                    profile: team_profile.clone(),
                    session_id: session_digest.clone(),
                    task_id: task_id.clone(),
                    principal_id: principal.clone(),
                    max_requests: u32::from(max_stages),
                },
            )?;
            let session = ComparisonSession::create(
                &dir_abs,
                session_digest,
                task_id,
                principal,
                team_profile.clone(),
                u32::from(max_stages),
            )
            .map_err(|e| format!("ComparisonSession::create prose: {e}"))?;
            let driver = LocalInferenceDriver::new(worker_abs, artifact_abs, team_profile, None)
                .map_err(|e| format!("LocalInferenceDriver::new prose: {e}"))?;
            (
                ProseActiveDriver::Native {
                    session: Box::new(session),
                    driver: Box::new(driver),
                },
                Some(source_prof_dig),
                Some(team_prof_dig),
            )
        }
        #[cfg(test)]
        ExperimentTransportConfig::Controlled { fixture, .. } => {
            let key_clar = auth.sources_by_id.contains_key("src-clarification");
            let arm_name = if architecture_scaffold {
                "prose_architecture"
            } else {
                "prose_plain"
            };
            let lookup_key = format!("{}::{arm_name}", case.id);
            let replies = fixture
                .arm_scripts
                .get(&lookup_key)
                .map(|s| {
                    if key_clar {
                        s.clarified_prep_replies.clone()
                    } else {
                        s.raw_prep_replies.clone()
                    }
                })
                .unwrap_or_default();
            (
                ProseActiveDriver::Controlled {
                    replies: VecDeque::from(replies),
                },
                None,
                None,
            )
        }
    };

    let mut admitted: u32 = 0;
    let mut unknown_hold = false;
    let mut quality_failure = false;
    let mut declared_ready = false;
    let mut prepared_summary: Option<String> = None;
    let mut last_raw: Option<String> = None;
    let mut last_diagnostics: Vec<String> = Vec::new();
    let mut raw_responses = Vec::new();
    let mut raw_digest = content_digest(b"no-prose-response");
    let mut ledger_stages: Vec<ProsePrepStageRecordV1> = Vec::new();

    for idx in 0..max_stages {
        if admitted >= u32::from(max_stages) || admitted >= u32::from(MAX_PREP_REQUESTS) {
            break;
        }
        let stage_name = match idx {
            0 => "proposal",
            1 => "revision",
            _ => "review",
        };
        let mut prompt = format!(
            "Role: Metron prose readiness preparation ({arch_note}).\n\
             Public harness environment: {harness_preamble}\n\
             Respond in plain text starting with `READY:` followed by implementation optional guidance of 0..=32 UTF-8 bytes, or `BLOCKED:` followed by the missing or ambiguous detail. Declare every material ambiguity and never invent requirements. Guidance does not replace the complete original request or approved clarifications.\n\
             Request:\n{}\n",
            auth.raw_request.trim()
        );
        if let Some(ws) = &auth.workspace_snapshot
            && !ws.is_empty()
        {
            prompt.push_str("Observed baseline files (path:sha256_prefix):\n");
            for (p, dig) in ws {
                prompt.push_str(&format!("- {p}: {}\n", &dig.as_str()[..12]));
            }
        }
        for (id, src) in &auth.sources_by_id {
            if matches!(
                src.kind,
                SourceKind::ClarificationAnswer | SourceKind::TrustedSpec
            ) && let Some(text) = &src.utf8_text
            {
                prompt.push_str(&format!(
                    "Host-approved {} ({id}):\n{}\n",
                    match src.kind {
                        SourceKind::ClarificationAnswer => "clarification",
                        _ => "trusted_spec",
                    },
                    text.trim()
                ));
            }
        }
        if idx >= 1 {
            prompt.push_str(&format!(
                "Stage: {}. Fix protocol or completeness errors below.\n",
                stage_name.to_ascii_uppercase()
            ));
            if let Some(prev) = &last_raw {
                let cap = prev.floor_char_boundary(prev.len().min(600));
                prompt.push_str(&format!("Previous response snippet:\n{}\n", &prev[..cap]));
            }
            for diag in last_diagnostics.iter().take(4) {
                prompt.push_str(&format!("- {diag}\n"));
            }
        }
        if prompt.len() > MAX_LOCAL_CONTEXT {
            return Err(format!(
                "prose prep prompt length {} exceeds MAX_LOCAL_CONTEXT ({MAX_LOCAL_CONTEXT}) without truncation",
                prompt.len()
            ));
        }
        let bounded_prompt =
            BoundedText::<MAX_LOCAL_CONTEXT>::new(prompt).map_err(|e| format!("{e:?}"))?;
        let prompt_sha = content_digest(bounded_prompt.as_str().as_bytes());
        let inflight_path = dir_abs.join("propose-in-flight.json");
        write_external_new_json(
            &inflight_path,
            workspace_fd,
            forbidden,
            &serde_json::json!({
                "schema_version": CONTRACT_SCHEMA_VERSION,
                "case_id": case.id,
                "stage_index": idx,
                "stage": stage_name,
                "prompt_sha256": prompt_sha,
            }),
        )?;

        admitted = admitted.saturating_add(1);
        let dir_handle = File::open(&dir_abs).map_err(|e| format!("{e}"))?;

        enum ProseStageOutcome {
            Settled(String),
            Rejected(String),
            Uncertain,
        }

        let stage_out = match &mut active {
            ProseActiveDriver::Native { session, driver } => {
                let cmp_case = ComparisonCase {
                    id: BoundedText::new(format!("prose-prep-{idx}"))
                        .map_err(|e| format!("{e:?}"))?,
                    role: LocalRole::Reasoner,
                    output_schema: LocalOutputSchema::Text,
                    context: bounded_prompt,
                };
                write_external_new_json(
                    &dir_abs.join(format!("stage-{idx}-input.json")),
                    workspace_fd,
                    forbidden,
                    &NativePreparationCase {
                        id: cmp_case.id.clone(),
                        context: cmp_case.context.clone(),
                    },
                )?;
                let permits = session
                    .admit(&[cmp_case])
                    .map_err(|e| format!("prose admit stage {idx}: {e}"))?;
                let dispatch_id = permits[0].request().dispatch_id.clone();
                let mut outcomes = driver.execute_comparison_cohort(permits, Duration::ZERO);
                let outcome = outcomes
                    .pop()
                    .ok_or_else(|| "empty prose outcome".to_string())?;
                let results = match outcome {
                    QualificationOutcome::Observed(obs) => vec![ComparisonResult {
                        dispatch_id,
                        observation: Some(*obs),
                        rejection: None,
                    }],
                    QualificationOutcome::Rejected(diag) => vec![ComparisonResult {
                        dispatch_id,
                        observation: None,
                        rejection: Some(diag),
                    }],
                    QualificationOutcome::Uncertain => vec![ComparisonResult {
                        dispatch_id,
                        observation: None,
                        rejection: None,
                    }],
                };
                let _ = session
                    .settle(&results)
                    .map_err(|e| format!("prose settle stage {idx}: {e}"))?;
                match (&results[0].observation, &results[0].rejection) {
                    (Some(obs), None) => {
                        unlink_pinned_leaf(&dir_handle, "propose-in-flight.json")?;
                        ProseStageOutcome::Settled(obs.content.as_str().to_string())
                    }
                    (None, Some(diag)) => {
                        unlink_pinned_leaf(&dir_handle, "propose-in-flight.json")?;
                        ProseStageOutcome::Rejected(diag.as_str().to_string())
                    }
                    _ => ProseStageOutcome::Uncertain,
                }
            }
            #[cfg(test)]
            ProseActiveDriver::Controlled { replies } => {
                let next = replies
                    .pop_front()
                    .unwrap_or_else(|| super::propose::UNCERTAIN_TEST_SENTINEL.to_string());
                if next == super::propose::UNCERTAIN_TEST_SENTINEL {
                    ProseStageOutcome::Uncertain
                } else {
                    unlink_pinned_leaf(&dir_handle, "propose-in-flight.json")?;
                    ProseStageOutcome::Settled(next)
                }
            }
        };

        let stage_bt = BoundedText::<32>::new(stage_name).map_err(|e| format!("{e:?}"))?;
        let bounded_cut = |s: &str| -> Result<BoundedText<256>, String> {
            let cap = s.floor_char_boundary(s.len().min(256));
            BoundedText::<256>::new(&s[..cap]).map_err(|e| format!("{e:?}"))
        };

        match stage_out {
            ProseStageOutcome::Uncertain => {
                unknown_hold = true;
                quality_failure = false;
                declared_ready = false;
                write_external_new_json(
                    &dir_abs.join("propose-hold.json"),
                    workspace_fd,
                    forbidden,
                    &serde_json::json!({
                        "schema_version": CONTRACT_SCHEMA_VERSION,
                        "status": "unknown_outcome_hold",
                        "case_id": case.id,
                        "stage_index": idx,
                    }),
                )?;
                ledger_stages.push(ProsePrepStageRecordV1 {
                    stage_index: idx,
                    stage: stage_bt,
                    outcome: BoundedText::<32>::new("uncertain").map_err(|e| format!("{e:?}"))?,
                    raw_output: None,
                    raw_output_sha256: None,
                    protocol_valid: None,
                    protocol_error: None,
                    diagnostic: None,
                    declared_ready: None,
                });
                break;
            }
            ProseStageOutcome::Rejected(diag) => {
                quality_failure = true;
                declared_ready = false;
                last_diagnostics = vec![diag.clone()];
                let rec = ProsePrepStageRecordV1 {
                    stage_index: idx,
                    stage: stage_bt,
                    outcome: BoundedText::<32>::new("rejected").map_err(|e| format!("{e:?}"))?,
                    raw_output: None,
                    raw_output_sha256: None,
                    protocol_valid: None,
                    protocol_error: None,
                    diagnostic: Some(bounded_cut(&diag)?),
                    declared_ready: Some(false),
                };
                write_external_new_json(
                    &dir_abs.join(format!("stage-{idx}.json")),
                    workspace_fd,
                    forbidden,
                    &rec,
                )?;
                ledger_stages.push(rec);
            }
            ProseStageOutcome::Settled(raw_text) => {
                raw_digest = content_digest(raw_text.as_bytes());
                last_raw = Some(raw_text.clone());
                raw_responses.push(raw_text.clone());
                let bounded_raw = BoundedUtf8::<MAX_INTERNAL>::new(raw_text.clone())
                    .map_err(|e| format!("bounded prose raw output: {e:?}"))?;
                match parse_prose_readiness_protocol(&raw_text) {
                    Ok(Ok(summary)) => {
                        declared_ready = true;
                        quality_failure = false;
                        prepared_summary = Some(summary);
                        let rec = ProsePrepStageRecordV1 {
                            stage_index: idx,
                            stage: stage_bt,
                            outcome: BoundedText::<32>::new("ready")
                                .map_err(|e| format!("{e:?}"))?,
                            raw_output: Some(bounded_raw),
                            raw_output_sha256: Some(raw_digest.clone()),
                            protocol_valid: Some(true),
                            protocol_error: None,
                            diagnostic: None,
                            declared_ready: Some(true),
                        };
                        write_external_new_json(
                            &dir_abs.join(format!("stage-{idx}.json")),
                            workspace_fd,
                            forbidden,
                            &rec,
                        )?;
                        ledger_stages.push(rec);
                        break;
                    }
                    Ok(Err(_blocked_reason)) => {
                        declared_ready = false;
                        quality_failure = false;
                        prepared_summary = None;
                        let rec = ProsePrepStageRecordV1 {
                            stage_index: idx,
                            stage: stage_bt,
                            outcome: BoundedText::<32>::new("blocked")
                                .map_err(|e| format!("{e:?}"))?,
                            raw_output: Some(bounded_raw),
                            raw_output_sha256: Some(raw_digest.clone()),
                            protocol_valid: Some(true),
                            protocol_error: None,
                            diagnostic: None,
                            declared_ready: Some(false),
                        };
                        write_external_new_json(
                            &dir_abs.join(format!("stage-{idx}.json")),
                            workspace_fd,
                            forbidden,
                            &rec,
                        )?;
                        ledger_stages.push(rec);
                        break;
                    }
                    Err(proto_err) => {
                        declared_ready = false;
                        quality_failure = true;
                        last_diagnostics = vec![proto_err.clone()];
                        let rec = ProsePrepStageRecordV1 {
                            stage_index: idx,
                            stage: stage_bt,
                            outcome: BoundedText::<32>::new("protocol_error")
                                .map_err(|e| format!("{e:?}"))?,
                            raw_output: Some(bounded_raw),
                            raw_output_sha256: Some(raw_digest.clone()),
                            protocol_valid: Some(false),
                            protocol_error: Some(bounded_cut(&proto_err)?),
                            diagnostic: None,
                            declared_ready: Some(false),
                        };
                        write_external_new_json(
                            &dir_abs.join(format!("stage-{idx}.json")),
                            workspace_fd,
                            forbidden,
                            &rec,
                        )?;
                        ledger_stages.push(rec);
                    }
                }
            }
        }
    }

    match active {
        ProseActiveDriver::Native { mut driver, .. } => driver.shutdown(),
        #[cfg(test)]
        ProseActiveDriver::Controlled { .. } => {}
    }

    let ledger = ProsePrepLedgerV1 {
        schema_version: CONTRACT_SCHEMA_VERSION,
        case_id: BoundedText::new(case.id.clone()).map_err(|e| format!("{e:?}"))?,
        evidence_origin,
        source_coding_profile_digest,
        team_profile_digest,
        max_prep_requests: max_stages,
        admitted_prep_requests: admitted,
        raw_output_sha256: raw_digest.clone(),
        declared_ready,
        unknown_hold,
        quality_failure,
        stages: ledger_stages,
    };
    write_external_new_json(
        &dir_abs.join("prose-ledger.json"),
        workspace_fd,
        forbidden,
        &ledger,
    )?;

    Ok(ProsePrepOutcome {
        raw_output_digest: raw_digest,
        prepared_summary,
        raw_responses,
        declared_ready,
        unknown_hold,
        quality_failure,
        admitted_requests: admitted,
    })
}

fn check_oracle_leak(
    case: &FrozenCorpusCaseV1,
    bundle: Option<&ContractBundleV1>,
    extra_texts: &[&str],
) -> bool {
    let clar_text = case.frozen_clarification.as_deref().unwrap_or("");
    let mut forbidden_needles = Vec::new();
    for v in &case.sealed_cli_vectors {
        let trimmed = v.stdout.trim();
        if trimmed.len() >= 4 && !case.raw_request.contains(trimmed) && !clar_text.contains(trimmed)
        {
            forbidden_needles.push(trimmed.to_string());
        }
    }
    let mut haystacks: Vec<String> = extra_texts.iter().map(|s| s.to_string()).collect();
    if let Some(b) = bundle {
        haystacks.push(b.objective.as_str().to_string());
    }
    for h in &haystacks {
        if h.contains("sealed_cli_vectors")
            || h.contains("clarified_expected_readiness")
            || h.contains("initial_expected_readiness")
        {
            return true;
        }
        for needle in &forbidden_needles {
            if h.contains(needle.as_str()) {
                return true;
            }
        }
    }
    false
}

#[derive(Clone, Debug)]
struct ResolvedCampaignPins {
    evidence_origin: EvidenceOriginV1,
    host_executable_sha256: Digest,
    coding_profile_digest: Option<Digest>,
    team_profile_digest: Option<Digest>,
    worker_sha256: Option<Digest>,
    artifact_sha256: Option<Digest>,
}

fn resolve_campaign_pins(
    transport: &ExperimentTransportConfig,
) -> Result<ResolvedCampaignPins, String> {
    let host_executable_sha256 = current_host_executable_digest()?;
    match transport {
        ExperimentTransportConfig::Native {
            coding_profile_path,
            worker_path,
            artifact_path,
        } => {
            let (_, coding_profile, _) =
                read_external_json::<LocalProfile>(coding_profile_path, None)?;
            let (team_profile, coding_profile_digest, team_profile_digest) =
                derive_team_text_profile(&coding_profile)?;
            let (_, worker_abs) = open_external_nofollow(worker_path, false)?;
            let (_, artifact_abs) = open_external_nofollow(artifact_path, false)?;
            verify_artifact(&artifact_abs, resolve_pinned_artifact(&team_profile)?)
                .map_err(|e| format!("verify native artifact: {e}"))?;
            let worker_hex = sha256_file(&worker_abs)
                .map_err(|e| format!("hash worker {}: {e:?}", worker_abs.display()))?;
            let artifact_hex = sha256_file(&artifact_abs)
                .map_err(|e| format!("hash artifact {}: {e:?}", artifact_abs.display()))?;
            Ok(ResolvedCampaignPins {
                evidence_origin: EvidenceOriginV1::NativeTeamSession,
                host_executable_sha256,
                coding_profile_digest: Some(coding_profile_digest),
                team_profile_digest: Some(team_profile_digest),
                worker_sha256: Some(Digest::new(worker_hex).map_err(|e| format!("{e:?}"))?),
                artifact_sha256: Some(Digest::new(artifact_hex).map_err(|e| format!("{e:?}"))?),
            })
        }
        #[cfg(test)]
        ExperimentTransportConfig::Controlled {
            evidence_origin, ..
        } => Ok(ResolvedCampaignPins {
            evidence_origin: *evidence_origin,
            host_executable_sha256,
            coding_profile_digest: None,
            team_profile_digest: None,
            worker_sha256: None,
            artifact_sha256: None,
        }),
    }
}

fn recompute_campaign_config_digest(cfg: &ExperimentCampaignConfigV1) -> Result<Digest, String> {
    #[derive(Serialize)]
    struct ConfigView<'a> {
        schema_version: u16,
        corpus_sha256: &'a Digest,
        corpus_freeze_sha256: &'a Digest,
        host_executable_sha256: &'a Digest,
        checker_impl_digest: &'a Digest,
        catalog_digest: &'a Digest,
        public_harness_preamble: &'a BoundedText<256>,
        objective_projection_recipe: &'a BoundedText<64>,
        arm_order: &'a [ExperimentArmV1],
        caps: &'a FrozenCorpusCapsV1,
        evidence_origin: EvidenceOriginV1,
        coding_profile_digest: &'a Option<Digest>,
        team_profile_digest: &'a Option<Digest>,
        worker_sha256: &'a Option<Digest>,
        artifact_sha256: &'a Option<Digest>,
    }
    let view = ConfigView {
        schema_version: cfg.schema_version,
        corpus_sha256: &cfg.corpus_sha256,
        corpus_freeze_sha256: &cfg.corpus_freeze_sha256,
        host_executable_sha256: &cfg.host_executable_sha256,
        checker_impl_digest: &cfg.checker_impl_digest,
        catalog_digest: &cfg.catalog_digest,
        public_harness_preamble: &cfg.public_harness_preamble,
        objective_projection_recipe: &cfg.objective_projection_recipe,
        arm_order: &cfg.arm_order,
        caps: &cfg.caps,
        evidence_origin: cfg.evidence_origin,
        coding_profile_digest: &cfg.coding_profile_digest,
        team_profile_digest: &cfg.team_profile_digest,
        worker_sha256: &cfg.worker_sha256,
        artifact_sha256: &cfg.artifact_sha256,
    };
    let bytes = canonical_bytes(&view, MAX_INTERNAL).map_err(|e| format!("config bytes: {e:?}"))?;
    structural_digest("coding-contract-experiment-config/v2", &[&bytes])
        .map_err(|e| format!("config digest: {e:?}"))
}

fn build_campaign_config(
    campaign_id: &str,
    corpus_sha: &Digest,
    freeze_sha: &Digest,
    corpus: &FrozenCorpusFileV1,
    catalog: &ToolCatalog,
    pins: &ResolvedCampaignPins,
) -> Result<ExperimentCampaignConfigV1, String> {
    let checker_impl_digest =
        contract_verifier_implementation_digest_with_host(catalog, &pins.host_executable_sha256)?;
    let mut cfg = ExperimentCampaignConfigV1 {
        schema_version: CONTRACT_SCHEMA_VERSION,
        campaign_id: BoundedText::new(campaign_id).map_err(|e| format!("campaign_id: {e:?}"))?,
        corpus_sha256: corpus_sha.clone(),
        corpus_freeze_sha256: freeze_sha.clone(),
        host_executable_sha256: pins.host_executable_sha256.clone(),
        checker_impl_digest,
        catalog_digest: catalog
            .digest()
            .map_err(|e| format!("catalog digest: {e:?}"))?,
        public_harness_preamble: BoundedText::new(PUBLIC_HARNESS_PREAMBLE)
            .map_err(|e| format!("{e:?}"))?,
        objective_projection_recipe: BoundedText::new(OBJECTIVE_PROJECTION_RECIPE_TAG)
            .map_err(|e| format!("{e:?}"))?,
        arm_order: corpus.arm_order.clone(),
        caps: corpus.caps.clone(),
        evidence_origin: pins.evidence_origin,
        coding_profile_digest: pins.coding_profile_digest.clone(),
        team_profile_digest: pins.team_profile_digest.clone(),
        worker_sha256: pins.worker_sha256.clone(),
        artifact_sha256: pins.artifact_sha256.clone(),
        config_digest: corpus_sha.clone(),
    };
    cfg.config_digest = recompute_campaign_config_digest(&cfg)?;
    Ok(cfg)
}

pub(crate) fn annotation_set_digest(
    set: &ContractSemanticAnnotationSetV1,
) -> Result<Digest, String> {
    let bytes =
        canonical_bytes(set, MAX_INTERNAL).map_err(|e| format!("annotation bytes: {e:?}"))?;
    structural_digest("coding-contract-annotations/v1", &[&bytes])
        .map_err(|e| format!("annotation digest: {e:?}"))
}

fn is_self_reviewer_name(reviewer: &str) -> bool {
    let lower = reviewer.trim().to_ascii_lowercase();
    lower.is_empty()
        || lower.starts_with("unreviewed")
        || matches!(
            lower.as_str(),
            "model"
                | "self"
                | "candidate"
                | "lfm3"
                | "auto"
                | "reasoner"
                | "unreviewed_template"
                | "unreviewed"
                | "todo"
                | "template"
                | "placeholder"
        )
}

pub(crate) fn verify_annotation_set(
    set: &ContractSemanticAnnotationSetV1,
    campaign_id: &str,
    corpus_sha256: &Digest,
    config_digest: &Digest,
    expected_outputs: &BTreeMap<(String, ExperimentArmV1, AnnotationStageV1), Digest>,
) -> Result<Digest, String> {
    if set.schema_version != CONTRACT_SCHEMA_VERSION {
        return Err(format!(
            "unsupported annotation schema_version {}",
            set.schema_version
        ));
    }
    if set.campaign_id.as_str() != campaign_id {
        return Err(format!(
            "annotation campaign_id '{}' mismatch expected '{campaign_id}'",
            set.campaign_id.as_str()
        ));
    }
    if &set.corpus_sha256 != corpus_sha256 {
        return Err("annotation corpus_sha256 mismatch".into());
    }
    if &set.config_digest != config_digest {
        return Err("annotation config_digest mismatch".into());
    }
    if set.annotations.len() > MAX_ANNOTATIONS {
        return Err(format!(
            "annotations count {} exceeds MAX_ANNOTATIONS ({MAX_ANNOTATIONS})",
            set.annotations.len()
        ));
    }

    let mut seen: BTreeMap<
        (String, ExperimentArmV1, AnnotationStageV1),
        &CaseSemanticAnnotationV1,
    > = BTreeMap::new();
    for ann in &set.annotations {
        if is_self_reviewer_name(ann.reviewer.as_str()) {
            return Err(format!(
                "invalid or self-review reviewer '{}' on annotation '{}'",
                ann.reviewer.as_str(),
                ann.case_id.as_str()
            ));
        }
        let key = (ann.case_id.as_str().to_string(), ann.arm, ann.stage);
        if seen.insert(key.clone(), ann).is_some() {
            return Err(format!(
                "duplicate annotation for ({}, {}, {:?})",
                key.0,
                arm_slug(key.1),
                key.2
            ));
        }
    }

    for (key, expected_digest) in expected_outputs {
        let Some(ann) = seen.get(key) else {
            return Err(format!(
                "missing independent annotation for ({}, {}, {:?})",
                key.0,
                arm_slug(key.1),
                key.2
            ));
        };
        if &ann.raw_output_digest != expected_digest {
            return Err(format!(
                "annotation raw_output_digest mismatch for ({}, {}, {:?})",
                key.0,
                arm_slug(key.1),
                key.2
            ));
        }
    }

    if seen.len() != expected_outputs.len() {
        return Err("annotation set contains entries not matching pilot campaign outputs".into());
    }

    annotation_set_digest(set)
}

pub(crate) fn evaluate_promotion_gate(
    campaign_id: &str,
    corpus_sha256: &Digest,
    config_digest: &Digest,
    evidence_origin: EvidenceOriginV1,
    diagnostic_gate_passed: bool,
    attempts: &[ExperimentAttemptResultV1],
    annotation_set: Option<&ContractSemanticAnnotationSetV1>,
) -> Result<ContractPromotionReceiptV1, String> {
    let complete_pilot = attempts.len() == 48
        && [
            ExperimentArmV1::ProsePlain,
            ExperimentArmV1::ProseArchitecture,
            ExperimentArmV1::TypedPlain,
            ExperimentArmV1::TypedArchitecture,
        ]
        .iter()
        .all(|arm| {
            let arm_attempts: Vec<_> = attempts.iter().filter(|a| a.arm == *arm).collect();
            arm_attempts.len() == 12
                && arm_attempts
                    .iter()
                    .filter(|a| a.slice == CorpusSliceV1::New)
                    .count()
                    == 6
                && arm_attempts
                    .iter()
                    .map(|a| a.case_id.as_str())
                    .collect::<BTreeSet<_>>()
                    .len()
                    == 12
        })
        && attempts
            .iter()
            .map(|a| a.case_id.as_str())
            .collect::<BTreeSet<_>>()
            .len()
            == 12;
    let mut expected_outputs = BTreeMap::new();
    for att in attempts {
        expected_outputs.insert(
            (
                att.case_id.as_str().to_string(),
                att.arm,
                AnnotationStageV1::Raw,
            ),
            att.raw_output_digest.clone(),
        );
        if let Some(clar_dig) = &att.clarified_output_digest {
            expected_outputs.insert(
                (
                    att.case_id.as_str().to_string(),
                    att.arm,
                    AnnotationStageV1::Clarified,
                ),
                clar_dig.clone(),
            );
        }
    }

    let mut ann_error: Option<String> = None;
    let ann_digest_opt = match annotation_set {
        None => {
            ann_error = Some("missing independent Sol semantic annotation file".to_string());
            None
        }
        Some(set) => match verify_annotation_set(
            set,
            campaign_id,
            corpus_sha256,
            config_digest,
            &expected_outputs,
        ) {
            Ok(dig) => Some(dig),
            Err(err) => {
                ann_error = Some(err);
                None
            }
        },
    };

    let arms = [
        ExperimentArmV1::ProsePlain,
        ExperimentArmV1::ProseArchitecture,
        ExperimentArmV1::TypedPlain,
        ExperimentArmV1::TypedArchitecture,
    ];
    let mut summaries = Vec::with_capacity(4);
    let mut summary_by_arm: BTreeMap<ExperimentArmV1, ArmComparisonSummaryV1> = BTreeMap::new();
    let mut passed_map: BTreeMap<(ExperimentArmV1, String), bool> = BTreeMap::new();

    for arm in arms {
        let mut s = ArmComparisonSummaryV1 {
            arm,
            new_accepted: 0,
            existing_accepted: 0,
            authority_faults: 0,
            oracle_leaks: 0,
            critical_false_ready: 0,
            silent_material_assumptions: 0,
            unjustified_blocks: 0,
            raw_readiness_exact_matches: 0,
            total_model_requests: 0,
            total_admitted_tool_effects: 0,
        };
        for att in attempts.iter().filter(|a| a.arm == arm) {
            let clean_accept =
                att.accepted && !att.authority_fault && !att.oracle_leak && !att.prep_unknown_hold;
            passed_map.insert((arm, att.case_id.as_str().to_string()), clean_accept);
            if clean_accept {
                match att.slice {
                    CorpusSliceV1::New => s.new_accepted += 1,
                    CorpusSliceV1::Existing => s.existing_accepted += 1,
                }
            }
            if att.authority_fault {
                s.authority_faults += 1;
            }
            if att.oracle_leak {
                s.oracle_leaks += 1;
            }
            if att.raw_readiness_exact_matches_val() {
                s.raw_readiness_exact_matches += 1;
            }
            s.total_model_requests = s
                .total_model_requests
                .saturating_add(att.prep_requests_admitted + att.coding_model_requests_used);
            s.total_admitted_tool_effects = s
                .total_admitted_tool_effects
                .saturating_add(att.total_admitted_tool_effects);
        }
        if let Some(set) = annotation_set {
            for ann in set.annotations.iter().filter(|a| a.arm == arm) {
                if ann.critical_false_ready {
                    s.critical_false_ready += 1;
                }
                if ann.unjustified_block {
                    s.unjustified_blocks += 1;
                }
                s.silent_material_assumptions = s
                    .silent_material_assumptions
                    .saturating_add(ann.silent_material_assumptions);
            }
        }
        summary_by_arm.insert(arm, s.clone());
        summaries.push(s);
    }

    let zero_regressions = |candidate: ExperimentArmV1, baseline: ExperimentArmV1| -> bool {
        !attempts.iter().filter(|a| a.arm == baseline).any(|a| {
            let cid = a.case_id.as_str().to_string();
            let base_ok = passed_map
                .get(&(baseline, cid.clone()))
                .copied()
                .unwrap_or(false);
            let cand_ok = passed_map.get(&(candidate, cid)).copied().unwrap_or(false);
            base_ok && !cand_ok
        })
    };

    let beats_arm = |candidate: ExperimentArmV1, baseline: ExperimentArmV1| -> bool {
        let c = &summary_by_arm[&candidate];
        let b = &summary_by_arm[&baseline];
        c.new_accepted > b.new_accepted
            && c.existing_accepted > b.existing_accepted
            && zero_regressions(candidate, baseline)
            && c.authority_faults == 0
            && c.oracle_leaks == 0
            && c.critical_false_ready == 0
            && c.silent_material_assumptions == 0
            && c.unjustified_blocks == 0
    };

    let mut reasons = Vec::new();
    let eligible_typed_plain = beats_arm(ExperimentArmV1::TypedPlain, ExperimentArmV1::ProsePlain);
    let eligible_typed_architecture = beats_arm(
        ExperimentArmV1::TypedArchitecture,
        ExperimentArmV1::ProseArchitecture,
    );
    let arch_beats_plain = beats_arm(
        ExperimentArmV1::TypedArchitecture,
        ExperimentArmV1::TypedPlain,
    );
    let prose_arch_beats_prose_plain = beats_arm(
        ExperimentArmV1::ProseArchitecture,
        ExperimentArmV1::ProsePlain,
    );
    let architecture_only_finding =
        prose_arch_beats_prose_plain && !eligible_typed_plain && !eligible_typed_architecture;

    let (verdict, selected_arm) = if !complete_pilot {
        push_bounded_reason(
            &mut reasons,
            "promotion blocked: requires exact 12 cases x 4 arms, six cases per slice",
        );
        (PromotionVerdictV1::BlockedInvalidAnnotationsOrOrigin, None)
    } else if !diagnostic_gate_passed {
        push_bounded_reason(
            &mut reasons,
            "separate diagnostic gate failed in at least one slice",
        );
        (PromotionVerdictV1::DiagnosticGateBlocked, None)
    } else if evidence_origin != EvidenceOriginV1::NativeTeamSession {
        push_bounded_reason(
            &mut reasons,
            "promotion blocked: non-native evidence_origin cannot establish native quality",
        );
        (PromotionVerdictV1::BlockedInvalidAnnotationsOrOrigin, None)
    } else if let Some(err) = ann_error {
        push_bounded_reason(&mut reasons, &format!("promotion blocked: {err}"));
        (PromotionVerdictV1::BlockedInvalidAnnotationsOrOrigin, None)
    } else if eligible_typed_plain && eligible_typed_architecture && arch_beats_plain {
        push_bounded_reason(
            &mut reasons,
            "promoted typed_architecture: beat prose_architecture and typed_plain across both slices with zero regressions/safety faults",
        );
        (
            PromotionVerdictV1::PromotedTypedPlusArchitecture,
            Some(ExperimentArmV1::TypedArchitecture),
        )
    } else if eligible_typed_plain {
        push_bounded_reason(
            &mut reasons,
            "promoted typed_plain: beat prose_plain across both slices with zero regressions/safety faults",
        );
        (
            PromotionVerdictV1::PromotedTypedOnly,
            Some(ExperimentArmV1::TypedPlain),
        )
    } else if eligible_typed_architecture && arch_beats_plain {
        push_bounded_reason(
            &mut reasons,
            "promoted typed_architecture: beat prose_architecture and typed_plain cleanly",
        );
        (
            PromotionVerdictV1::PromotedTypedPlusArchitecture,
            Some(ExperimentArmV1::TypedArchitecture),
        )
    } else {
        if architecture_only_finding {
            push_bounded_reason(
                &mut reasons,
                "architecture_only_finding: prose_architecture improved over prose_plain without typed contract gain",
            );
        } else {
            push_bounded_reason(
                &mut reasons,
                "negative promotion gate: typed arms did not achieve +1 accepted per slice without regression/safety violations",
            );
        }
        (PromotionVerdictV1::NegativeDoNotPromote, None)
    };

    let decision = PromotionGateDecisionV1 {
        verdict,
        eligible_typed_plain,
        eligible_typed_architecture,
        selected_arm,
        architecture_only_finding,
        annotation_set_digest: ann_digest_opt,
        reasons,
        summaries,
    };
    let camp_bt = BoundedText::new(campaign_id).map_err(|e| format!("campaign_id: {e:?}"))?;
    let receipt_digest = promotion_receipt_digest(
        &camp_bt,
        corpus_sha256,
        config_digest,
        evidence_origin,
        &decision,
    )?;

    Ok(ContractPromotionReceiptV1 {
        schema_version: CONTRACT_SCHEMA_VERSION,
        receipt_digest,
        campaign_id: camp_bt,
        corpus_sha256: corpus_sha256.clone(),
        config_digest: config_digest.clone(),
        evidence_origin,
        decision,
    })
}

impl ExperimentAttemptResultV1 {
    fn raw_readiness_exact_matches_val(&self) -> bool {
        self.raw_readiness_matched
    }
}

fn push_bounded_reason(reasons: &mut Vec<BoundedText<256>>, msg: &str) {
    let end = msg.floor_char_boundary(msg.len().min(256));
    if let Ok(bt) = BoundedText::new(&msg[..end]) {
        reasons.push(bt);
    }
}

/// Fail-closed native promotion receipt verifier for unit verification.
#[cfg(test)]
pub(crate) fn verify_native_promotion_receipt(
    receipt_path: &Path,
    annotations_path: &Path,
    workspace_fd: Option<&File>,
) -> Result<(ContractPromotionReceiptV1, ExperimentArmV1), String> {
    let (_, receipt, _) =
        read_external_json::<ContractPromotionReceiptV1>(receipt_path, workspace_fd)?;
    let (_, annotations, _) =
        read_external_json::<ContractSemanticAnnotationSetV1>(annotations_path, workspace_fd)?;

    if receipt.schema_version != CONTRACT_SCHEMA_VERSION
        || annotations.schema_version != CONTRACT_SCHEMA_VERSION
    {
        return Err("unsupported promotion receipt or annotation schema version".into());
    }
    if receipt.corpus_sha256.as_str() != EXPECTED_INDEPENDENT_CORPUS_SHA256 {
        return Err("promotion receipt corpus_sha256 does not match frozen corpus".into());
    }
    let expected_receipt_dig = promotion_receipt_digest(
        &receipt.campaign_id,
        &receipt.corpus_sha256,
        &receipt.config_digest,
        receipt.evidence_origin,
        &receipt.decision,
    )?;
    if expected_receipt_dig != receipt.receipt_digest {
        return Err("promotion receipt_digest tampered".into());
    }
    if receipt.evidence_origin != EvidenceOriginV1::NativeTeamSession {
        return Err(
            "run/resume blocked: promotion receipt must originate from NativeTeamSession".into(),
        );
    }
    if !matches!(
        receipt.decision.verdict,
        PromotionVerdictV1::PromotedTypedOnly | PromotionVerdictV1::PromotedTypedPlusArchitecture
    ) {
        return Err(format!(
            "run/resume blocked: promotion verdict is {:?}, not promoted",
            receipt.decision.verdict
        ));
    }
    let selected_arm = receipt
        .decision
        .selected_arm
        .ok_or_else(|| "run/resume blocked: promoted receipt missing selected_arm".to_string())?;
    if annotations.campaign_id != receipt.campaign_id
        || annotations.corpus_sha256 != receipt.corpus_sha256
        || annotations.config_digest != receipt.config_digest
        || annotations.annotations.is_empty()
    {
        return Err("annotation set does not match promoted campaign identity".into());
    }
    for ann in &annotations.annotations {
        if is_self_reviewer_name(ann.reviewer.as_str()) {
            return Err("annotation contains invalid self-review reviewer".into());
        }
    }
    let expected_ann_digest = annotation_set_digest(&annotations)?;
    if receipt.decision.annotation_set_digest.as_ref() != Some(&expected_ann_digest) {
        return Err("run/resume blocked: promotion receipt annotation_set_digest mismatch".into());
    }
    Ok((receipt, selected_arm))
}

fn slice_for_workflow(w: ContractWorkflow) -> CorpusSliceV1 {
    match w {
        ContractWorkflow::New => CorpusSliceV1::New,
        ContractWorkflow::Existing => CorpusSliceV1::Existing,
    }
}

pub(crate) fn run_experiment(mut req: ExperimentRequest) -> Result<ExperimentReportV1, String> {
    if req.output_dir.exists() {
        let (_existing_fd, abs) = open_external_nofollow(&req.output_dir, true)?;
        return Err(format!(
            "experiment output_dir {} already exists; choose a fresh directory",
            abs.display()
        ));
    }
    let (_out_fd, out_abs) = create_external_private_dir(&req.output_dir, None, &[])?;

    let (_, corpus_bytes, _) =
        super::io::read_external_bounded_bytes(&req.corpus_path, None, MAX_INTERNAL)?;
    let (_, freeze_bytes, _) =
        super::io::read_external_bounded_bytes(&req.corpus_freeze_path, None, MAX_INTERNAL)?;
    let freeze_sha = content_digest(&freeze_bytes);
    let (corpus, _freeze, corpus_sha) = load_and_verify_corpus_bytes(&corpus_bytes, &freeze_bytes)?;

    let pins = resolve_campaign_pins(&req.transport)?;
    let evidence_origin = pins.evidence_origin;
    let campaign_config = build_campaign_config(
        &req.campaign_id,
        &corpus_sha,
        &freeze_sha,
        &corpus,
        &req.catalog,
        &pins,
    )?;
    let config_digest = campaign_config.config_digest.clone();
    write_external_new_json(
        &out_abs.join("campaign-config.json"),
        None,
        &[],
        &campaign_config,
    )?;

    // 1. Separate reviewed-contract diagnostic gate (2 new, 2 existing).
    let diag_dir = out_abs.join("diagnostic");
    create_external_private_dir(&diag_dir, None, &[])?;
    let mut diag_results = Vec::with_capacity(corpus.diagnostic_cases.len());
    let mut diag_new_accepted = 0u32;
    let mut diag_existing_accepted = 0u32;

    for (idx, dcase) in corpus.diagnostic_cases.iter().enumerate() {
        let case_dir = diag_dir.join(format!("{idx:02}-{}", dcase.id));
        let (_case_fd, case_abs) = create_external_private_dir(&case_dir, None, &[])?;
        let ws_path = case_abs.join("workspace");
        let run_dir = case_abs.join("run");
        let bundle_path = case_abs.join("bundle.json");

        let (ws_fd, ws_abs, snap) = build_workspace_for_attempt(&ws_path, &[], dcase, false)?;
        let auth = build_host_auth_from_case(dcase, None, snap);
        let draft = build_diagnostic_draft(dcase, &auth)?;
        let (_, bundle) = seal_draft_with_corpus_vectors(&draft, &auth, &[])?;
        write_external_new_json(&bundle_path, Some(&ws_fd), &[], &bundle)?;

        let task_id = TaskId::new(format!(
            "diag-{idx}-{}",
            &content_digest(dcase.id.as_bytes()).as_str()[..12]
        ))
        .map_err(|e| format!("{e:?}"))?;

        let (native_profile, native_worker, native_artifact, scripted, runner_factory) =
            match &mut req.transport {
                ExperimentTransportConfig::Native {
                    coding_profile_path,
                    worker_path,
                    artifact_path,
                } => (
                    Some(coding_profile_path.clone()),
                    Some(worker_path.clone()),
                    Some(artifact_path.clone()),
                    None::<Vec<String>>,
                    #[cfg(test)]
                    None,
                    #[cfg(not(test))]
                    None::<()>,
                ),
                #[cfg(test)]
                ExperimentTransportConfig::Controlled {
                    fixture,
                    custom_runner_factory,
                    ..
                } => {
                    let replies = fixture
                        .diagnostic_scripts
                        .get(&dcase.id)
                        .cloned()
                        .unwrap_or_default();
                    let make = custom_runner_factory.as_mut().map(|f| f());
                    (None, None, None, Some(replies), make)
                }
            };

        #[cfg(not(test))]
        let _ = (&scripted, &runner_factory);
        let outcome = execute_contract_coding_attempt(CodingAttemptRequest {
            task_id,
            bundle_path,
            workspace_path: ws_abs,
            run_dir,
            catalog: req.catalog.clone(),
            dev_checks_enabled: true,
            architecture_scaffold: dcase.workflow == ContractWorkflow::New,
            prep_requests_admitted: 0,
            total_model_cap: corpus.caps.total_model_requests,
            max_tool_calls: corpus.caps.admitted_tool_effects,
            evidence_origin,
            profile_path: native_profile,
            worker_path: native_worker,
            artifact_path: native_artifact,
            #[cfg(test)]
            scripted_replies: scripted,
            predecessor_run_dir: None,
            predecessor_task_id: None,
            #[cfg(test)]
            custom_verifier_runner: runner_factory,
            #[cfg(test)]
            allowed_metadata_roots: vec![PathBuf::from("/workspace"), case_abs.clone()],
        })?;

        let slice = slice_for_workflow(dcase.workflow);
        if outcome.accepted {
            match slice {
                CorpusSliceV1::New => diag_new_accepted += 1,
                CorpusSliceV1::Existing => diag_existing_accepted += 1,
            }
        }
        diag_results.push(DiagnosticAttemptResultV1 {
            case_id: BoundedText::new(dcase.id.clone()).map_err(|e| format!("{e:?}"))?,
            slice,
            accepted: outcome.accepted,
            model_requests_used: outcome.model_requests_used,
            tool_calls_used: outcome.tool_calls_used,
            outcome: BoundedText::new(outcome.outcome).map_err(|e| format!("{e:?}"))?,
        });
    }

    let diag_passed = diag_new_accepted >= 1 && diag_existing_accepted >= 1;
    let diagnostic_gate = DiagnosticGateReportV1 {
        schema_version: CONTRACT_SCHEMA_VERSION,
        passed: diag_passed,
        new_accepted: diag_new_accepted,
        existing_accepted: diag_existing_accepted,
        results: diag_results,
    };
    write_external_new_json(
        &out_abs.join("diagnostic-gate-report.json"),
        None,
        &[],
        &diagnostic_gate,
    )?;

    let annotations_opt = req
        .annotations_path
        .as_ref()
        .map(|p| {
            read_external_json::<ContractSemanticAnnotationSetV1>(p, None).map(|(_, set, _)| set)
        })
        .transpose()?;

    // 2. Execute 12 x 4 = 48 2x2 factorial attempts with counterbalanced arm rotation.
    // Even if the diagnostic gate failed, execute all 48 prep extractions (`skip_coding = true`)
    // so full prep outputs and an exact annotation template are retained while blocking promotion.
    let skip_coding = !diag_passed;
    let pilot_dir = out_abs.join("pilot");
    create_external_private_dir(&pilot_dir, None, &[])?;
    let mut attempts = Vec::with_capacity(48);

    for (case_idx, case) in corpus.cases.iter().enumerate() {
        let slice = slice_for_workflow(case.workflow);
        for offset in 0..corpus.arm_order.len() {
            let arm = corpus.arm_order[(case_idx + offset) % corpus.arm_order.len()];
            let dev_checks = matches!(
                arm,
                ExperimentArmV1::ProseArchitecture | ExperimentArmV1::TypedArchitecture
            );
            let typed = matches!(
                arm,
                ExperimentArmV1::TypedPlain | ExperimentArmV1::TypedArchitecture
            );
            let arch_scaffold = case.workflow == ContractWorkflow::New && dev_checks;

            let attempt_dir =
                pilot_dir.join(format!("{case_idx:02}-{}-{}", case.id, arm_slug(arm)));
            let (_att_fd, att_abs) = create_external_private_dir(&attempt_dir, None, &[])?;
            let ws_path = att_abs.join("workspace");
            let (_ws_fd, ws_abs, ws_snap) =
                build_workspace_for_attempt(&ws_path, &[], case, arch_scaffold)?;
            let raw_auth = build_host_auth_from_case(case, None, ws_snap.clone());

            let contract_id = format!("{}-{}", case.id, arm_slug(arm));
            let (
                raw_digest,
                clar_digest,
                raw_declared_ready,
                prep_unknown_hold,
                prep_quality_failure,
                prep_requests_admitted,
                ready_draft_opt,
                final_auth,
                prep_leak,
            ) = if typed {
                let raw_prep_dir = att_abs.join("prep-raw");
                let raw_transport =
                    propose_transport_for_attempt(&req.transport, &case.id, arm, false)?;
                let raw_out = run_propose(
                    &raw_prep_dir,
                    Some(&_ws_fd),
                    None,
                    &raw_auth,
                    &ProposeParams {
                        contract_id: contract_id.clone(),
                        workflow: case.workflow,
                        selected_binary: case.selected_binary.clone(),
                        max_stages: raw_preparation_cap(case, corpus.caps.preparation_requests),
                        architecture_scaffold: dev_checks,
                    },
                    raw_transport,
                )?;
                let raw_dig = stage_digest_from_ledger(&raw_out.ledger);
                let raw_ready = raw_out
                    .validation
                    .as_ref()
                    .is_some_and(|v| v.declared_ready);
                let mut total_prep = u32::from(raw_out.ledger.admitted_prep_requests);
                let remaining_prep = corpus
                    .caps
                    .preparation_requests
                    .saturating_sub(raw_out.ledger.admitted_prep_requests);
                let mut hold = raw_out.ledger.unknown_hold;
                let mut qual_fail = raw_out.ledger.quality_failure;
                let mut chosen_draft = if raw_ready { raw_out.draft } else { None };
                let mut active_auth = raw_auth.clone();
                let mut clar_dig_opt = None;
                let mut extra_leak = check_oracle_leak_in_ledger(case, &raw_out.ledger);

                if let Some(clar_text) = &case.frozen_clarification
                    && !hold
                {
                    if remaining_prep == 0 {
                        chosen_draft = None;
                        qual_fail = true;
                    } else {
                        let clar_auth = build_host_auth_from_case(case, Some(clar_text), ws_snap);
                        let clar_prep_dir = att_abs.join("prep-clarified");
                        let clar_transport =
                            propose_transport_for_attempt(&req.transport, &case.id, arm, true)?;
                        let clar_out = run_propose(
                            &clar_prep_dir,
                            Some(&_ws_fd),
                            None,
                            &clar_auth,
                            &ProposeParams {
                                contract_id: contract_id.clone(),
                                workflow: case.workflow,
                                selected_binary: case.selected_binary.clone(),
                                max_stages: remaining_prep,
                                architecture_scaffold: dev_checks,
                            },
                            clar_transport,
                        )?;
                        clar_dig_opt = Some(stage_digest_from_ledger(&clar_out.ledger));
                        total_prep = total_prep
                            .saturating_add(u32::from(clar_out.ledger.admitted_prep_requests));
                        hold = clar_out.ledger.unknown_hold;
                        qual_fail = clar_out.ledger.quality_failure;
                        let clar_ready = clar_out
                            .validation
                            .as_ref()
                            .is_some_and(|v| v.declared_ready);
                        chosen_draft = if clar_ready && !hold && !qual_fail {
                            clar_out.draft
                        } else {
                            None
                        };
                        extra_leak =
                            extra_leak || check_oracle_leak_in_ledger(case, &clar_out.ledger);
                        active_auth = clar_auth;
                    }
                }
                (
                    raw_dig,
                    clar_dig_opt,
                    raw_ready,
                    hold,
                    qual_fail,
                    total_prep,
                    chosen_draft,
                    active_auth,
                    extra_leak,
                )
            } else {
                let raw_prep = run_prose_prep(
                    &att_abs.join("prep-raw"),
                    Some(&_ws_fd),
                    &[],
                    case,
                    &raw_auth,
                    dev_checks,
                    raw_preparation_cap(case, corpus.caps.preparation_requests),
                    evidence_origin,
                    &req.transport,
                )?;
                let mut total_prep = raw_prep.admitted_requests;
                let remaining_prep = corpus
                    .caps
                    .preparation_requests
                    .saturating_sub(u8::try_from(raw_prep.admitted_requests).unwrap_or(u8::MAX));
                let mut hold = raw_prep.unknown_hold;
                let mut qual_fail = raw_prep.quality_failure;
                let mut active_auth = raw_auth;
                let mut clar_dig_opt = None;
                let mut clar_ready = raw_prep.declared_ready;
                let mut prepared_summary = raw_prep.prepared_summary;
                let raw_refs: Vec<&str> =
                    raw_prep.raw_responses.iter().map(String::as_str).collect();
                let mut extra_leak = check_oracle_leak(case, None, &raw_refs);

                if let Some(clar_text) = &case.frozen_clarification
                    && !hold
                {
                    if remaining_prep == 0 {
                        clar_ready = false;
                        qual_fail = true;
                        prepared_summary = None;
                    } else {
                        let clar_auth = build_host_auth_from_case(case, Some(clar_text), ws_snap);
                        let clar_prep = run_prose_prep(
                            &att_abs.join("prep-clarified"),
                            Some(&_ws_fd),
                            &[],
                            case,
                            &clar_auth,
                            dev_checks,
                            remaining_prep,
                            evidence_origin,
                            &req.transport,
                        )?;
                        clar_dig_opt = Some(clar_prep.raw_output_digest);
                        total_prep = total_prep.saturating_add(clar_prep.admitted_requests);
                        hold = clar_prep.unknown_hold;
                        qual_fail = clar_prep.quality_failure;
                        clar_ready = clar_prep.declared_ready;
                        prepared_summary = clar_prep.prepared_summary;
                        let clar_refs: Vec<&str> =
                            clar_prep.raw_responses.iter().map(String::as_str).collect();
                        extra_leak = extra_leak || check_oracle_leak(case, None, &clar_refs);
                        active_auth = clar_auth;
                    }
                }

                let flat_draft = if !hold && !qual_fail && clar_ready {
                    Some(build_flat_prose_draft(
                        &contract_id,
                        case,
                        &active_auth,
                        case.frozen_clarification.as_deref(),
                        prepared_summary.as_deref(),
                    )?)
                } else {
                    None
                };

                (
                    raw_prep.raw_output_digest,
                    clar_dig_opt,
                    raw_prep.declared_ready,
                    hold,
                    qual_fail,
                    total_prep,
                    flat_draft,
                    active_auth,
                    extra_leak,
                )
            };

            let raw_readiness_matched = matches!(
                (case.initial_expected_readiness, raw_declared_ready),
                (ExpectedReadinessV1::Ready, true) | (ExpectedReadinessV1::Blocked, false)
            );

            let remaining_model = corpus
                .caps
                .total_model_requests
                .saturating_sub(prep_requests_admitted);

            let (accepted, authority_fault, bundle_leak, bundle_digest, coding_used, tool_used) =
                if skip_coding || prep_unknown_hold || prep_quality_failure || remaining_model == 0
                {
                    (false, false, false, None, 0, 0)
                } else if let Some(draft) = ready_draft_opt {
                    let (_val, bundle) = seal_draft_with_corpus_vectors(
                        &draft,
                        &final_auth,
                        &case.sealed_cli_vectors,
                    )?;
                    let b_dig = bundle.bundle_digest.clone();
                    let b_leak = check_oracle_leak(case, Some(&bundle), &[]);
                    let bundle_path = att_abs.join("bundle.json");
                    write_external_new_json(&bundle_path, Some(&_ws_fd), &[], &bundle)?;

                    let run_dir = att_abs.join("run");
                    let task_id = TaskId::new(format!(
                        "exp-{case_idx}-{}-{}",
                        arm_slug(arm),
                        &b_dig.as_str()[..10]
                    ))
                    .map_err(|e| format!("{e:?}"))?;

                    let (native_profile, native_worker, native_artifact, scripted, runner_factory) =
                        match &mut req.transport {
                            ExperimentTransportConfig::Native {
                                coding_profile_path,
                                worker_path,
                                artifact_path,
                            } => (
                                Some(coding_profile_path.clone()),
                                Some(worker_path.clone()),
                                Some(artifact_path.clone()),
                                None::<Vec<String>>,
                                #[cfg(test)]
                                None,
                                #[cfg(not(test))]
                                None::<()>,
                            ),
                            #[cfg(test)]
                            ExperimentTransportConfig::Controlled {
                                fixture,
                                custom_runner_factory,
                                ..
                            } => {
                                let k = format!("{}::{}", case.id, arm_slug(arm));
                                let replies = fixture
                                    .arm_scripts
                                    .get(&k)
                                    .map(|s| s.coding_replies.clone())
                                    .unwrap_or_default();
                                let make = custom_runner_factory.as_mut().map(|f| f());
                                (None, None, None, Some(replies), make)
                            }
                        };

                    #[cfg(not(test))]
                    let _ = (&scripted, &runner_factory);
                    let res = execute_contract_coding_attempt(CodingAttemptRequest {
                        task_id,
                        bundle_path,
                        workspace_path: ws_abs,
                        run_dir,
                        catalog: req.catalog.clone(),
                        dev_checks_enabled: dev_checks,
                        architecture_scaffold: arch_scaffold,
                        prep_requests_admitted,
                        total_model_cap: corpus.caps.total_model_requests,
                        max_tool_calls: corpus.caps.admitted_tool_effects,
                        evidence_origin,
                        profile_path: native_profile,
                        worker_path: native_worker,
                        artifact_path: native_artifact,
                        #[cfg(test)]
                        scripted_replies: scripted,
                        predecessor_run_dir: None,
                        predecessor_task_id: None,
                        #[cfg(test)]
                        custom_verifier_runner: runner_factory,
                        #[cfg(test)]
                        allowed_metadata_roots: vec![PathBuf::from("/workspace"), att_abs.clone()],
                    })?;
                    let auth_fault = res.outcome == "integrity_failure"
                        || (res.accepted
                            && (!res.report_verified || res.unknown_or_open_reservations > 0));
                    (
                        res.accepted && !auth_fault,
                        auth_fault,
                        b_leak,
                        Some(b_dig),
                        res.model_requests_used as u32,
                        res.tool_calls_used as u32,
                    )
                } else {
                    (false, false, false, None, 0, 0)
                };

            attempts.push(ExperimentAttemptResultV1 {
                case_id: BoundedText::new(case.id.clone()).map_err(|e| format!("{e:?}"))?,
                slice,
                arm,
                raw_output_digest: raw_digest,
                clarified_output_digest: clar_digest,
                raw_declared_ready,
                raw_readiness_matched,
                prep_unknown_hold,
                prep_quality_failure,
                prep_requests_admitted,
                coding_model_requests_used: coding_used,
                total_admitted_tool_effects: tool_used,
                accepted,
                authority_fault,
                oracle_leak: prep_leak || bundle_leak,
                bundle_digest,
            });
        }
    }

    let annotation_template =
        build_annotation_template(&req.campaign_id, &corpus_sha, &config_digest, &attempts)?;
    write_external_new_json(
        &out_abs.join("annotation-template.json"),
        None,
        &[],
        &annotation_template,
    )?;

    let receipt = evaluate_promotion_gate(
        &req.campaign_id,
        &corpus_sha,
        &config_digest,
        evidence_origin,
        diag_passed,
        &attempts,
        annotations_opt.as_ref(),
    )?;
    let report = ExperimentReportV1 {
        schema_version: CONTRACT_SCHEMA_VERSION,
        campaign_id: BoundedText::new(req.campaign_id.clone()).map_err(|e| format!("{e:?}"))?,
        corpus_sha256: corpus_sha,
        config_digest,
        evidence_origin,
        diagnostic_gate,
        attempts,
        promotion_receipt: receipt.clone(),
    };
    write_external_new_json(&out_abs.join("promotion-receipt.json"), None, &[], &receipt)?;
    write_external_new_json(&out_abs.join("experiment-report.json"), None, &[], &report)?;
    Ok(report)
}

fn build_expected_outputs_map(
    attempts: &[ExperimentAttemptResultV1],
) -> BTreeMap<(String, ExperimentArmV1, AnnotationStageV1), Digest> {
    let mut expected_outputs = BTreeMap::new();
    for att in attempts {
        expected_outputs.insert(
            (
                att.case_id.as_str().to_string(),
                att.arm,
                AnnotationStageV1::Raw,
            ),
            att.raw_output_digest.clone(),
        );
        if let Some(clar_dig) = &att.clarified_output_digest {
            expected_outputs.insert(
                (
                    att.case_id.as_str().to_string(),
                    att.arm,
                    AnnotationStageV1::Clarified,
                ),
                clar_dig.clone(),
            );
        }
    }
    expected_outputs
}

fn build_annotation_template(
    campaign_id: &str,
    corpus_sha256: &Digest,
    config_digest: &Digest,
    attempts: &[ExperimentAttemptResultV1],
) -> Result<ContractSemanticAnnotationSetV1, String> {
    let reviewer = BoundedText::new("UNREVIEWED_TEMPLATE").map_err(|e| format!("{e:?}"))?;
    let notes = BoundedText::new(
        "Fill independent semantic review; replace reviewer with non-model identifier.",
    )
    .map_err(|e| format!("{e:?}"))?;
    let mut annotations = Vec::with_capacity(attempts.len().saturating_mul(2));
    for att in attempts {
        annotations.push(CaseSemanticAnnotationV1 {
            case_id: att.case_id.clone(),
            arm: att.arm,
            stage: AnnotationStageV1::Raw,
            raw_output_digest: att.raw_output_digest.clone(),
            clause_complete: false,
            silent_material_assumptions: 0,
            unjustified_block: false,
            critical_false_ready: false,
            reviewer: reviewer.clone(),
            notes: notes.clone(),
        });
        if let Some(clar_dig) = &att.clarified_output_digest {
            annotations.push(CaseSemanticAnnotationV1 {
                case_id: att.case_id.clone(),
                arm: att.arm,
                stage: AnnotationStageV1::Clarified,
                raw_output_digest: clar_dig.clone(),
                clause_complete: false,
                silent_material_assumptions: 0,
                unjustified_block: false,
                critical_false_ready: false,
                reviewer: reviewer.clone(),
                notes: notes.clone(),
            });
        }
    }
    Ok(ContractSemanticAnnotationSetV1 {
        schema_version: CONTRACT_SCHEMA_VERSION,
        campaign_id: BoundedText::new(campaign_id).map_err(|e| format!("campaign_id: {e:?}"))?,
        corpus_sha256: corpus_sha256.clone(),
        config_digest: config_digest.clone(),
        annotations,
    })
}

fn retained_preparation_state(
    dir: &Path,
    typed: bool,
    case: &FrozenCorpusCaseV1,
    clarified: bool,
) -> Result<(bool, bool, bool), String> {
    if typed {
        let (_, ledger, _) =
            read_external_json::<ProposeSessionLedgerV1>(&dir.join("propose-ledger.json"), None)?;
        let draft_file = dir.join("draft.json");
        let mut ready = false;
        if draft_file.exists() {
            let (_, draft, _) = read_external_json::<ContractDraftV1>(&draft_file, None)?;
            if draft.raw_request.as_str() != case.raw_request
                || draft.raw_request_sha256 != case.raw_request_sha256
            {
                return Err("retained preparation draft request mismatch".into());
            }
            let snapshot = draft
                .baseline
                .files
                .iter()
                .map(|f| (f.path.as_str().to_owned(), f.sha256.clone()))
                .collect();
            let auth = build_host_auth_from_case(
                case,
                if clarified {
                    case.frozen_clarification.as_deref()
                } else {
                    None
                },
                snapshot,
            );
            let (derived, _) = validate_against_auth_with_mode(&draft, Some(&auth), true)?;
            let (_, captured, _) = read_external_json::<ContractValidationReportV1>(
                &dir.join("validation-report.json"),
                None,
            )?;
            if derived != captured {
                return Err("retained preparation validation differs from rederived result".into());
            }
            ready = derived.declared_ready;
        }
        Ok((ready, ledger.unknown_hold, ledger.quality_failure))
    } else {
        let (_, ledger, _) =
            read_external_json::<ProsePrepLedgerV1>(&dir.join("prose-ledger.json"), None)?;
        let last = ledger
            .stages
            .last()
            .ok_or("empty prose preparation ledger")?;
        let (ready, quality) = match last
            .raw_output
            .as_ref()
            .map(|s| parse_prose_readiness_protocol(s.as_str()))
        {
            Some(Ok(Ok(_))) => (true, false),
            Some(Ok(Err(_))) => (false, false),
            Some(Err(_)) => (false, true),
            None => (false, last.outcome.as_str() == "rejected"),
        };
        if ready != ledger.declared_ready
            || quality != ledger.quality_failure
            || (last.outcome.as_str() == "uncertain") != ledger.unknown_hold
        {
            return Err("prose preparation state differs from raw protocol".into());
        }
        Ok((ready, ledger.unknown_hold, quality))
    }
}

fn verify_campaign_run_bindings(
    settings: &ContractSettingsV1,
    cfg: &ExperimentCampaignConfigV1,
    dev_checks: bool,
    architecture_scaffold: bool,
    verified_native_paths: &mut BTreeMap<PathBuf, Digest>,
) -> Result<(), String> {
    if settings
        .catalog
        .digest()
        .map_err(|e| format!("catalog: {e:?}"))?
        != cfg.catalog_digest
        || settings.dev_checks_enabled != dev_checks
        || settings.architecture_scaffold != architecture_scaffold
        || settings.prep_requests_admitted + settings.remaining_coding_model_requests
            != cfg.caps.total_model_requests
        || settings.task.data().budget.max_tool_calls.get()
            != u64::from(cfg.caps.admitted_tool_effects)
    {
        return Err("retained run catalog/arm/budget binding mismatch".into());
    }
    if cfg.evidence_origin == EvidenceOriginV1::NativeTeamSession {
        let profile = settings
            .profile
            .as_ref()
            .ok_or("native settings missing profile")?;
        if Some(profile.digest().map_err(|e| format!("profile: {e:?}"))?)
            != cfg.coding_profile_digest
        {
            return Err("native coding profile binding mismatch".into());
        }
        for (path, expected) in [
            (&settings.worker, &cfg.worker_sha256),
            (&settings.artifact, &cfg.artifact_sha256),
        ] {
            let (_, abs) = open_external_nofollow(
                path.as_deref()
                    .ok_or("native settings missing binary path")?,
                false,
            )?;
            if verified_native_paths.get(&abs) == expected.as_ref() {
                continue;
            }
            let actual =
                sha256_file(&abs).map_err(|e| format!("native retained file hash: {e}"))?;
            if expected.as_ref().map(Digest::as_str) != Some(actual.as_str()) {
                return Err("native retained worker/artifact binding mismatch".into());
            }
            verified_native_paths.insert(abs, Digest::new(actual).map_err(|e| format!("{e:?}"))?);
        }
    }
    Ok(())
}

fn audit_native_preparation(
    dir: &Path,
    cfg: &ExperimentCampaignConfigV1,
    typed: bool,
    admitted: u32,
    stage_digests: &[(Option<Digest>, Option<Digest>)],
    unknown_hold: bool,
) -> Result<(), String> {
    let (_, inputs, _) =
        read_external_json::<NativePreparationInputs>(&dir.join("native-preparation.json"), None)?;
    if Some(inputs.profile.digest().map_err(|e| format!("{e:?}"))?) != cfg.team_profile_digest
        || inputs.max_requests == 0
        || inputs.max_requests > u32::from(cfg.caps.preparation_requests)
        || stage_digests.len() != admitted as usize
    {
        return Err("native preparation source profile/count binding mismatch".into());
    }
    let expected_principal = if typed {
        "contract-propose"
    } else {
        "contract-prose-prep"
    };
    if inputs.principal_id.as_str() != expected_principal {
        return Err("native preparation principal mismatch".into());
    }
    let mut cases = Vec::new();
    for (idx, (prompt_sha, _)) in stage_digests.iter().enumerate() {
        let (_, input, _) = read_external_json::<NativePreparationCase>(
            &dir.join(format!("stage-{idx}-input.json")),
            None,
        )?;
        let expected_id = if typed {
            format!("propose-stage-{idx}")
        } else {
            format!("prose-prep-{idx}")
        };
        if input.id.as_str() != expected_id
            || prompt_sha
                .as_ref()
                .is_some_and(|sha| sha != &content_digest(input.context.as_str().as_bytes()))
        {
            return Err("native preparation exact input binding mismatch".into());
        }
        cases.push(ComparisonCase {
            id: input.id,
            role: LocalRole::Reasoner,
            output_schema: LocalOutputSchema::Text,
            context: input.context,
        });
    }
    let audit = audit_comparison_session(
        &dir.join(format!("comparison-{}.journal", inputs.session_id.as_str())),
        &ComparisonAuditExpectation {
            session_id: &inputs.session_id,
            task_id: &inputs.task_id,
            principal_id: &inputs.principal_id,
            profile: &inputs.profile,
            max_requests: inputs.max_requests,
            cases: &cases,
        },
    )
    .map_err(|e| format!("native preparation journal audit: {e}"))?;
    if audit.admitted != admitted || (audit.unknown + audit.unfinished > 0) != unknown_hold {
        return Err("native preparation journal counts/unknown state mismatch".into());
    }
    for (entry, (_, expected_raw)) in audit.requests.iter().zip(stage_digests) {
        let raw = match &entry.outcome {
            ComparisonAuditOutcome::Receipt(receipt) => match &receipt.data().output {
                metron_protocol::ReceiptOutput::LocalModel { observation } => {
                    Some(content_digest(observation.content.as_str().as_bytes()))
                }
                metron_protocol::ReceiptOutput::Failure { .. } => None,
                _ => return Err("native preparation has non-model receipt".into()),
            },
            _ => None,
        };
        if raw != *expected_raw {
            return Err("native preparation raw output differs from journal receipt".into());
        }
    }
    Ok(())
}

fn verify_prep_ledger_on_disk(
    prep_dir: &Path,
    typed: bool,
    case_id: &str,
    expected_digest: &Digest,
    cfg: &ExperimentCampaignConfigV1,
) -> Result<u32, String> {
    let (_, dir_abs) = open_external_nofollow(prep_dir, true)?;
    if typed {
        let (_, ledger, _) = read_external_json::<ProposeSessionLedgerV1>(
            &dir_abs.join("propose-ledger.json"),
            None,
        )?;
        if ledger.schema_version != CONTRACT_SCHEMA_VERSION {
            return Err("unsupported propose-ledger schema_version".into());
        }
        if ledger.evidence_origin != cfg.evidence_origin {
            return Err(format!(
                "propose-ledger evidence_origin mismatch for '{case_id}'"
            ));
        }
        if cfg.evidence_origin == EvidenceOriginV1::NativeTeamSession
            && (ledger.source_coding_profile_digest != cfg.coding_profile_digest
                || ledger.team_profile_digest != cfg.team_profile_digest)
        {
            return Err(format!(
                "propose-ledger profile digests mismatch campaign config for '{case_id}'"
            ));
        }
        if ledger.admitted_prep_requests > ledger.max_prep_requests
            || u32::from(ledger.admitted_prep_requests) > u32::from(cfg.caps.preparation_requests)
        {
            return Err(format!(
                "propose-ledger admitted_prep_requests exceeds cap for '{case_id}'"
            ));
        }
        let actual_dig = stage_digest_from_ledger(&ledger);
        if &actual_dig != expected_digest {
            return Err(format!(
                "propose-ledger digest mismatch vs experiment report for '{case_id}'"
            ));
        }
        if cfg.evidence_origin == EvidenceOriginV1::NativeTeamSession {
            audit_native_preparation(
                &dir_abs,
                cfg,
                true,
                u32::from(ledger.admitted_prep_requests),
                &ledger
                    .stages
                    .iter()
                    .map(|s| (Some(s.prompt_sha256.clone()), s.raw_response_sha256.clone()))
                    .collect::<Vec<_>>(),
                ledger.unknown_hold,
            )?;
        }
        Ok(u32::from(ledger.admitted_prep_requests))
    } else {
        let (_, ledger, _) =
            read_external_json::<ProsePrepLedgerV1>(&dir_abs.join("prose-ledger.json"), None)?;
        if ledger.schema_version != CONTRACT_SCHEMA_VERSION {
            return Err("unsupported prose-ledger schema_version".into());
        }
        if ledger.case_id.as_str() != case_id {
            return Err(format!(
                "prose-ledger case_id '{}' mismatch expected '{case_id}'",
                ledger.case_id.as_str()
            ));
        }
        if ledger.evidence_origin != cfg.evidence_origin {
            return Err(format!(
                "prose-ledger evidence_origin mismatch for '{case_id}'"
            ));
        }
        if cfg.evidence_origin == EvidenceOriginV1::NativeTeamSession
            && (ledger.source_coding_profile_digest != cfg.coding_profile_digest
                || ledger.team_profile_digest != cfg.team_profile_digest)
        {
            return Err(format!(
                "prose-ledger profile digests mismatch campaign config for '{case_id}'"
            ));
        }
        if ledger.admitted_prep_requests > u32::from(ledger.max_prep_requests)
            || ledger.admitted_prep_requests > u32::from(cfg.caps.preparation_requests)
        {
            return Err(format!(
                "prose-ledger admitted_prep_requests exceeds cap for '{case_id}'"
            ));
        }
        if &ledger.raw_output_sha256 != expected_digest {
            return Err(format!(
                "prose-ledger raw_output_sha256 mismatch vs experiment report for '{case_id}'"
            ));
        }
        if cfg.evidence_origin == EvidenceOriginV1::NativeTeamSession {
            audit_native_preparation(
                &dir_abs,
                cfg,
                false,
                ledger.admitted_prep_requests,
                &ledger
                    .stages
                    .iter()
                    .map(|s| (None, s.raw_output_sha256.clone()))
                    .collect::<Vec<_>>(),
                ledger.unknown_hold,
            )?;
        }
        Ok(ledger.admitted_prep_requests)
    }
}

/// Offline read-only finalizer that re-verifies campaign config, corpus/freeze SHAs, diagnostic
/// and 48 pilot directories on disk, and external Sol annotations without re-running models/tools.
pub(crate) fn finalize_experiment_campaign(
    campaign_dir: &Path,
    annotations_path: &Path,
    corpus_path: Option<&Path>,
    corpus_freeze_path: Option<&Path>,
) -> Result<(ExperimentReportV1, ContractPromotionReceiptV1), String> {
    let (_camp_fd, camp_abs) = open_external_nofollow(campaign_dir, true)?;
    let (_, cfg, _) = read_external_json::<ExperimentCampaignConfigV1>(
        &camp_abs.join("campaign-config.json"),
        None,
    )?;
    let (_, diag_report, _) = read_external_json::<DiagnosticGateReportV1>(
        &camp_abs.join("diagnostic-gate-report.json"),
        None,
    )?;
    let (_, mut report, _) =
        read_external_json::<ExperimentReportV1>(&camp_abs.join("experiment-report.json"), None)?;
    let (_, annotations, _) =
        read_external_json::<ContractSemanticAnnotationSetV1>(annotations_path, None)?;

    if cfg.schema_version != CONTRACT_SCHEMA_VERSION
        || diag_report.schema_version != CONTRACT_SCHEMA_VERSION
        || report.schema_version != CONTRACT_SCHEMA_VERSION
    {
        return Err("unsupported campaign schema_version".into());
    }
    if cfg.corpus_sha256.as_str() != EXPECTED_INDEPENDENT_CORPUS_SHA256
        || cfg.corpus_freeze_sha256.as_str() != EXPECTED_INDEPENDENT_CORPUS_FREEZE_SHA256
    {
        return Err(
            "campaign config does not bind expected frozen v3 corpus and freeze SHAs".into(),
        );
    }
    let corpus_opt = match (corpus_path, corpus_freeze_path) {
        (Some(cp), Some(fp)) => {
            let (_, c_bytes, _) = super::io::read_external_bounded_bytes(cp, None, MAX_INTERNAL)?;
            let (_, f_bytes, _) = super::io::read_external_bounded_bytes(fp, None, MAX_INTERNAL)?;
            let freeze_sha = content_digest(&f_bytes);
            let (corpus, _freeze, corpus_sha) = load_and_verify_corpus_bytes(&c_bytes, &f_bytes)?;
            if corpus_sha != cfg.corpus_sha256 || freeze_sha != cfg.corpus_freeze_sha256 {
                return Err(
                    "provided --corpus / --corpus-freeze SHA256 differs from campaign config"
                        .into(),
                );
            }
            if corpus.arm_order != cfg.arm_order || corpus.caps != cfg.caps {
                return Err("provided corpus arm_order/caps differs from campaign config".into());
            }
            Some(corpus)
        }
        (None, None) => return Err("finalization requires --corpus and --corpus-freeze".into()),
        _ => {
            return Err(
                "pass both --corpus and --corpus-freeze together when verifying against corpus"
                    .into(),
            );
        }
    };

    let expected_cfg_dig = recompute_campaign_config_digest(&cfg)?;
    if expected_cfg_dig != cfg.config_digest {
        return Err("campaign-config.json config_digest verification failed".into());
    }
    if cfg.public_harness_preamble.as_str() != PUBLIC_HARNESS_PREAMBLE
        || cfg.objective_projection_recipe.as_str() != OBJECTIVE_PROJECTION_RECIPE_TAG
    {
        return Err("campaign config harness preamble or objective recipe tag mismatch".into());
    }
    if cfg.campaign_id != report.campaign_id
        || cfg.corpus_sha256 != report.corpus_sha256
        || cfg.config_digest != report.config_digest
        || cfg.evidence_origin != report.evidence_origin
        || report.diagnostic_gate != diag_report
    {
        return Err(
            "experiment-report.json disagrees with persisted campaign config or diagnostic report"
                .into(),
        );
    }
    if cfg.evidence_origin == EvidenceOriginV1::NativeTeamSession
        && (cfg.coding_profile_digest.is_none()
            || cfg.team_profile_digest.is_none()
            || cfg.worker_sha256.is_none()
            || cfg.artifact_sha256.is_none())
    {
        return Err(
            "native campaign config is missing pinned profile/worker/artifact digests".into(),
        );
    }

    // Verify diagnostic gate counts and persisted diagnostic artifacts on disk.
    let (_diag_fd, diag_abs) = open_external_nofollow(&camp_abs.join("diagnostic"), true)?;
    if diag_report.results.len() != 4 {
        return Err(format!(
            "expected 4 diagnostic cases, found {}",
            diag_report.results.len()
        ));
    }
    let mut verified_native_paths = BTreeMap::new();
    let (mut calc_diag_new, mut calc_diag_existing) = (0u32, 0u32);
    for (idx, d_res) in diag_report.results.iter().enumerate() {
        if let Some(corpus) = &corpus_opt {
            let expected = &corpus.diagnostic_cases[idx];
            if d_res.case_id.as_str() != expected.id
                || d_res.slice != slice_for_workflow(expected.workflow)
            {
                return Err(format!(
                    "diagnostic case {idx} identity/slice mismatch vs corpus"
                ));
            }
        }
        if d_res.accepted {
            match d_res.slice {
                CorpusSliceV1::New => calc_diag_new += 1,
                CorpusSliceV1::Existing => calc_diag_existing += 1,
            }
        }
        let case_dir = diag_abs.join(format!("{idx:02}-{}", d_res.case_id.as_str()));
        let (_, case_abs) = open_external_nofollow(&case_dir, true)?;
        let (ws_fd, _) = open_external_nofollow(&case_abs.join("workspace"), true)?;
        let (_, bundle, bundle_sha) =
            read_external_json::<ContractBundleV1>(&case_abs.join("bundle.json"), Some(&ws_fd))?;
        if bundle_body_digest(&bundle.body)? != bundle.bundle_digest {
            return Err(format!(
                "diagnostic case '{}' bundle digest invalid",
                d_res.case_id.as_str()
            ));
        }
        let run_dir = case_abs.join("run");
        let report_file = run_dir.join("contract-report.json");
        let (_, c_settings, _) = read_external_json::<ContractSettingsV1>(
            &run_dir.join("contract-settings.json"),
            Some(&ws_fd),
        )?;
        verify_campaign_run_bindings(
            &c_settings,
            &cfg,
            true,
            d_res.slice == CorpusSliceV1::New,
            &mut verified_native_paths,
        )?;
        let (model_admitted, tool_admitted, hold) =
            audit_retained_admissions(&run_dir, &c_settings)?;
        if model_admitted != d_res.model_requests_used
            || tool_admitted != d_res.tool_calls_used
            || (d_res.accepted && hold)
        {
            return Err("diagnostic retained admission counts/hold mismatch".into());
        }
        if d_res.accepted || report_file.exists() {
            let (_, c_report, _) =
                read_external_json::<ContractReportV1>(&report_file, Some(&ws_fd))?;
            let run_audit = verify_report_against_run_dir_historical(
                &c_report,
                &bundle,
                &bundle_sha,
                &run_dir,
                Some(&ws_fd),
            )?;
            if run_audit.recorded_host_executable_sha256 != cfg.host_executable_sha256
                || c_settings.checker_implementation_digest != cfg.checker_impl_digest
                || c_settings.evidence_origin != cfg.evidence_origin
                || c_report.body.accepted != d_res.accepted
            {
                return Err(format!(
                    "diagnostic case '{}' run settings/host/acceptance mismatch",
                    d_res.case_id.as_str()
                ));
            }
        }
    }
    if calc_diag_new != diag_report.new_accepted
        || calc_diag_existing != diag_report.existing_accepted
        || diag_report.passed != (calc_diag_new >= 1 && calc_diag_existing >= 1)
    {
        return Err("diagnostic-gate-report.json summary counts do not match results".into());
    }

    // Verify all 48 pilot attempt directories, ledgers, and coding artifacts on disk.
    let (_pilot_fd, pilot_abs) = open_external_nofollow(&camp_abs.join("pilot"), true)?;
    if report.attempts.len() != 48 {
        return Err(format!(
            "expected 48 pilot attempts, found {}",
            report.attempts.len()
        ));
    }
    let mut case_order: Vec<String> = Vec::with_capacity(12);
    let mut case_idx_map: BTreeMap<String, usize> = BTreeMap::new();
    let mut seen_pairs: BTreeSet<(String, ExperimentArmV1)> = BTreeSet::new();
    for att in &report.attempts {
        let cid = att.case_id.as_str().to_string();
        if !seen_pairs.insert((cid.clone(), att.arm)) {
            return Err(format!(
                "duplicate attempt ({cid}, {}) in experiment report",
                arm_slug(att.arm)
            ));
        }
        case_idx_map.entry(cid.clone()).or_insert_with(|| {
            let next_idx = case_order.len();
            case_order.push(cid);
            next_idx
        });
    }
    if case_order.len() != 12 || seen_pairs.len() != 48 {
        return Err("experiment report does not cover 12 cases x 4 arms (48 attempts)".into());
    }
    if let Some(corpus) = &corpus_opt {
        if corpus.cases.len() != case_order.len()
            || !corpus
                .cases
                .iter()
                .zip(case_order.iter())
                .all(|(c, cid)| &c.id == cid)
        {
            return Err("pilot case ordering does not match frozen corpus".into());
        }
        let slice_by_id: BTreeMap<&str, CorpusSliceV1> = corpus
            .cases
            .iter()
            .map(|c| (c.id.as_str(), slice_for_workflow(c.workflow)))
            .collect();
        for att in &report.attempts {
            if slice_by_id.get(att.case_id.as_str()) != Some(&att.slice) {
                return Err(format!(
                    "attempt '{}' slice disagrees with corpus",
                    att.case_id.as_str()
                ));
            }
        }
    }

    for att in &report.attempts {
        let case_id = att.case_id.as_str();
        let case_idx = case_idx_map[case_id];
        let attempt_dir = pilot_abs.join(format!("{case_idx:02}-{case_id}-{}", arm_slug(att.arm)));
        let (_, att_abs) = open_external_nofollow(&attempt_dir, true)?;
        let typed = matches!(
            att.arm,
            ExperimentArmV1::TypedPlain | ExperimentArmV1::TypedArchitecture
        );

        let frozen_case = &corpus_opt.as_ref().ok_or("missing required corpus")?.cases[case_idx];
        let mut observed_oracle_leak = false;
        for (clarified, prep_name) in [(false, "prep-raw"), (true, "prep-clarified")] {
            let prep_path = att_abs.join(prep_name);
            if !prep_path.exists() {
                continue;
            }
            let stage_count = if typed {
                read_external_json::<ProposeSessionLedgerV1>(
                    &prep_path.join("propose-ledger.json"),
                    None,
                )?
                .1
                .admitted_prep_requests as usize
            } else {
                read_external_json::<ProsePrepLedgerV1>(&prep_path.join("prose-ledger.json"), None)?
                    .1
                    .admitted_prep_requests as usize
            };
            let outputs: Vec<String> = if typed {
                read_external_json::<ProposeSessionLedgerV1>(
                    &prep_path.join("propose-ledger.json"),
                    None,
                )?
                .1
                .stages
                .iter()
                .filter_map(|s| s.raw_response.as_ref().map(|s| s.as_str().to_owned()))
                .collect()
            } else {
                read_external_json::<ProsePrepLedgerV1>(&prep_path.join("prose-ledger.json"), None)?
                    .1
                    .stages
                    .iter()
                    .filter_map(|s| s.raw_output.as_ref().map(|s| s.as_str().to_owned()))
                    .collect()
            };
            observed_oracle_leak |= check_oracle_leak(
                frozen_case,
                None,
                &outputs.iter().map(String::as_str).collect::<Vec<_>>(),
            );
            if !clarified && frozen_case.frozen_clarification.is_some() && stage_count > 1 {
                return Err("raw clarification stage exceeded one request".into());
            }
            if cfg.evidence_origin == EvidenceOriginV1::NativeTeamSession {
                for stage in 0..stage_count {
                    let (_, input, _) = read_external_json::<NativePreparationCase>(
                        &prep_path.join(format!("stage-{stage}-input.json")),
                        None,
                    )?;
                    if !input.context.as_str().contains(&frozen_case.raw_request)
                        || !input.context.as_str().contains(PUBLIC_HARNESS_PREAMBLE)
                        || !input.context.as_str().contains(
                            if matches!(
                                att.arm,
                                ExperimentArmV1::ProseArchitecture
                                    | ExperimentArmV1::TypedArchitecture
                            ) {
                                "architecture_scaffold=on"
                            } else {
                                "architecture_scaffold=off"
                            },
                        )
                        || (clarified
                            && frozen_case
                                .frozen_clarification
                                .as_ref()
                                .is_some_and(|answer| !input.context.as_str().contains(answer)))
                        || check_oracle_leak(frozen_case, None, &[input.context.as_str()])
                    {
                        return Err(
                            "native preparation input differs from frozen case or leaks oracle"
                                .into(),
                        );
                    }
                }
            }
        }
        let raw_state =
            retained_preparation_state(&att_abs.join("prep-raw"), typed, frozen_case, false)?;
        let final_state = if att.clarified_output_digest.is_some() {
            retained_preparation_state(&att_abs.join("prep-clarified"), typed, frozen_case, true)?
        } else {
            raw_state
        };
        let expected_raw_match = matches!(
            (frozen_case.initial_expected_readiness, raw_state.0),
            (ExpectedReadinessV1::Ready, true) | (ExpectedReadinessV1::Blocked, false)
        );
        if att.raw_declared_ready != raw_state.0
            || att.raw_readiness_matched != expected_raw_match
            || att.prep_unknown_hold != final_state.1
            || att.prep_quality_failure != final_state.2
            || (att.accepted && (!final_state.0 || final_state.1 || final_state.2))
        {
            return Err("experiment preparation state disagrees with retained raw evidence".into());
        }
        let mut observed_prep = verify_prep_ledger_on_disk(
            &att_abs.join("prep-raw"),
            typed,
            case_id,
            &att.raw_output_digest,
            &cfg,
        )?;
        let clar_dir = att_abs.join("prep-clarified");
        if let Some(clar_dig) = &att.clarified_output_digest {
            observed_prep = observed_prep.saturating_add(verify_prep_ledger_on_disk(
                &clar_dir, typed, case_id, clar_dig, &cfg,
            )?);
        } else if clar_dir.exists() {
            return Err(format!(
                "unexpected prep-clarified dir for '{case_id}' ({}) when clarified_output_digest is None",
                arm_slug(att.arm)
            ));
        }

        if observed_prep != att.prep_requests_admitted
            || observed_prep > u32::from(cfg.caps.preparation_requests)
        {
            return Err(format!(
                "prep requests on disk ({observed_prep}) disagree with report ({}) or cap for '{case_id}' ({})",
                att.prep_requests_admitted,
                arm_slug(att.arm)
            ));
        }

        if !diag_report.passed && att.bundle_digest.is_some() {
            return Err(format!(
                "pilot coded '{case_id}' ({}) despite failed diagnostic gate",
                arm_slug(att.arm)
            ));
        }

        let bundle_path = att_abs.join("bundle.json");
        let run_dir = att_abs.join("run");
        match &att.bundle_digest {
            None => {
                if att.accepted
                    || att.coding_model_requests_used != 0
                    || att.total_admitted_tool_effects != 0
                    || bundle_path.exists()
                    || run_dir.exists()
                {
                    return Err(format!(
                        "uncoded attempt '{case_id}' ({}) has unexpected acceptance, budget, bundle, or run dir",
                        arm_slug(att.arm)
                    ));
                }
            }
            Some(expected_b_dig) => {
                let (ws_fd, _) = open_external_nofollow(&att_abs.join("workspace"), true)?;
                let (_, bundle, bundle_sha) =
                    read_external_json::<ContractBundleV1>(&bundle_path, Some(&ws_fd))?;
                observed_oracle_leak |= check_oracle_leak(frozen_case, Some(&bundle), &[]);
                if bundle.body.raw_request.as_str() != frozen_case.raw_request
                    || bundle.body.raw_request_sha256 != frozen_case.raw_request_sha256
                    || bundle.body.baseline.selected_cli_binary.as_str()
                        != frozen_case.selected_binary
                {
                    return Err("pilot bundle differs from frozen request/binary".into());
                }
                if &bundle.bundle_digest != expected_b_dig
                    || &bundle_body_digest(&bundle.body)? != expected_b_dig
                {
                    return Err(format!(
                        "pilot bundle digest mismatch for '{case_id}' ({})",
                        arm_slug(att.arm)
                    ));
                }
                if observed_prep.saturating_add(att.coding_model_requests_used)
                    > cfg.caps.total_model_requests
                    || att.total_admitted_tool_effects > cfg.caps.admitted_tool_effects
                {
                    return Err(format!(
                        "pilot model/tool budget exceeded cap for '{case_id}' ({})",
                        arm_slug(att.arm)
                    ));
                }
                let report_file = run_dir.join("contract-report.json");
                let settings_file = run_dir.join("contract-settings.json");
                let (_, c_settings, _) =
                    read_external_json::<ContractSettingsV1>(&settings_file, Some(&ws_fd))?;
                verify_campaign_run_bindings(
                    &c_settings,
                    &cfg,
                    matches!(
                        att.arm,
                        ExperimentArmV1::ProseArchitecture | ExperimentArmV1::TypedArchitecture
                    ),
                    att.slice == CorpusSliceV1::New
                        && matches!(
                            att.arm,
                            ExperimentArmV1::ProseArchitecture | ExperimentArmV1::TypedArchitecture
                        ),
                    &mut verified_native_paths,
                )?;
                let (model_admitted, tool_admitted, hold) =
                    audit_retained_admissions(&run_dir, &c_settings)?;
                if model_admitted != u64::from(att.coding_model_requests_used)
                    || tool_admitted != u64::from(att.total_admitted_tool_effects)
                    || (att.accepted && hold)
                {
                    return Err("pilot retained admission counts/hold mismatch".into());
                }
                if att.accepted || report_file.exists() {
                    if att.accepted && att.authority_fault {
                        return Err(format!(
                            "attempt '{case_id}' ({}) marked both accepted and authority_fault",
                            arm_slug(att.arm)
                        ));
                    }
                    let (_, c_report, _) =
                        read_external_json::<ContractReportV1>(&report_file, Some(&ws_fd))?;
                    let run_audit = verify_report_against_run_dir_historical(
                        &c_report,
                        &bundle,
                        &bundle_sha,
                        &run_dir,
                        Some(&ws_fd),
                    )?;
                    if run_audit.recorded_host_executable_sha256 != cfg.host_executable_sha256
                        || c_settings.checker_implementation_digest != cfg.checker_impl_digest
                        || c_settings.evidence_origin != cfg.evidence_origin
                        || c_settings.prep_requests_admitted != observed_prep
                        || c_report.body.accepted != att.accepted
                    {
                        return Err(format!(
                            "pilot run settings/host/acceptance mismatch on disk for '{case_id}' ({})",
                            arm_slug(att.arm)
                        ));
                    }
                } else {
                    if att.accepted {
                        return Err(format!(
                            "accepted attempt '{case_id}' ({}) missing contract-report.json",
                            arm_slug(att.arm)
                        ));
                    }
                    if settings_file.exists() {
                        let (_, c_settings, _) =
                            read_external_json::<ContractSettingsV1>(&settings_file, Some(&ws_fd))?;
                        let _ = validate_contract_settings_historical(&c_settings)?;
                        if c_settings.host_executable_sha256 != cfg.host_executable_sha256
                            || c_settings.checker_implementation_digest != cfg.checker_impl_digest
                            || c_settings.evidence_origin != cfg.evidence_origin
                        {
                            return Err(format!(
                                "failed pilot run settings mismatch for '{case_id}' ({})",
                                arm_slug(att.arm)
                            ));
                        }
                    }
                }
            }
        }
        if att.oracle_leak != observed_oracle_leak {
            return Err(
                "reported oracle leakage disagrees with retained preparation/bundle".into(),
            );
        }
    }

    // Fail closed if annotations fail schema, reviewer, campaign/corpus/config, or stage digest checks.
    let expected_outputs = build_expected_outputs_map(&report.attempts);
    verify_annotation_set(
        &annotations,
        cfg.campaign_id.as_str(),
        &cfg.corpus_sha256,
        &cfg.config_digest,
        &expected_outputs,
    )
    .map_err(|e| format!("annotation verification failed: {e}"))?;

    let finalized_receipt = evaluate_promotion_gate(
        cfg.campaign_id.as_str(),
        &cfg.corpus_sha256,
        &cfg.config_digest,
        cfg.evidence_origin,
        diag_report.passed,
        &report.attempts,
        Some(&annotations),
    )?;
    report.promotion_receipt = finalized_receipt.clone();

    write_external_new_json(
        &camp_abs.join("finalized-promotion-receipt.json"),
        None,
        &[],
        &finalized_receipt,
    )?;
    write_external_new_json(
        &camp_abs.join("finalized-experiment-report.json"),
        None,
        &[],
        &report,
    )?;
    Ok((report, finalized_receipt))
}

fn propose_transport_for_attempt(
    transport: &ExperimentTransportConfig,
    _case_id: &str,
    _arm: ExperimentArmV1,
    _clarified: bool,
) -> Result<ProposeTransportConfig, String> {
    match transport {
        ExperimentTransportConfig::Native {
            coding_profile_path,
            worker_path,
            artifact_path,
        } => Ok(ProposeTransportConfig::NativeTeam {
            coding_profile_path: coding_profile_path.clone(),
            worker_path: worker_path.clone(),
            artifact_path: artifact_path.clone(),
        }),
        #[cfg(test)]
        ExperimentTransportConfig::Controlled {
            fixture,
            evidence_origin,
            ..
        } => {
            let key = format!("{_case_id}::{}", arm_slug(_arm));
            let replies = fixture
                .arm_scripts
                .get(&key)
                .map(|s| {
                    if _clarified {
                        s.clarified_prep_replies.clone()
                    } else {
                        s.raw_prep_replies.clone()
                    }
                })
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| vec![super::propose::UNCERTAIN_TEST_SENTINEL.to_string()]);
            Ok(ProposeTransportConfig::InMemoryReplies {
                replies: VecDeque::from(replies),
                origin: *evidence_origin,
            })
        }
    }
}

fn stage_digest_from_ledger(ledger: &ProposeSessionLedgerV1) -> Digest {
    for rec in ledger.stages.iter().rev() {
        if let Some(dig) = &rec.raw_response_sha256 {
            return dig.clone();
        }
    }
    content_digest(ledger.session_id.as_str().as_bytes())
}

fn check_oracle_leak_in_ledger(case: &FrozenCorpusCaseV1, ledger: &ProposeSessionLedgerV1) -> bool {
    let texts: Vec<&str> = ledger
        .stages
        .iter()
        .filter_map(|s| s.raw_response.as_ref().map(|b| b.as_str()))
        .collect();
    check_oracle_leak(case, None, &texts)
}

#[cfg(test)]
mod focused_protocol_tests {
    use super::*;
    #[test]
    fn prose_readiness_is_explicit_and_guidance_is_utf8_byte_bounded() {
        assert!(parse_prose_readiness_protocol("looks ready").is_err());
        assert!(parse_prose_readiness_protocol("BLOCKED:").is_err());
        assert!(
            parse_prose_readiness_protocol("BLOCKED: need tie policy")
                .unwrap()
                .is_err()
        );
        assert_eq!(
            parse_prose_readiness_protocol("READY:").unwrap().unwrap(),
            ""
        );
        assert!(parse_prose_readiness_protocol(&format!("READY: {}", "é".repeat(16))).is_ok());
        assert!(parse_prose_readiness_protocol(&format!("READY: {}", "é".repeat(17))).is_err());
    }
    #[test]
    fn raw_reserves_two_clarified_requests_for_each_frozen_case() {
        let corpus: FrozenCorpusFileV1 = serde_json::from_str(include_str!(
            "../../../metron-testkit/fixtures/independent-corpus.json"
        ))
        .unwrap();
        for case in &corpus.cases {
            let raw_cap = raw_preparation_cap(case, corpus.caps.preparation_requests);
            assert!(raw_cap <= 3);
            if case.frozen_clarification.is_some() {
                assert_eq!(raw_cap, 1);
                assert_eq!(corpus.caps.preparation_requests - raw_cap, 2);
            }
        }
    }
}
