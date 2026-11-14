//! Host source authentication, declared readiness validation, checker binding checks,
//! and strict `ContractReportV1` authority verification.

use super::io::PredecessorInspection;
use super::types::*;
use metron_protocol::{
    AuditSnapshot, BoundedText, CodingVerificationSelection, Digest, JournalEvent, MAX_ENVELOPE,
    MAX_INTERNAL, StoreOperation, StorePort, StoreResult, TaskId, canonical_bytes,
    coding::CODING_TASK_VERSION, coding_runtime::CodingObservation, content_digest,
    domain::ReceiptOutput,
};
use metron_store_sqlite::SqliteStore;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    path::Path,
};

#[derive(Clone, Debug)]
pub(crate) struct HostAuthSource {
    pub kind: SourceKind,
    pub sha256: Digest,
    pub label: String,
    pub utf8_text: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct HostAuthContext {
    pub raw_request: String,
    pub raw_request_sha256: Digest,
    pub sources_by_id: BTreeMap<String, HostAuthSource>,
    pub workspace_snapshot: Option<BTreeMap<String, Digest>>,
    pub predecessor_bundle_digest: Option<Digest>,
    pub predecessor_inspection: Option<PredecessorInspection>,
}

fn push_finding(
    findings: &mut Vec<ValidationFindingV1>,
    code: &str,
    severity: FindingSeverity,
    message: &str,
) {
    if findings.len() >= MAX_FINDINGS {
        return;
    }
    let code_end = code.floor_char_boundary(code.len().min(64));
    let msg_end = message.floor_char_boundary(message.len().min(256));
    if let (Ok(code_bt), Ok(msg_bt)) = (
        BoundedText::new(&code[..code_end]),
        BoundedText::new(&message[..msg_end]),
    ) {
        findings.push(ValidationFindingV1 {
            code: code_bt,
            severity,
            message: msg_bt,
        });
    }
}

fn check_unique_ids<'a, I>(
    ids: I,
    kind: &str,
    findings: &mut Vec<ValidationFindingV1>,
) -> BTreeSet<String>
where
    I: Iterator<Item = &'a str>,
{
    let mut set = BTreeSet::new();
    for id in ids {
        let trimmed = id.trim();
        if trimmed.is_empty() || trimmed != id {
            push_finding(
                findings,
                "invalid_identifier",
                FindingSeverity::Error,
                &format!("{kind} identifier must be non-empty without leading/trailing spaces"),
            );
            continue;
        }
        if !set.insert(id.to_string()) {
            push_finding(
                findings,
                "duplicate_identifier",
                FindingSeverity::Error,
                &format!("duplicate {kind} identifier '{id}'"),
            );
        }
    }
    set
}

fn valid_binary_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['/', '\\', '\0', ' ', '\t', '\n', '\r'])
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

fn normalize_bundle_body(
    draft: &ContractDraftV1,
    draft_hash: &Digest,
    authenticated_sources: Vec<ContractSourceEntryV1>,
) -> ContractBundleBodyV1 {
    ContractBundleBodyV1 {
        schema_version: CONTRACT_SCHEMA_VERSION,
        contract_id: draft.contract_id.clone(),
        workflow: draft.workflow,
        draft_digest: draft_hash.clone(),
        raw_request: draft.raw_request.clone(),
        raw_request_sha256: draft.raw_request_sha256.clone(),
        sources: authenticated_sources,
        baseline: draft.baseline.clone(),
        compatibility: draft.compatibility.clone(),
        requirements: draft.requirements.clone(),
        checker_policy: draft.checker_policy.clone(),
        lineage: draft.lineage.clone(),
    }
}

/// Ordered exact check list `(id, checker)` expected by `ContractBundleV1`.
pub(crate) fn expected_check_specs_for_bundle(
    bundle: &ContractBundleV1,
) -> Vec<(String, &'static str)> {
    let mut specs = vec![
        (
            "metadata".to_string(),
            CHECKER_METADATA_SNAPSHOT_LOCAL_PATH_V1,
        ),
        ("build".to_string(), CHECKER_BUILD_CARGO_OFFLINE_LOCKED_V1),
        (
            "test".to_string(),
            CHECKER_TEST_CARGO_TEST_OFFLINE_LOCKED_V1,
        ),
    ];
    for fixed in &bundle.body.checker_policy.fixed_files {
        specs.push((
            format!("fixed:{}", fixed.id.as_str()),
            CHECKER_FIXED_FILE_V1,
        ));
    }
    for path in &bundle.body.compatibility.forbidden_files {
        specs.push((
            format!("forbidden:{}", path.as_str()),
            CHECKER_FORBIDDEN_FILE_V1,
        ));
    }
    for path in &bundle.body.compatibility.preserved_files {
        specs.push((
            format!("preserved:{}", path.as_str()),
            CHECKER_PRESERVED_FILE_V1,
        ));
    }
    if !bundle.body.compatibility.allowed_edit_paths.is_empty() {
        specs.push((
            "edit_scope".to_string(),
            CHECKER_METADATA_SNAPSHOT_LOCAL_PATH_V1,
        ));
    }
    for iface in &bundle.body.checker_policy.interface_checks {
        specs.push((
            format!("interface:{}", iface.id.as_str()),
            CHECKER_SOURCE_LITERAL_V1,
        ));
    }
    if bundle.body.checker_policy.cli_checker.is_some() {
        for idx in 0..bundle.body.checker_policy.cli_vectors.len() {
            specs.push((format!("cli:{idx}"), CHECKER_CLI_SELECTED_VECTORS_V1));
        }
    }
    specs
}

pub(crate) fn build_contract_report(
    bundle: &ContractBundleV1,
    task_digest: Digest,
    tree_digest: Digest,
    checker_implementation_digest: Digest,
    checks: Vec<ContractCheckResultV1>,
    verifier_cost: ContractVerifierCostV1,
) -> Result<ContractReportV1, String> {
    let evaluator_digest =
        expected_contract_evaluator_digest(bundle, &checker_implementation_digest)?;
    let accepted = !checks.is_empty()
        && checks
            .iter()
            .all(|check| check.status == ContractCheckStatus::Passed);
    let body = ContractReportBodyV1 {
        schema_version: CONTRACT_SCHEMA_VERSION,
        bundle_digest: bundle.bundle_digest.clone(),
        task_digest,
        tree_digest,
        checker_implementation_digest,
        evaluator_digest,
        checks,
        verifier_cost,
        accepted,
    };
    let report = ContractReportV1 {
        schema_version: CONTRACT_SCHEMA_VERSION,
        report_digest: report_body_digest(&body)?,
        body,
    };
    verify_report_structural_only(&report, bundle)?;
    Ok(report)
}

/// Verify internal structural consistency of `ContractReportV1` against `ContractBundleV1`
/// (exact check-set, bundle/evaluator/report digests, passed consistency). Never establishes
/// independent task execution/artifact acceptance without persisted journal & artifact verification.
pub(crate) fn verify_report_structural_only(
    report: &ContractReportV1,
    bundle: &ContractBundleV1,
) -> Result<(), String> {
    if report.schema_version != CONTRACT_SCHEMA_VERSION
        || report.body.schema_version != CONTRACT_SCHEMA_VERSION
    {
        return Err("unsupported ContractReportV1 version".into());
    }
    if report.body.bundle_digest != bundle.bundle_digest {
        return Err("ContractReportV1 bundle_digest mismatch".into());
    }
    let expected_eval =
        expected_contract_evaluator_digest(bundle, &report.body.checker_implementation_digest)?;
    if report.body.evaluator_digest != expected_eval {
        return Err("ContractReportV1 evaluator_digest mismatch against bundle policy".into());
    }
    if report.body.checks.is_empty() || report.body.checks.len() > MAX_REPORT_CHECKS {
        return Err("ContractReportV1 check count out of bounds".into());
    }
    let expected_checks = expected_check_specs_for_bundle(bundle);
    if report.body.checks.len() != expected_checks.len() {
        return Err(format!(
            "ContractReportV1 check-set size mismatch: expected {}, got {}",
            expected_checks.len(),
            report.body.checks.len()
        ));
    }
    for (actual, (exp_id, exp_checker)) in report.body.checks.iter().zip(&expected_checks) {
        if !is_trusted_checker(actual.checker.as_str()) {
            return Err(format!(
                "ContractReportV1 references unsupported checker: {}",
                actual.checker.as_str()
            ));
        }
        if actual.id.as_str() != exp_id || actual.checker.as_str() != *exp_checker {
            return Err(format!(
                "ContractReportV1 check-set mismatch: expected ({exp_id}, {exp_checker}), got ({}, {})",
                actual.id.as_str(),
                actual.checker.as_str()
            ));
        }
    }
    let all_passed = report
        .body
        .checks
        .iter()
        .all(|check| check.status == ContractCheckStatus::Passed);
    if report.body.accepted != all_passed {
        return Err(
            "ContractReportV1 accepted boolean contradicts individual check statuses".into(),
        );
    }
    let recomputed = report_body_digest(&report.body)?;
    if recomputed != report.report_digest {
        return Err("ContractReportV1 report_digest tamper detected".into());
    }
    let _ = canonical_bytes(report, MAX_INTERNAL).map_err(|e| format!("report bounds: {e:?}"))?;
    Ok(())
}

/// Bind `ContractReportV1` against persisted `contract-settings.json`, read-only audited/replayed
/// SQLite `RecordCodingVerification` selection, and verified immutable `CodingArtifact`.
#[derive(Clone, Debug)]
pub(crate) struct HistoricalRunAudit {
    pub task_id: TaskId,
    pub recorded_host_executable_sha256: Digest,
    pub matches_current_host_executable: bool,
}

pub(crate) fn verify_report_against_run_dir(
    report: &ContractReportV1,
    bundle: &ContractBundleV1,
    bundle_file_sha: &Digest,
    run_dir: &Path,
    workspace_dir: Option<&File>,
) -> Result<TaskId, String> {
    verify_report_against_run_dir_with_mode(
        report,
        bundle,
        bundle_file_sha,
        run_dir,
        workspace_dir,
        true,
    )
    .map(|info| info.task_id)
}

pub(crate) fn verify_report_against_run_dir_historical(
    report: &ContractReportV1,
    bundle: &ContractBundleV1,
    bundle_file_sha: &Digest,
    run_dir: &Path,
    workspace_dir: Option<&File>,
) -> Result<HistoricalRunAudit, String> {
    verify_report_against_run_dir_with_mode(
        report,
        bundle,
        bundle_file_sha,
        run_dir,
        workspace_dir,
        false,
    )
}

fn verify_report_against_run_dir_with_mode(
    report: &ContractReportV1,
    bundle: &ContractBundleV1,
    bundle_file_sha: &Digest,
    run_dir: &Path,
    workspace_dir: Option<&File>,
    require_current_host_binary: bool,
) -> Result<HistoricalRunAudit, String> {
    verify_report_structural_only(report, bundle)?;
    let (run_fd, run_abs) = super::io::open_external_nofollow(run_dir, true)?;
    if let Some(ws) = workspace_dir {
        super::io::ensure_dir_outside(&run_fd, ws, "the candidate workspace")?;
    }
    let (_, settings, _) = super::io::read_external_json::<ContractSettingsV1>(
        &run_abs.join("contract-settings.json"),
        workspace_dir,
    )?;
    let matches_current_host_executable = if require_current_host_binary {
        validate_contract_settings(&settings)?;
        true
    } else {
        validate_contract_settings_historical(&settings)?
    };
    if settings.bundle_digest != bundle.bundle_digest
        || settings.bundle_file_sha256 != *bundle_file_sha
    {
        return Err("run_dir contract-settings.json bundle binding mismatch".into());
    }
    if settings.task.data().objective != bundle.objective {
        return Err("run_dir contract-settings.json objective mismatch".into());
    }

    let expected_impl = contract_verifier_implementation_digest_with_host(
        &settings.catalog,
        &settings.host_executable_sha256,
    )?;
    let expected_eval = expected_contract_evaluator_digest(bundle, &expected_impl)?;
    if settings.checker_implementation_digest != expected_impl
        || settings.expected_evaluator_digest != expected_eval
    {
        return Err(
            "run_dir contract-settings.json verifier implementation or evaluator binding mismatch"
                .into(),
        );
    }
    if report.body.checker_implementation_digest != expected_impl
        || report.body.evaluator_digest != expected_eval
    {
        return Err(
            "ContractReportV1 checker implementation or evaluator digest differs from trusted settings"
                .into(),
        );
    }

    let before_id = super::io::verify_pinned_sqlite_file(&run_fd, &run_abs)?;
    let sqlite_path = run_abs.join("runtime.sqlite");
    let mut store =
        SqliteStore::open_read_only(&sqlite_path).map_err(|e| format!("run store: {e:?}"))?;
    let ids = store
        .task_ids()
        .map_err(|e| format!("run store task ids: {e:?}"))?;
    let task_id = &settings.task.data().task_id;
    if ids.len() != 1 || &ids[0] != task_id {
        return Err("run_dir SQLite task_id mismatch against contract-settings.json".into());
    }
    let mut snapshot = store.audit().map_err(|e| format!("run snapshot: {e:?}"))?;
    let projection = snapshot
        .projection(task_id)
        .map_err(|e| format!("run projection: {e:?}"))?;
    if projection.task.coding() != Some(&settings.task) {
        return Err("run journal CodingTask differs from contract-settings.json".into());
    }
    let engine =
        crate::cognition(&projection.genesis, &projection.task).map_err(|(_, e)| e.to_string())?;
    let audit = metron_harness::audit::verify(&mut snapshot, task_id)
        .map_err(|e| format!("run audit: {e:?}"))?;
    let behavior = metron_harness::replay::behavior(&mut snapshot, task_id, engine.as_ref())
        .map_err(|e| format!("run behavior replay: {e:?}"))?;
    let (after_run_fd, _) = super::io::open_external_nofollow(run_dir, true)?;
    let after_id = super::io::verify_pinned_sqlite_file(&after_run_fd, &run_abs)?;
    if after_id != before_id {
        return Err("run_dir SQLite directory or file identity changed during verification".into());
    }
    if !audit.chain_valid || !audit.accounting_valid || !audit.projection_valid || !behavior {
        return Err("run journal failed audit or behavior replay".into());
    }

    let mut selected: Option<(CodingVerificationSelection, bool)> = None;
    let mut after = None;
    let mut seen = 0usize;
    loop {
        let page = snapshot
            .journal_page(after)
            .map_err(|e| format!("run journal page: {e:?}"))?;
        for record in page.records {
            seen += 1;
            if seen > metron_protocol::MAX_AUDIT_RECORDS {
                return Err("run journal audit capacity exceeded".into());
            }
            let JournalEvent::CommandCommitted { command, result } = &record.data().event;
            if let StoreOperation::RecordCodingVerification {
                task_id: rec_task,
                selection,
                accepted,
                ..
            } = &command.data().operation
                && rec_task == task_id
                && matches!(result, StoreResult::VerificationRecorded)
            {
                selected = Some((selection.clone(), *accepted));
            }
        }
        let Some(next) = page.next else { break };
        after = Some(next);
    }
    let (selection, journal_accepted) =
        selected.ok_or("run journal has no committed RecordCodingVerification selection")?;

    let publication = projection
        .verification
        .publication
        .as_ref()
        .ok_or("run journal lacks committed artifact publication")?;
    let ReceiptOutput::Coding { observation } = &publication.data().output else {
        return Err("run publication receipt has wrong output kind".into());
    };
    let CodingObservation::Artifact { artifact } = observation.as_ref() else {
        return Err("run publication receipt does not contain CodingArtifact".into());
    };
    if !crate::coding_tools::inspect_artifact(&run_abs, artifact).unwrap_or(false) {
        return Err("persisted artifact failed immutable tree integrity inspection".into());
    }

    let expected_task_digest = settings
        .task
        .digest()
        .map_err(|e| format!("task digest: {e:?}"))?;
    if report.body.task_digest != expected_task_digest {
        return Err("ContractReportV1 task_digest mismatch against persisted CodingTask".into());
    }
    if report.body.tree_digest != artifact.tree_digest
        || artifact.tree_digest != selection.artifact_digest
    {
        return Err("ContractReportV1 tree_digest mismatch against verified CodingArtifact".into());
    }
    if selection.acceptance_digest != bundle.bundle_digest
        || selection.evaluator_digest != report.body.evaluator_digest
        || selection.report_digest != report.report_digest
    {
        return Err(
            "ContractReportV1 selection digests do not match committed journal verification".into(),
        );
    }
    if report.body.accepted != journal_accepted {
        return Err("ContractReportV1 accepted mismatch against committed journal outcome".into());
    }
    Ok(HistoricalRunAudit {
        task_id: task_id.clone(),
        recorded_host_executable_sha256: settings.host_executable_sha256,
        matches_current_host_executable,
    })
}

fn validate_checker_slots_and_payloads(
    draft: &ContractDraftV1,
    findings: &mut Vec<ValidationFindingV1>,
    allow_unbound_cli_vectors: bool,
) {
    let policy = &draft.checker_policy;
    if policy.metadata_checker.as_str() != CHECKER_METADATA_SNAPSHOT_LOCAL_PATH_V1 {
        push_finding(
            findings,
            "checker_slot_mismatch",
            FindingSeverity::Error,
            &format!(
                "metadata_checker slot must be '{CHECKER_METADATA_SNAPSHOT_LOCAL_PATH_V1}', got '{}'",
                policy.metadata_checker.as_str()
            ),
        );
    }
    if policy.build_checker.as_str() != CHECKER_BUILD_CARGO_OFFLINE_LOCKED_V1 {
        push_finding(
            findings,
            "checker_slot_mismatch",
            FindingSeverity::Error,
            &format!(
                "build_checker slot must be '{CHECKER_BUILD_CARGO_OFFLINE_LOCKED_V1}', got '{}'",
                policy.build_checker.as_str()
            ),
        );
    }
    if policy.test_checker.as_str() != CHECKER_TEST_CARGO_TEST_OFFLINE_LOCKED_V1 {
        push_finding(
            findings,
            "checker_slot_mismatch",
            FindingSeverity::Error,
            &format!(
                "test_checker slot must be '{CHECKER_TEST_CARGO_TEST_OFFLINE_LOCKED_V1}', got '{}'",
                policy.test_checker.as_str()
            ),
        );
    }
    if let Some(cli) = &policy.cli_checker
        && cli.as_str() != CHECKER_CLI_SELECTED_VECTORS_V1
    {
        push_finding(
            findings,
            "checker_slot_mismatch",
            FindingSeverity::Error,
            &format!(
                "cli_checker slot must be '{CHECKER_CLI_SELECTED_VECTORS_V1}', got '{}'",
                cli.as_str()
            ),
        );
    }
    if policy.cli_checker.is_some() && policy.cli_vectors.is_empty() {
        if allow_unbound_cli_vectors {
            push_finding(
                findings,
                "unbound_private_cli_vectors",
                FindingSeverity::Warning,
                "cli_checker enabled without host-sealed private cli_vectors; preparation valid but unbound until host freezes vectors",
            );
        } else {
            push_finding(
                findings,
                "missing_checker_payload",
                FindingSeverity::Error,
                "cli_checker is enabled but cli_vectors is empty",
            );
        }
    }
    if policy.cli_checker.is_none() && !policy.cli_vectors.is_empty() {
        push_finding(
            findings,
            "disabled_checker_payload",
            FindingSeverity::Error,
            "cli_vectors supplied while cli_checker slot is unset",
        );
    }
    for vec_item in &policy.cli_vectors {
        if vec_item.args.len() > MAX_CLI_ARGS {
            push_finding(
                findings,
                "cli_vector_args_out_of_bounds",
                FindingSeverity::Error,
                &format!("cli_vector args exceed {MAX_CLI_ARGS}"),
            );
        }
    }
    for iface in &policy.interface_checks {
        if iface.mode == InterfaceCheckModeV1::SemanticRustApi {
            push_finding(
                findings,
                "unsupported_semantic_api_check",
                FindingSeverity::Error,
                &format!(
                    "interface_check '{}' requested semantic_rust_api proof which is unsupported; use selected-cli-vectors-v1 or source_lexical_tokens",
                    iface.id.as_str()
                ),
            );
        }
        if iface.required_substrings.is_empty() && iface.forbidden_substrings.is_empty() {
            push_finding(
                findings,
                "missing_checker_payload",
                FindingSeverity::Error,
                &format!(
                    "interface_check '{}' must specify required or forbidden substrings",
                    iface.id.as_str()
                ),
            );
        }
    }

    for ob in &draft.requirements.mandatory_obligations {
        let checker = ob.checker.as_str();
        if !is_trusted_checker(checker) {
            push_finding(
                findings,
                "unknown_checker",
                FindingSeverity::Error,
                &format!(
                    "obligation '{}' references unsupported checker '{}'",
                    ob.id.as_str(),
                    checker
                ),
            );
            continue;
        }
        let has_payload = match checker {
            CHECKER_METADATA_SNAPSHOT_LOCAL_PATH_V1
            | CHECKER_BUILD_CARGO_OFFLINE_LOCKED_V1
            | CHECKER_TEST_CARGO_TEST_OFFLINE_LOCKED_V1 => true,
            CHECKER_CLI_SELECTED_VECTORS_V1 => {
                policy.cli_checker.is_some()
                    && (!policy.cli_vectors.is_empty() || allow_unbound_cli_vectors)
            }
            CHECKER_FIXED_FILE_V1 => !policy.fixed_files.is_empty(),
            CHECKER_SOURCE_LITERAL_V1 => !policy.interface_checks.is_empty(),
            CHECKER_FORBIDDEN_FILE_V1 => !draft.compatibility.forbidden_files.is_empty(),
            CHECKER_PRESERVED_FILE_V1 => !draft.compatibility.preserved_files.is_empty(),
            _ => false,
        };
        if !has_payload {
            push_finding(
                findings,
                "missing_checker_payload",
                FindingSeverity::Error,
                &format!(
                    "obligation '{}' maps to '{checker}' without enabled payload entries",
                    ob.id.as_str()
                ),
            );
        }
    }
}

fn host_text_authorizes_text(
    auth: Option<&HostAuthContext>,
    source_ref: &str,
    expected_phrase: &str,
) -> bool {
    let Some(ctx) = auth else {
        return false;
    };
    let Some(src) = ctx.sources_by_id.get(source_ref) else {
        return false;
    };
    if !matches!(
        src.kind,
        SourceKind::ClarificationAnswer | SourceKind::TrustedSpec
    ) {
        return false;
    }
    let phrase = expected_phrase.trim();
    if phrase.is_empty() {
        return false;
    }
    src.utf8_text
        .as_ref()
        .is_some_and(|text| text.contains(phrase))
}

pub(crate) fn validate_against_auth(
    draft: &ContractDraftV1,
    auth: Option<&HostAuthContext>,
) -> Result<(ContractValidationReportV1, Option<ContractBundleV1>), String> {
    validate_against_auth_with_mode(draft, auth, false)
}

pub(crate) fn validate_against_auth_with_mode(
    draft: &ContractDraftV1,
    auth: Option<&HostAuthContext>,
    allow_unbound_cli_vectors: bool,
) -> Result<(ContractValidationReportV1, Option<ContractBundleV1>), String> {
    let draft_hash = draft_digest(draft)?;
    let mut findings = Vec::new();

    if draft.schema_version != CONTRACT_SCHEMA_VERSION {
        push_finding(
            &mut findings,
            "unsupported_version",
            FindingSeverity::Error,
            &format!(
                "unsupported schema_version {}; expected {CONTRACT_SCHEMA_VERSION}",
                draft.schema_version
            ),
        );
    }
    if draft.contract_id.as_str().trim().is_empty() {
        push_finding(
            &mut findings,
            "empty_contract_id",
            FindingSeverity::Error,
            "contract_id must not be empty",
        );
    }
    if draft.raw_request.as_str().trim().is_empty() {
        push_finding(
            &mut findings,
            "empty_raw_request",
            FindingSeverity::Error,
            "raw_request must not be empty",
        );
    }
    let computed_req_sha = content_digest(draft.raw_request.as_str().as_bytes());
    if computed_req_sha != draft.raw_request_sha256 {
        push_finding(
            &mut findings,
            "raw_request_digest_mismatch",
            FindingSeverity::Error,
            "raw_request_sha256 does not match raw_request UTF-8 bytes",
        );
    }
    if let Some(ctx) = auth
        && !ctx.raw_request.is_empty()
        && (draft.raw_request.as_str() != ctx.raw_request
            || draft.raw_request_sha256 != ctx.raw_request_sha256)
    {
        push_finding(
            &mut findings,
            "raw_request_tampered",
            FindingSeverity::Error,
            "draft raw_request differs from host-authenticated request file",
        );
    }

    if let Some(claims) = &draft.untrusted_claims {
        if claims.model_self_approved {
            push_finding(
                &mut findings,
                "model_self_approval_denied",
                FindingSeverity::Error,
                "model_self_approved claim cannot grant contract or execution authority",
            );
        }
        if claims.model_chosen_executable.is_some() {
            push_finding(
                &mut findings,
                "model_executable_claim_denied",
                FindingSeverity::Error,
                "model_chosen_executable claim is forbidden; only closed host checker bindings apply",
            );
        }
    }

    if draft.sources.is_empty() || draft.sources.len() > MAX_SOURCE_REFS {
        push_finding(
            &mut findings,
            "sources_out_of_bounds",
            FindingSeverity::Error,
            &format!("sources count must be 1..={MAX_SOURCE_REFS}"),
        );
    }
    if draft.requirements.mandatory_obligations.is_empty()
        || draft.requirements.mandatory_obligations.len() > MAX_REQUIREMENTS
    {
        push_finding(
            &mut findings,
            "obligations_out_of_bounds",
            FindingSeverity::Error,
            &format!("mandatory_obligations count must be 1..={MAX_REQUIREMENTS}"),
        );
    }
    if draft.requirements.assumptions.len() > MAX_ASSUMPTIONS {
        push_finding(
            &mut findings,
            "assumptions_out_of_bounds",
            FindingSeverity::Error,
            &format!("assumptions count exceeds {MAX_ASSUMPTIONS}"),
        );
    }
    if draft.requirements.unknowns.len() > MAX_UNKNOWNS {
        push_finding(
            &mut findings,
            "unknowns_out_of_bounds",
            FindingSeverity::Error,
            &format!("unknowns count exceeds {MAX_UNKNOWNS}"),
        );
    }
    if draft.requirements.resolutions.len() > MAX_RESOLUTIONS {
        push_finding(
            &mut findings,
            "resolutions_out_of_bounds",
            FindingSeverity::Error,
            &format!("resolutions count exceeds {MAX_RESOLUTIONS}"),
        );
    }
    if draft.baseline.files.len() > MAX_BASELINE_FILES {
        push_finding(
            &mut findings,
            "baseline_files_out_of_bounds",
            FindingSeverity::Error,
            &format!("baseline files count exceeds {MAX_BASELINE_FILES}"),
        );
    }
    if usize::from(draft.baseline.local_rust_package_count) > MAX_LOCAL_RUST_PACKAGES
        || usize::from(draft.checker_policy.max_local_packages) > MAX_LOCAL_RUST_PACKAGES
    {
        push_finding(
            &mut findings,
            "rust_package_limit_exceeded",
            FindingSeverity::Error,
            &format!("local path Rust packages exceed {MAX_LOCAL_RUST_PACKAGES}"),
        );
    }
    if !valid_binary_name(draft.baseline.selected_cli_binary.as_str())
        || usize::from(draft.baseline.selected_cli_target_count) != MAX_SELECTED_CLI_TARGETS
    {
        push_finding(
            &mut findings,
            "invalid_selected_cli_target",
            FindingSeverity::Error,
            "contract requires one valid selected CLI target binary name without path separators",
        );
    }
    if draft.checker_policy.cli_vectors.len() > MAX_CLI_VECTORS {
        push_finding(
            &mut findings,
            "cli_vectors_out_of_bounds",
            FindingSeverity::Error,
            &format!("cli_vectors count exceeds {MAX_CLI_VECTORS}"),
        );
    }
    let checker_binding_count =
        draft.checker_policy.fixed_files.len() + draft.checker_policy.interface_checks.len();
    if checker_binding_count > MAX_CHECKER_BINDINGS {
        push_finding(
            &mut findings,
            "checker_bindings_out_of_bounds",
            FindingSeverity::Error,
            &format!("fixed/interface checker bindings exceed {MAX_CHECKER_BINDINGS}"),
        );
    }
    if draft.compatibility.approved_compatibility.len() > MAX_REQUIREMENTS
        || draft.compatibility.intended_delta.len() > MAX_REQUIREMENTS
        || draft.compatibility.preserved_files.len() > MAX_SCOPE_PATHS
        || draft.compatibility.allowed_edit_paths.len() > MAX_SCOPE_PATHS
        || draft.compatibility.forbidden_files.len() > MAX_SCOPE_PATHS
    {
        push_finding(
            &mut findings,
            "compatibility_scope_out_of_bounds",
            FindingSeverity::Error,
            "compatibility or path scope lists exceed bounds",
        );
    }

    validate_checker_slots_and_payloads(draft, &mut findings, allow_unbound_cli_vectors);
    for ob in &draft.requirements.mandatory_obligations {
        if ob.clause.as_str().trim().is_empty() {
            push_finding(
                &mut findings,
                "empty_obligation_clause",
                FindingSeverity::Error,
                &format!("obligation '{}' has empty clause", ob.id.as_str()),
            );
        }
    }

    let source_ids = check_unique_ids(
        draft.sources.iter().map(|s| s.id.as_str()),
        "source",
        &mut findings,
    );
    let _ob_ids = check_unique_ids(
        draft
            .requirements
            .mandatory_obligations
            .iter()
            .map(|o| o.id.as_str()),
        "obligation",
        &mut findings,
    );
    let _assumption_ids = check_unique_ids(
        draft.requirements.assumptions.iter().map(|a| a.id.as_str()),
        "assumption",
        &mut findings,
    );
    let unknown_ids = check_unique_ids(
        draft.requirements.unknowns.iter().map(|u| u.id.as_str()),
        "unknown",
        &mut findings,
    );
    let resolution_ids = check_unique_ids(
        draft.requirements.resolutions.iter().map(|r| r.id.as_str()),
        "resolution",
        &mut findings,
    );
    let _fixed_ids = check_unique_ids(
        draft
            .checker_policy
            .fixed_files
            .iter()
            .map(|f| f.id.as_str()),
        "fixed_file_check",
        &mut findings,
    );
    let _iface_ids = check_unique_ids(
        draft
            .checker_policy
            .interface_checks
            .iter()
            .map(|c| c.id.as_str()),
        "interface_check",
        &mut findings,
    );

    for ob in &draft.requirements.mandatory_obligations {
        if !source_ids.contains(ob.source_ref.as_str()) {
            push_finding(
                &mut findings,
                "dangling_source_ref",
                FindingSeverity::Error,
                &format!(
                    "obligation '{}' references missing source '{}'",
                    ob.id.as_str(),
                    ob.source_ref.as_str()
                ),
            );
        }
    }
    for assumption in &draft.requirements.assumptions {
        if !source_ids.contains(assumption.source_ref.as_str()) {
            push_finding(
                &mut findings,
                "dangling_source_ref",
                FindingSeverity::Error,
                &format!(
                    "assumption '{}' references missing source '{}'",
                    assumption.id.as_str(),
                    assumption.source_ref.as_str()
                ),
            );
        }
    }
    for res in &draft.requirements.resolutions {
        if !source_ids.contains(res.source_ref.as_str()) {
            push_finding(
                &mut findings,
                "dangling_source_ref",
                FindingSeverity::Error,
                &format!(
                    "resolution '{}' references missing source '{}'",
                    res.id.as_str(),
                    res.source_ref.as_str()
                ),
            );
        }
        if !unknown_ids.contains(res.unknown_id.as_str()) {
            push_finding(
                &mut findings,
                "dangling_unknown_ref",
                FindingSeverity::Error,
                &format!(
                    "resolution '{}' references missing unknown '{}'",
                    res.id.as_str(),
                    res.unknown_id.as_str()
                ),
            );
        }
    }
    for unk in &draft.requirements.unknowns {
        if let Some(res_id) = &unk.resolved_by {
            if !resolution_ids.contains(res_id.as_str()) {
                push_finding(
                    &mut findings,
                    "dangling_resolution_ref",
                    FindingSeverity::Error,
                    &format!(
                        "unknown '{}' references missing resolution '{}'",
                        unk.id.as_str(),
                        res_id.as_str()
                    ),
                );
            } else if !draft
                .requirements
                .resolutions
                .iter()
                .any(|r| r.id == *res_id && r.unknown_id == unk.id)
            {
                push_finding(
                    &mut findings,
                    "mismatched_resolution_link",
                    FindingSeverity::Error,
                    &format!(
                        "unknown '{}' points to resolution '{}' whose unknown_id differs",
                        unk.id.as_str(),
                        res_id.as_str()
                    ),
                );
            }
        }
    }

    // Host source authentication overrides any pre-populated flags from draft JSON.
    let mut authenticated_sources = Vec::with_capacity(draft.sources.len());
    for entry in &draft.sources {
        let mut normalized = entry.clone();
        if let Some(ctx) = auth {
            if let Some(real) = ctx.sources_by_id.get(entry.id.as_str()) {
                if entry.claimed_kind != real.kind {
                    push_finding(
                        &mut findings,
                        "source_authority_escalation",
                        FindingSeverity::Error,
                        &format!(
                            "source '{}' claimed kind {:?} does not match host-authenticated kind {:?}",
                            entry.id.as_str(),
                            entry.claimed_kind,
                            real.kind
                        ),
                    );
                }
                if entry.content_sha256 != real.sha256 {
                    push_finding(
                        &mut findings,
                        "source_digest_mismatch",
                        FindingSeverity::Error,
                        &format!(
                            "source '{}' digest does not match host-loaded bytes ({})",
                            entry.id.as_str(),
                            real.label
                        ),
                    );
                }
                if let Some(claimed_host_kind) = entry.host_kind
                    && claimed_host_kind != real.kind
                {
                    push_finding(
                        &mut findings,
                        "source_host_kind_tampered",
                        FindingSeverity::Error,
                        &format!(
                            "source '{}' host_kind claim differs from host authentication",
                            entry.id.as_str()
                        ),
                    );
                }
                normalized.host_kind = Some(real.kind);
                normalized.host_authenticated = true;
            } else {
                normalized.host_kind = None;
                normalized.host_authenticated = false;
                push_finding(
                    &mut findings,
                    "unauthenticated_source_ref",
                    FindingSeverity::Error,
                    &format!(
                        "source '{}' was not loaded and authenticated by host",
                        entry.id.as_str()
                    ),
                );
            }
        } else if entry.host_authenticated || entry.host_kind.is_some() {
            push_finding(
                &mut findings,
                "self_authenticated_source_rejected",
                FindingSeverity::Error,
                &format!(
                    "untrusted draft source '{}' cannot pre-assert host_authenticated/host_kind",
                    entry.id.as_str()
                ),
            );
        }
        authenticated_sources.push(normalized);
    }

    // Baseline digests, preserved/edit/forbidden scopes, and snapshot staleness.
    let mut baseline_map = BTreeMap::new();
    for (idx, file) in draft.baseline.files.iter().enumerate() {
        if idx > 0 && draft.baseline.files[idx - 1].path.as_str() >= file.path.as_str() {
            push_finding(
                &mut findings,
                "unsorted_or_duplicate_baseline_file",
                FindingSeverity::Error,
                &format!(
                    "baseline files must be strictly sorted unique paths (saw '{}')",
                    file.path.as_str()
                ),
            );
        }
        baseline_map.insert(file.path.as_str(), &file.sha256);
    }
    if draft.workflow == ContractWorkflow::Existing {
        if draft.baseline.files.is_empty() {
            push_finding(
                &mut findings,
                "missing_existing_baseline",
                FindingSeverity::Error,
                "existing workflow requires observed baseline files",
            );
        }
        if draft.compatibility.intended_delta.is_empty() {
            push_finding(
                &mut findings,
                "missing_intended_delta",
                FindingSeverity::ReadinessBlock,
                "existing repair contract requires explicit intended_delta entries distinct from observed baseline",
            );
        }
    }
    if let Some(ctx) = auth
        && let Some(snapshot) = &ctx.workspace_snapshot
        && (snapshot.len() != baseline_map.len()
            || snapshot
                .iter()
                .any(|(path, digest)| baseline_map.get(path.as_str()) != Some(&digest)))
    {
        push_finding(
            &mut findings,
            "stale_or_tampered_baseline",
            FindingSeverity::Error,
            "draft baseline files/digests do not match current observed workspace snapshot",
        );
    }

    let preserved_set: BTreeSet<&str> = draft
        .compatibility
        .preserved_files
        .iter()
        .map(|p| p.as_str())
        .collect();
    let allowed_set: BTreeSet<&str> = draft
        .compatibility
        .allowed_edit_paths
        .iter()
        .map(|p| p.as_str())
        .collect();
    let forbidden_set: BTreeSet<&str> = draft
        .compatibility
        .forbidden_files
        .iter()
        .map(|p| p.as_str())
        .collect();

    if preserved_set.len() != draft.compatibility.preserved_files.len()
        || allowed_set.len() != draft.compatibility.allowed_edit_paths.len()
        || forbidden_set.len() != draft.compatibility.forbidden_files.len()
    {
        push_finding(
            &mut findings,
            "duplicate_scope_path",
            FindingSeverity::Error,
            "compatibility scope lists must not contain duplicate paths",
        );
    }
    for path in &preserved_set {
        if !baseline_map.contains_key(path) {
            push_finding(
                &mut findings,
                "dangling_preserved_file",
                FindingSeverity::Error,
                &format!("preserved file '{path}' is missing from observed baseline"),
            );
        }
        if allowed_set.contains(path) {
            push_finding(
                &mut findings,
                "scope_conflict",
                FindingSeverity::Error,
                &format!("preserved file '{path}' cannot also appear in allowed_edit_paths"),
            );
        }
    }
    for path in &allowed_set {
        if forbidden_set.contains(path) {
            push_finding(
                &mut findings,
                "scope_conflict",
                FindingSeverity::Error,
                &format!("path '{path}' appears in both allowed_edit_paths and forbidden_files"),
            );
        }
    }
    for fixed in &draft.checker_policy.fixed_files {
        if let Some(base_sha) = baseline_map.get(fixed.path.as_str())
            && preserved_set.contains(fixed.path.as_str())
            && *base_sha != &fixed.expected_sha256
        {
            push_finding(
                &mut findings,
                "preserved_file_hash_conflict",
                FindingSeverity::Error,
                &format!(
                    "fixed_file digest for preserved path '{}' contradicts observed baseline digest",
                    fixed.path.as_str()
                ),
            );
        }
        if allowed_set.contains(fixed.path.as_str()) {
            push_finding(
                &mut findings,
                "fixed_file_edit_conflict",
                FindingSeverity::Error,
                &format!(
                    "fixed_file '{}' conflicts with allowed_edit_paths",
                    fixed.path.as_str()
                ),
            );
        }
    }
    for delta in &draft.compatibility.intended_delta {
        if delta.as_str().trim().is_empty() {
            push_finding(
                &mut findings,
                "empty_delta_clause",
                FindingSeverity::Error,
                "intended_delta clause must not be empty",
            );
        }
    }

    // Predecessor revision lineage checks bound to audited journal replay_head and typed holds.
    if let Some(lineage) = &draft.lineage {
        if lineage.revision < 2 {
            push_finding(
                &mut findings,
                "invalid_lineage_revision",
                FindingSeverity::Error,
                "superseding contract lineage revision must be >= 2",
            );
        }
        if lineage.task_id == lineage.predecessor_task_id {
            push_finding(
                &mut findings,
                "lineage_requires_fresh_task_id",
                FindingSeverity::Error,
                "superseding contract lineage must use a fresh TaskId distinct from predecessor_task_id",
            );
        }
        if lineage.asserts_unknown_reservations_cleared {
            push_finding(
                &mut findings,
                "lineage_cannot_clear_unknown_reservations",
                FindingSeverity::Error,
                "new revision lineage metadata must not assert that predecessor unknown reservations are cleared",
            );
        }
        if let Some(ctx) = auth {
            if let Some(pred_digest) = &ctx.predecessor_bundle_digest
                && pred_digest != &lineage.predecessor_bundle_digest
            {
                push_finding(
                    &mut findings,
                    "predecessor_bundle_digest_mismatch",
                    FindingSeverity::Error,
                    "lineage predecessor_bundle_digest does not match host-verified predecessor bundle",
                );
            }
            if let Some(insp) = &ctx.predecessor_inspection {
                if insp.task_id != lineage.predecessor_task_id {
                    push_finding(
                        &mut findings,
                        "predecessor_task_id_mismatch",
                        FindingSeverity::Error,
                        "lineage predecessor_task_id does not match host-inspected predecessor journal",
                    );
                }
                if lineage.predecessor_replay_head.as_ref() != Some(&insp.replay_head) {
                    push_finding(
                        &mut findings,
                        "predecessor_replay_head_mismatch",
                        FindingSeverity::Error,
                        "lineage predecessor_replay_head does not match coherent predecessor journal head",
                    );
                }
                if insp.unknown_hold && !lineage.predecessor_unknown_hold {
                    push_finding(
                        &mut findings,
                        "predecessor_unknown_hold_dropped",
                        FindingSeverity::Error,
                        "lineage metadata suppressed observed predecessor unknown hold or active dispatch",
                    );
                }
                if lineage.predecessor_open_reservations < insp.open_reservations {
                    push_finding(
                        &mut findings,
                        "predecessor_reservation_count_dropped",
                        FindingSeverity::Error,
                        "lineage metadata lowered observed predecessor open reservation count",
                    );
                }
                if lineage.predecessor_unadmitted_pending_intents != insp.unadmitted_pending_intents
                {
                    push_finding(
                        &mut findings,
                        "predecessor_unadmitted_pending_mismatch",
                        FindingSeverity::Error,
                        "lineage predecessor_unadmitted_pending_intents does not match host-inspected pending dispatches",
                    );
                }
            }
        }
    }

    // Declared readiness checks: all declared unknowns block regardless of `material`/`blocking`
    // flags; resolutions & material assumptions require host-authenticated ClarificationAnswer or
    // TrustedSpec containing the exact answer/text (model `approved: true` alone is ignored).
    for res in &draft.requirements.resolutions {
        if !host_text_authorizes_text(auth, res.source_ref.as_str(), res.answer.as_str()) {
            push_finding(
                &mut findings,
                "unauthenticated_resolution",
                FindingSeverity::ReadinessBlock,
                &format!(
                    "resolution '{}' lacks host-authenticated clarification or trusted-spec backing exact answer",
                    res.id.as_str()
                ),
            );
        }
    }
    for unk in &draft.requirements.unknowns {
        let host_resolved = unk.resolved_by.as_ref().is_some_and(|res_id| {
            draft.requirements.resolutions.iter().any(|r| {
                &r.id == res_id
                    && r.unknown_id == unk.id
                    && host_text_authorizes_text(auth, r.source_ref.as_str(), r.answer.as_str())
            })
        });
        if !host_resolved {
            push_finding(
                &mut findings,
                "unresolved_unknown",
                FindingSeverity::ReadinessBlock,
                &format!(
                    "declared unknown '{}' blocks readiness until resolved by host-authenticated clarification or trusted spec",
                    unk.id.as_str()
                ),
            );
        }
    }
    for assumption in &draft.requirements.assumptions {
        if assumption.material
            && !host_text_authorizes_text(
                auth,
                assumption.source_ref.as_str(),
                assumption.text.as_str(),
            )
        {
            push_finding(
                &mut findings,
                "unsupported_material_assumption",
                FindingSeverity::ReadinessBlock,
                &format!(
                    "material assumption '{}' lacks host-authenticated clarification or trusted-spec backing",
                    assumption.id.as_str()
                ),
            );
        }
    }

    let mut body = normalize_bundle_body(draft, &draft_hash, authenticated_sources);
    // Persist complete authenticated clarification bytes even when a model supplies
    // only an abridged resolution. Exact chunks make replay independent of source files.
    if let Some(auth) = auth {
        for (source_index, source) in body.sources.iter().enumerate() {
            if source.host_kind != Some(SourceKind::ClarificationAnswer) {
                continue;
            }
            if let Some(text) = auth
                .sources_by_id
                .get(source.id.as_str())
                .and_then(|s| s.utf8_text.as_ref())
            {
                let retained = body
                    .requirements
                    .mandatory_obligations
                    .iter()
                    .filter(|ob| {
                        ob.source_ref == source.id && ob.id.as_str().starts_with("host-clar-")
                    })
                    .map(|ob| ob.clause.as_str())
                    .collect::<String>();
                if retained == *text {
                    continue;
                }
                let mut remaining = text.as_str();
                let mut chunk_index = 0;
                while !remaining.is_empty() {
                    if body.requirements.mandatory_obligations.len() >= MAX_REQUIREMENTS {
                        push_finding(
                            &mut findings,
                            "clarification_obligation_limit",
                            FindingSeverity::Error,
                            "complete authenticated clarification exceeds the bounded obligation count",
                        );
                        break;
                    }
                    let chunk_id = format!("host-clar-{source_index}-{chunk_index}");
                    if body
                        .requirements
                        .mandatory_obligations
                        .iter()
                        .any(|ob| ob.id.as_str() == chunk_id)
                    {
                        push_finding(
                            &mut findings,
                            "reserved_clarification_id",
                            FindingSeverity::Error,
                            "draft obligation collides with host clarification chunk identity",
                        );
                        break;
                    }
                    let end = remaining.floor_char_boundary(256.min(remaining.len()));
                    body.requirements
                        .mandatory_obligations
                        .push(ContractObligationV1 {
                            id: BoundedText::new(chunk_id).map_err(|e| format!("{e:?}"))?,
                            clause: BoundedText::new(&remaining[..end])
                                .map_err(|e| format!("{e:?}"))?,
                            checker: BoundedText::new(CHECKER_TEST_CARGO_TEST_OFFLINE_LOCKED_V1)
                                .map_err(|e| format!("{e:?}"))?,
                            source_ref: source.id.clone(),
                            approved_exception: false,
                        });
                    remaining = &remaining[end..];
                    chunk_index += 1;
                }
            }
        }
    }
    let prospective_digest = bundle_body_digest(&body)?;
    let (objective_bytes, objective_result) =
        match build_contract_objective(&prospective_digest, &body) {
            Ok(obj) => (Some(obj.as_str().len()), Ok(obj)),
            Err((bytes, msg)) => {
                push_finding(
                    &mut findings,
                    "objective_overflow",
                    FindingSeverity::Error,
                    &msg,
                );
                (Some(bytes), Err(msg))
            }
        };

    if let Err(err) = canonical_bytes(draft, MAX_INTERNAL) {
        push_finding(
            &mut findings,
            "draft_size_limit",
            FindingSeverity::Error,
            &format!("canonical draft exceeds {MAX_INTERNAL} byte limit: {err:?}"),
        );
    }

    let has_errors = findings
        .iter()
        .any(|f| f.severity == FindingSeverity::Error);
    let has_blocks = findings
        .iter()
        .any(|f| f.severity == FindingSeverity::ReadinessBlock);
    let structurally_valid = !has_errors && objective_result.is_ok();
    let declared_ready = structurally_valid && !has_blocks;

    let has_unbound_cli = findings
        .iter()
        .any(|f| f.code.as_str() == "unbound_private_cli_vectors");
    let bundle = match objective_result {
        Ok(objective) if structurally_valid && !has_unbound_cli => Some(ContractBundleV1 {
            schema_version: CONTRACT_SCHEMA_VERSION,
            bundle_digest: prospective_digest.clone(),
            objective,
            body,
        }),
        _ => None,
    };

    let report = ContractValidationReportV1 {
        schema_version: CONTRACT_SCHEMA_VERSION,
        draft_digest: draft_hash,
        prospective_bundle_digest: structurally_valid.then_some(prospective_digest),
        structurally_valid,
        declared_ready,
        objective_bytes,
        findings,
    };
    Ok((report, bundle))
}

pub(crate) fn validate_contract_settings_historical(
    settings: &ContractSettingsV1,
) -> Result<bool, String> {
    if settings.schema_version != CONTRACT_SCHEMA_VERSION {
        return Err("unsupported ContractSettingsV1 schema_version".into());
    }
    if settings.task.data().schema_version != CODING_TASK_VERSION {
        return Err("unsupported CodingTask schema_version".into());
    }
    settings
        .catalog
        .validate()
        .map_err(|e| format!("tool catalog validation failed: {e:?}"))?;
    settings
        .policy
        .validate(&settings.task)
        .map_err(|e| format!("policy validation failed: {e:?}"))?;
    if settings.task.data().acceptance_digest != settings.bundle_digest {
        return Err("CodingTask acceptance_digest must equal frozen bundle_digest".into());
    }
    let catalog_digest = settings
        .catalog
        .digest()
        .map_err(|e| format!("catalog digest: {e:?}"))?;
    if settings.task.data().tool_catalog_digest != catalog_digest {
        return Err("CodingTask tool_catalog_digest mismatch against persisted catalog".into());
    }
    let expected_impl = contract_verifier_implementation_digest_with_host(
        &settings.catalog,
        &settings.host_executable_sha256,
    )?;
    if settings.checker_implementation_digest != expected_impl {
        return Err(
            "ContractSettingsV1 checker_implementation_digest does not match host verifier implementation and catalog"
                .into(),
        );
    }
    if let Some(profile) = &settings.profile {
        profile
            .validate()
            .map_err(|e| format!("local profile invalid: {e:?}"))?;
        let prof_digest = profile
            .digest()
            .map_err(|e| format!("profile digest: {e:?}"))?;
        if settings.task.data().model_profile_digest != prof_digest {
            return Err("CodingTask model_profile_digest mismatch against settings.profile".into());
        }
    }
    let _ = metron_protocol::coding::CodingTask::new(settings.task.data().clone())
        .map_err(|e| format!("task validation failed: {e:?}"))?;
    let _ = canonical_bytes(settings, MAX_ENVELOPE.max(MAX_INTERNAL))
        .map_err(|e| format!("settings size check failed: {e:?}"))?;
    Ok(settings.host_executable_sha256 == current_host_executable_digest()?)
}

pub(crate) fn validate_contract_settings(settings: &ContractSettingsV1) -> Result<(), String> {
    let matches_current = validate_contract_settings_historical(settings)?;
    if !matches_current {
        return Err(
            "ContractSettingsV1 host_executable_sha256 differs from current host executable; fresh contract workflow required"
                .into(),
        );
    }
    Ok(())
}
