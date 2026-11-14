//! CLI flag parsing and command dispatch for `metron-runtime coding contract`.

#[cfg(test)]
use super::experiment::ControlledExperimentFixtureV1;
use super::experiment::{
    ExperimentRequest, ExperimentTransportConfig, finalize_experiment_campaign, run_experiment,
};
use super::io::{
    PredecessorInspection, collect_workspace_snapshot, ensure_dir_outside,
    inspect_predecessor_journal_read_only, open_external_nofollow, read_external_bounded_bytes,
    read_external_json, write_external_new_json,
};
use super::propose::{ProposeParams, ProposeTransportConfig, run_propose};
use super::types::*;
use super::validation::{
    HostAuthContext, HostAuthSource, validate_against_auth, validate_contract_settings_historical,
    verify_report_against_run_dir_historical, verify_report_structural_only,
};
#[cfg(test)]
use metron_protocol::{
    BoundedText,
    toolkit::{ToolId, ToolPin},
};
use metron_protocol::{Digest, MAX_INTERNAL, content_digest, toolkit::ToolCatalog};
use std::{
    collections::BTreeMap,
    fs::File,
    path::{Path, PathBuf},
};

type ParsedFlagMaps = (BTreeMap<String, String>, BTreeMap<String, Vec<String>>);

fn parse_contract_flags(
    args: &[String],
    single_flags: &[&str],
    repeatable_flags: &[&str],
) -> Result<ParsedFlagMaps, String> {
    let mut singles = BTreeMap::new();
    let mut multi: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut i = 0usize;
    while i < args.len() {
        let key = args[i].as_str();
        if !key.starts_with("--") || key.contains('=') {
            return Err(format!("expected '--flag value', got '{key}'"));
        }
        let val = args
            .get(i + 1)
            .filter(|v| !v.is_empty() && !v.starts_with("--"))
            .ok_or_else(|| format!("missing value for {key}"))?;
        if single_flags.contains(&key) {
            if singles.insert(key.to_string(), val.clone()).is_some() {
                return Err(format!("duplicate flag {key}"));
            }
        } else if repeatable_flags.contains(&key) {
            multi.entry(key.to_string()).or_default().push(val.clone());
        } else {
            return Err(format!("unsupported flag {key}"));
        }
        i += 2;
    }
    Ok((singles, multi))
}

fn parse_named_source_arg(spec: &str) -> Result<(&str, PathBuf), String> {
    let (id, raw_path) = spec
        .split_once('=')
        .filter(|(id, p)| !id.trim().is_empty() && !p.is_empty())
        .ok_or_else(|| format!("expected ID=FILE for source flag, got '{spec}'"))?;
    Ok((id, PathBuf::from(raw_path)))
}

struct OpenedAuthHandles {
    workspace_fd: Option<File>,
    predecessor_dir_fd: Option<File>,
    context: HostAuthContext,
}

fn build_host_auth_context(
    singles: &BTreeMap<String, String>,
    multi: &BTreeMap<String, Vec<String>>,
    require_request: bool,
) -> Result<OpenedAuthHandles, String> {
    let (workspace_fd, workspace_snapshot) = match singles.get("--workspace") {
        Some(ws) => {
            let (fd, map) = collect_workspace_snapshot(Path::new(ws))?;
            (Some(fd), Some(map))
        }
        None => (None, None),
    };
    let ws_ref = workspace_fd.as_ref();

    let mut sources_by_id = BTreeMap::new();
    let (raw_request, raw_request_sha256) = if let Some(req_path) = singles.get("--request-file") {
        let (resolved, bytes, sha) =
            read_external_bounded_bytes(Path::new(req_path), ws_ref, MAX_RAW_REQUEST_BYTES)?;
        let text =
            String::from_utf8(bytes).map_err(|_| "request file must be valid UTF-8".to_string())?;
        sources_by_id.insert(
            "src-request".to_string(),
            HostAuthSource {
                kind: SourceKind::OperatorRequest,
                sha256: sha.clone(),
                label: resolved.display().to_string(),
                utf8_text: Some(text.clone()),
            },
        );
        (text, sha)
    } else if require_request {
        return Err("missing --request-file".into());
    } else {
        (String::new(), content_digest(b""))
    };

    if let Some(clar_path) = singles.get("--clarification-file") {
        let (resolved, bytes, sha) =
            read_external_bounded_bytes(Path::new(clar_path), ws_ref, MAX_RAW_REQUEST_BYTES)?;
        let text = String::from_utf8(bytes)
            .map_err(|_| "clarification file must be valid UTF-8".to_string())?;
        sources_by_id.insert(
            "src-clarification".to_string(),
            HostAuthSource {
                kind: SourceKind::ClarificationAnswer,
                sha256: sha,
                label: resolved.display().to_string(),
                utf8_text: Some(text),
            },
        );
    }

    if let Some(specs) = multi.get("--source") {
        for spec in specs {
            let (id, path) = parse_named_source_arg(spec)?;
            let (resolved, _bytes, sha) = read_external_bounded_bytes(&path, None, MAX_INTERNAL)?;
            if sources_by_id
                .insert(
                    id.to_string(),
                    HostAuthSource {
                        kind: SourceKind::RepositoryFile,
                        sha256: sha,
                        label: resolved.display().to_string(),
                        utf8_text: None,
                    },
                )
                .is_some()
            {
                return Err(format!("duplicate source id '{id}'"));
            }
        }
    }

    if let Some(specs) = multi.get("--trusted-spec") {
        for spec in specs {
            let (id, path) = parse_named_source_arg(spec)?;
            let (resolved, bytes, sha) = read_external_bounded_bytes(&path, ws_ref, MAX_INTERNAL)?;
            let text = String::from_utf8(bytes)
                .map_err(|_| "trusted-spec file must be valid UTF-8".to_string())?;
            if sources_by_id
                .insert(
                    id.to_string(),
                    HostAuthSource {
                        kind: SourceKind::TrustedSpec,
                        sha256: sha,
                        label: resolved.display().to_string(),
                        utf8_text: Some(text),
                    },
                )
                .is_some()
            {
                return Err(format!("duplicate trusted-spec id '{id}'"));
            }
        }
    }

    if let Some(snapshot) = &workspace_snapshot {
        for (rel, sha) in snapshot {
            let key = format!("src-file:{rel}");
            sources_by_id.entry(key).or_insert_with(|| HostAuthSource {
                kind: SourceKind::RepositoryFile,
                sha256: sha.clone(),
                label: rel.clone(),
                utf8_text: None,
            });
        }
    }

    let predecessor_bundle_digest = singles
        .get("--predecessor-bundle")
        .map(|path| {
            let (_, pred_bundle, _) =
                read_external_json::<ContractBundleV1>(Path::new(path), ws_ref)?;
            let recomputed = bundle_body_digest(&pred_bundle.body)?;
            if recomputed != pred_bundle.bundle_digest {
                return Err("predecessor bundle digest tamper detected".into());
            }
            Ok::<_, String>(pred_bundle.bundle_digest)
        })
        .transpose()?;

    let (predecessor_dir_fd, predecessor_inspection): (
        Option<File>,
        Option<PredecessorInspection>,
    ) = match singles.get("--predecessor-run-dir") {
        Some(pred_run) => {
            let (fd, insp) = inspect_predecessor_journal_read_only(Path::new(pred_run), None)?;
            (Some(fd), Some(insp))
        }
        None => (None, None),
    };

    Ok(OpenedAuthHandles {
        workspace_fd,
        predecessor_dir_fd,
        context: HostAuthContext {
            raw_request,
            raw_request_sha256,
            sources_by_id,
            workspace_snapshot,
            predecessor_bundle_digest,
            predecessor_inspection,
        },
    })
}

fn validate_cmd(args: &[String]) -> Result<serde_json::Value, String> {
    let (singles, multi) = parse_contract_flags(
        args,
        &[
            "--draft",
            "--request-file",
            "--workspace",
            "--clarification-file",
            "--predecessor-bundle",
            "--predecessor-run-dir",
            "--output",
        ],
        &["--source", "--trusted-spec"],
    )?;
    let draft_arg = singles.get("--draft").ok_or("missing --draft")?;
    let handles = build_host_auth_context(&singles, &multi, true)?;
    let ws_ref = handles.workspace_fd.as_ref();
    let (_, draft, _) = read_external_json::<ContractDraftV1>(Path::new(draft_arg), ws_ref)?;
    let (report, _bundle) = validate_against_auth(&draft, Some(&handles.context))?;
    if let Some(out_path) = singles.get("--output") {
        let mut forbidden = Vec::new();
        if let Some(pred_fd) = &handles.predecessor_dir_fd {
            forbidden.push((pred_fd, "the immutable predecessor run directory"));
        }
        write_external_new_json(Path::new(out_path), ws_ref, &forbidden, &report)?;
    }
    let outcome = if report.structurally_valid && report.declared_ready {
        "succeeded"
    } else {
        "blocked"
    };
    Ok(serde_json::json!({
        "outcome": outcome,
        "action": "validate",
        "structurally_valid": report.structurally_valid,
        "declared_ready": report.declared_ready,
        "prospective_bundle_digest": report.prospective_bundle_digest,
        "objective_bytes": report.objective_bytes,
        "report": report,
    }))
}

fn freeze_cmd(args: &[String]) -> Result<serde_json::Value, String> {
    let (singles, multi) = parse_contract_flags(
        args,
        &[
            "--draft",
            "--request-file",
            "--expected-digest",
            "--output",
            "--workspace",
            "--clarification-file",
            "--predecessor-bundle",
            "--predecessor-run-dir",
        ],
        &["--source", "--trusted-spec"],
    )?;
    let draft_arg = singles.get("--draft").ok_or("missing --draft")?;
    let expected_raw = singles
        .get("--expected-digest")
        .ok_or("missing --expected-digest")?;
    let expected_digest =
        Digest::new(expected_raw.clone()).map_err(|_| "invalid --expected-digest".to_string())?;
    let output_arg = singles.get("--output").ok_or("missing --output")?;
    let handles = build_host_auth_context(&singles, &multi, true)?;
    let ws_ref = handles.workspace_fd.as_ref();

    let (_, draft, _) = read_external_json::<ContractDraftV1>(Path::new(draft_arg), ws_ref)?;
    let (report, bundle) = validate_against_auth(&draft, Some(&handles.context))?;
    if !report.structurally_valid || !report.declared_ready {
        return Ok(serde_json::json!({
            "outcome": "blocked",
            "action": "freeze",
            "diagnostic": "contract draft is not both structurally valid and declared ready",
            "structurally_valid": report.structurally_valid,
            "declared_ready": report.declared_ready,
            "prospective_bundle_digest": report.prospective_bundle_digest,
            "findings": report.findings,
        }));
    }
    let bundle = bundle.ok_or("validated draft did not yield a bundle")?;
    if bundle.bundle_digest != expected_digest {
        return Err(format!(
            "operator expected digest mismatch: expected {}, computed {}",
            expected_digest.as_str(),
            bundle.bundle_digest.as_str()
        ));
    }
    let mut forbidden = Vec::new();
    if let Some(pred_fd) = &handles.predecessor_dir_fd {
        forbidden.push((pred_fd, "the immutable predecessor run directory"));
    }
    let (written_path, file_sha) =
        write_external_new_json(Path::new(output_arg), ws_ref, &forbidden, &bundle)?;
    Ok(serde_json::json!({
        "outcome": "succeeded",
        "action": "freeze",
        "bundle_digest": bundle.bundle_digest,
        "draft_digest": bundle.body.draft_digest,
        "bundle_file_sha256": file_sha,
        "output": written_path.display().to_string(),
        "objective_bytes": bundle.objective.as_str().len(),
        "lineage": bundle.body.lineage,
    }))
}

fn bundle_to_draft(bundle: &ContractBundleV1) -> ContractDraftV1 {
    ContractDraftV1 {
        schema_version: bundle.body.schema_version,
        contract_id: bundle.body.contract_id.clone(),
        workflow: bundle.body.workflow,
        raw_request: bundle.body.raw_request.clone(),
        raw_request_sha256: bundle.body.raw_request_sha256.clone(),
        sources: bundle.body.sources.clone(),
        baseline: bundle.body.baseline.clone(),
        compatibility: bundle.body.compatibility.clone(),
        requirements: bundle.body.requirements.clone(),
        checker_policy: bundle.body.checker_policy.clone(),
        lineage: bundle.body.lineage.clone(),
        untrusted_claims: None,
    }
}

fn audit_cmd(args: &[String]) -> Result<serde_json::Value, String> {
    let (singles, multi) = parse_contract_flags(
        args,
        &[
            "--bundle",
            "--request-file",
            "--workspace",
            "--clarification-file",
            "--predecessor-bundle",
            "--predecessor-run-dir",
            "--report",
            "--run-dir",
        ],
        &["--source", "--trusted-spec"],
    )?;
    let bundle_arg = singles.get("--bundle").ok_or("missing --bundle")?;
    let require_req = singles.contains_key("--request-file");
    let handles = build_host_auth_context(&singles, &multi, require_req)?;
    let ws_ref = handles.workspace_fd.as_ref();
    let (_bundle_path, bundle, bundle_file_sha) =
        read_external_json::<ContractBundleV1>(Path::new(bundle_arg), ws_ref)?;

    if bundle.schema_version != CONTRACT_SCHEMA_VERSION
        || bundle.body.schema_version != CONTRACT_SCHEMA_VERSION
    {
        return Ok(serde_json::json!({
            "outcome": "integrity_failure",
            "action": "audit",
            "diagnostic": "unsupported bundle schema_version",
        }));
    }
    let recomputed_digest = bundle_body_digest(&bundle.body)?;
    if recomputed_digest != bundle.bundle_digest {
        return Ok(serde_json::json!({
            "outcome": "integrity_failure",
            "action": "audit",
            "diagnostic": "bundle_digest mismatch (tampered frozen bundle)",
            "expected_digest": recomputed_digest,
            "recorded_digest": bundle.bundle_digest,
        }));
    }
    let expected_obj = match build_contract_objective(&bundle.bundle_digest, &bundle.body) {
        Ok(obj) => obj,
        Err((_, msg)) => {
            return Ok(serde_json::json!({
                "outcome": "integrity_failure",
                "action": "audit",
                "diagnostic": msg,
            }));
        }
    };
    if expected_obj != bundle.objective {
        return Ok(serde_json::json!({
            "outcome": "integrity_failure",
            "action": "audit",
            "diagnostic": "bundle objective does not match deterministic bundle derivation",
        }));
    }

    let mut normalized_draft = bundle_to_draft(&bundle);
    let all_sources_marked_auth = normalized_draft
        .sources
        .iter()
        .all(|s| s.host_authenticated && s.host_kind == Some(s.claimed_kind));
    if !all_sources_marked_auth {
        return Ok(serde_json::json!({
            "outcome": "integrity_failure",
            "action": "audit",
            "diagnostic": "bundle contains unauthenticated or mismatched source entry",
        }));
    }

    let external_sources_supplied = singles.contains_key("--request-file")
        || singles.contains_key("--workspace")
        || singles.contains_key("--clarification-file")
        || singles.contains_key("--predecessor-bundle")
        || singles.contains_key("--predecessor-run-dir")
        || multi.contains_key("--source")
        || multi.contains_key("--trusted-spec");

    let sources_reauthenticated = if external_sources_supplied {
        let (val_report, _) = validate_against_auth(&normalized_draft, Some(&handles.context))?;
        if !val_report.structurally_valid || !val_report.declared_ready {
            return Ok(serde_json::json!({
                "outcome": "integrity_failure",
                "action": "audit",
                "diagnostic": "frozen bundle failed host re-authentication or staleness audit",
                "findings": val_report.findings,
            }));
        }
        true
    } else {
        for s in &mut normalized_draft.sources {
            s.host_authenticated = false;
            s.host_kind = None;
        }
        let (val_report, _) = validate_against_auth(&normalized_draft, None)?;
        if !val_report.structurally_valid {
            return Ok(serde_json::json!({
                "outcome": "integrity_failure",
                "action": "audit",
                "diagnostic": "frozen bundle failed internal structural invariants",
                "findings": val_report.findings,
            }));
        }
        false
    };

    if let (Some(pred_fd), Some(run_dir_arg)) =
        (&handles.predecessor_dir_fd, singles.get("--run-dir"))
    {
        let (run_fd, _) = open_external_nofollow(Path::new(run_dir_arg), true)?;
        if ensure_dir_outside(&run_fd, pred_fd, "the immutable predecessor run directory").is_err()
        {
            return Ok(serde_json::json!({
                "outcome": "integrity_failure",
                "action": "audit",
                "diagnostic": "contract run_dir aliases immutable predecessor directory",
            }));
        }
    }

    if let Some(report_path) = singles.get("--report") {
        let (_, report, _) =
            read_external_json::<ContractReportV1>(Path::new(report_path), ws_ref)?;
        if let Err(err) = verify_report_structural_only(&report, &bundle) {
            return Ok(serde_json::json!({
                "outcome": "integrity_failure",
                "action": "audit",
                "diagnostic": err,
            }));
        }
        let Some(run_dir_arg) = singles.get("--run-dir") else {
            return Ok(serde_json::json!({
                "outcome": "blocked",
                "action": "audit",
                "audit_scope": "report_structural_only_missing_run_dir",
                "independent_acceptance_verified": false,
                "bundle_digest": bundle.bundle_digest,
                "report_digest": report.report_digest,
                "diagnostic": "report matches bundle structurally but cannot claim independent acceptance without persisted --run-dir journal and artifact verification",
            }));
        };
        let audit = match verify_report_against_run_dir_historical(
            &report,
            &bundle,
            &bundle_file_sha,
            Path::new(run_dir_arg),
            ws_ref,
        ) {
            Ok(audit) => audit,
            Err(err) => {
                return Ok(serde_json::json!({
                    "outcome": "integrity_failure",
                    "action": "audit",
                    "diagnostic": err,
                }));
            }
        };
        let audit_scope = if !audit.matches_current_host_executable {
            "historical_binary_recorded_consistency_only"
        } else if sources_reauthenticated {
            "full_reauthenticated"
        } else {
            "run_and_report_verified"
        };
        return Ok(serde_json::json!({
            "outcome": "succeeded",
            "action": "audit",
            "audit_scope": audit_scope,
            "independent_acceptance_verified": true,
            "matches_current_host_executable": audit.matches_current_host_executable,
            "fresh_execution_compatible": audit.matches_current_host_executable,
            "historical_binary_audit": !audit.matches_current_host_executable,
            "recorded_host_executable_sha256": audit.recorded_host_executable_sha256,
            "sources_reauthenticated": sources_reauthenticated,
            "bundle_digest": bundle.bundle_digest,
            "bundle_file_sha256": bundle_file_sha,
            "objective_bytes": bundle.objective.as_str().len(),
            "report_digest": report.report_digest,
            "accepted": report.body.accepted,
            "task_id": audit.task_id,
        }));
    }

    if let Some(run_dir_arg) = singles.get("--run-dir") {
        let (_, run_abs) = open_external_nofollow(Path::new(run_dir_arg), true)?;
        let (_, settings, _) = read_external_json::<ContractSettingsV1>(
            &run_abs.join("contract-settings.json"),
            ws_ref,
        )?;
        let matches_current_exe = validate_contract_settings_historical(&settings)?;
        if settings.bundle_digest != bundle.bundle_digest
            || settings.bundle_file_sha256 != bundle_file_sha
            || settings.task.data().objective != bundle.objective
        {
            return Ok(serde_json::json!({
                "outcome": "integrity_failure",
                "action": "audit",
                "diagnostic": "contract-settings.json binding mismatch against frozen bundle",
            }));
        }
        let audit_scope = if !matches_current_exe {
            "historical_binary_settings_recorded_only"
        } else if sources_reauthenticated {
            "full_reauthenticated"
        } else {
            "bundle_and_settings_verified"
        };
        return Ok(serde_json::json!({
            "outcome": "succeeded",
            "action": "audit",
            "audit_scope": audit_scope,
            "independent_acceptance_verified": false,
            "matches_current_host_executable": matches_current_exe,
            "fresh_execution_compatible": matches_current_exe,
            "historical_binary_audit": !matches_current_exe,
            "recorded_host_executable_sha256": settings.host_executable_sha256,
            "sources_reauthenticated": sources_reauthenticated,
            "bundle_digest": bundle.bundle_digest,
            "bundle_file_sha256": bundle_file_sha,
            "objective_bytes": bundle.objective.as_str().len(),
            "task_id": settings.task.data().task_id,
        }));
    }

    Ok(serde_json::json!({
        "outcome": "succeeded",
        "action": "audit",
        "audit_scope": if sources_reauthenticated { "bundle_and_sources_reauthenticated" } else { "bundle_structural_integrity_only" },
        "independent_acceptance_verified": false,
        "sources_reauthenticated": sources_reauthenticated,
        "bundle_digest": bundle.bundle_digest,
        "bundle_file_sha256": bundle_file_sha,
        "objective_bytes": bundle.objective.as_str().len(),
    }))
}

fn parse_bool_flag(raw: &str, flag: &str) -> Result<bool, String> {
    match raw {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        other => Err(format!(
            "invalid boolean for {flag}: '{other}' (expected true/false)"
        )),
    }
}

fn parse_workflow_flag(raw: &str) -> Result<ContractWorkflow, String> {
    match raw {
        "new" => Ok(ContractWorkflow::New),
        "existing" => Ok(ContractWorkflow::Existing),
        other => Err(format!(
            "invalid --workflow '{other}' (expected 'new' or 'existing')"
        )),
    }
}

#[cfg(test)]
fn placeholder_controlled_catalog() -> Result<ToolCatalog, String> {
    Ok(ToolCatalog {
        schema_version: 1,
        tools: vec![ToolPin {
            tool: ToolId::Cargo,
            executable: BoundedText::new("/unavailable-controlled-cargo")
                .map_err(|e| format!("{e:?}"))?,
            sha256: content_digest(b"controlled-placeholder"),
            version_prefix: BoundedText::new("controlled").map_err(|e| format!("{e:?}"))?,
        }],
    })
}

fn propose_cmd(args: &[String]) -> Result<serde_json::Value, String> {
    let (singles, multi) = parse_contract_flags(
        args,
        &[
            "--request-file",
            "--contract-id",
            "--output-dir",
            "--workflow",
            "--selected-binary",
            "--stages",
            "--architecture-scaffold",
            "--workspace",
            "--clarification-file",
            "--predecessor-bundle",
            "--predecessor-run-dir",
            "--profile",
            "--worker",
            "--artifact",
            "--recorded-replies",
        ],
        &["--source", "--trusted-spec"],
    )?;
    let contract_id = singles
        .get("--contract-id")
        .ok_or("missing --contract-id")?
        .clone();
    let out_dir = singles.get("--output-dir").ok_or("missing --output-dir")?;
    let workflow = match singles.get("--workflow") {
        Some(w) => parse_workflow_flag(w)?,
        None if singles.contains_key("--workspace") => ContractWorkflow::Existing,
        None => ContractWorkflow::New,
    };
    let selected_binary = singles
        .get("--selected-binary")
        .cloned()
        .unwrap_or_else(|| "metron-case".to_string());
    let max_stages = match singles.get("--stages") {
        Some(raw) => raw
            .parse::<u8>()
            .map_err(|e| format!("invalid --stages '{raw}': {e}"))?,
        None => MAX_PREP_REQUESTS,
    };
    let architecture_scaffold = match singles.get("--architecture-scaffold") {
        Some(raw) => parse_bool_flag(raw, "--architecture-scaffold")?,
        None => false,
    };

    let has_recorded = singles.contains_key("--recorded-replies");
    let has_native = singles.contains_key("--profile")
        || singles.contains_key("--worker")
        || singles.contains_key("--artifact");
    if has_recorded && has_native {
        return Err(
            "--recorded-replies cannot be combined with --profile/--worker/--artifact".into(),
        );
    }
    #[cfg(not(test))]
    if has_recorded {
        return Err(
            "recorded model replies cannot execute preparation; import drafts through validate"
                .into(),
        );
    }
    let transport = if let Some(replies) = singles.get("--recorded-replies") {
        ProposeTransportConfig::RecordedRepliesFile {
            replies_path: PathBuf::from(replies),
        }
    } else {
        let prof = singles
            .get("--profile")
            .ok_or("missing --profile (or --recorded-replies)")?;
        let worker = singles
            .get("--worker")
            .ok_or("missing --worker (or --recorded-replies)")?;
        let art = singles
            .get("--artifact")
            .ok_or("missing --artifact (or --recorded-replies)")?;
        ProposeTransportConfig::NativeTeam {
            coding_profile_path: PathBuf::from(prof),
            worker_path: PathBuf::from(worker),
            artifact_path: PathBuf::from(art),
        }
    };

    let handles = build_host_auth_context(&singles, &multi, true)?;
    let res = run_propose(
        Path::new(out_dir),
        handles.workspace_fd.as_ref(),
        handles.predecessor_dir_fd.as_ref(),
        &handles.context,
        &ProposeParams {
            contract_id,
            workflow,
            selected_binary,
            max_stages,
            architecture_scaffold,
        },
        transport,
    )?;

    Ok(serde_json::json!({
        "outcome": res.outcome,
        "action": "propose",
        "evidence_origin": res.ledger.evidence_origin,
        "prospective_bundle_digest": res.ledger.prospective_bundle_digest,
        "execution_ready": res.bundle.is_some(),
        "unbound_private_cli_vectors": res.validation.as_ref().is_some_and(|v| v.findings.iter().any(|f| f.code.as_str() == "unbound_private_cli_vectors")),
        "ledger": res.ledger,
        "draft": res.draft,
        "validation": res.validation,
        "bundle": res.bundle,
    }))
}

fn promotion_outcome_label(verdict: PromotionVerdictV1) -> &'static str {
    match verdict {
        PromotionVerdictV1::PromotedTypedOnly
        | PromotionVerdictV1::PromotedTypedPlusArchitecture => "succeeded",
        PromotionVerdictV1::DiagnosticGateBlocked => "diagnostic_gate_blocked",
        PromotionVerdictV1::BlockedInvalidAnnotationsOrOrigin => "blocked",
        PromotionVerdictV1::NegativeDoNotPromote => "negative_do_not_promote",
    }
}

fn experiment_cmd(args: &[String]) -> Result<serde_json::Value, String> {
    let (singles, _) = parse_contract_flags(
        args,
        &[
            "--corpus",
            "--corpus-freeze",
            "--output-dir",
            "--finalize-campaign",
            "--catalog",
            "--annotations",
            "--campaign-id",
            "--profile",
            "--worker",
            "--artifact",
            "--controlled-replies",
        ],
        &[],
    )?;
    if let Some(camp_dir) = singles.get("--finalize-campaign") {
        for conflict in [
            "--output-dir",
            "--catalog",
            "--campaign-id",
            "--profile",
            "--worker",
            "--artifact",
            "--controlled-replies",
        ] {
            if singles.contains_key(conflict) {
                return Err(format!(
                    "--finalize-campaign cannot be combined with {conflict}"
                ));
            }
        }
        let ann_path = singles
            .get("--annotations")
            .ok_or("--finalize-campaign requires --annotations")?;
        let corpus_opt = singles.get("--corpus").map(PathBuf::from);
        let freeze_opt = singles.get("--corpus-freeze").map(PathBuf::from);
        if corpus_opt.is_none() || freeze_opt.is_none() {
            return Err("finalization requires --corpus and --corpus-freeze".into());
        }
        let (report, receipt) = finalize_experiment_campaign(
            Path::new(camp_dir),
            Path::new(ann_path),
            corpus_opt.as_deref(),
            freeze_opt.as_deref(),
        )?;
        let outcome = promotion_outcome_label(receipt.decision.verdict);
        return Ok(serde_json::json!({
            "outcome": outcome,
            "action": "experiment",
            "finalized": true,
            "evidence_origin": report.evidence_origin,
            "verdict": receipt.decision.verdict,
            "selected_arm": receipt.decision.selected_arm,
            "architecture_only_finding": receipt.decision.architecture_only_finding,
            "diagnostic_gate": report.diagnostic_gate,
            "promotion_receipt": receipt,
            "experiment_report": report,
        }));
    }

    let corpus_path = singles.get("--corpus").ok_or("missing --corpus")?;
    let freeze_path = singles
        .get("--corpus-freeze")
        .ok_or("missing --corpus-freeze")?;
    let output_dir = singles.get("--output-dir").ok_or("missing --output-dir")?;
    let campaign_id = singles
        .get("--campaign-id")
        .cloned()
        .unwrap_or_else(|| "m2-pilot-v1".to_string());
    let annotations_path = singles.get("--annotations").map(PathBuf::from);

    let has_ctrl = singles.contains_key("--controlled-replies");
    let has_native = singles.contains_key("--profile")
        || singles.contains_key("--worker")
        || singles.contains_key("--artifact");
    if has_ctrl && has_native {
        return Err(
            "--controlled-replies cannot be combined with --profile/--worker/--artifact".into(),
        );
    }

    #[cfg(not(test))]
    if has_ctrl {
        return Err("controlled effect execution is test-only".into());
    }
    let native_transport = || -> Result<_, String> {
        let catalog_path = singles
            .get("--catalog")
            .ok_or("native experiment requires explicit --catalog")?;
        let prof = singles.get("--profile").ok_or("missing --profile")?;
        let worker = singles.get("--worker").ok_or("missing --worker")?;
        let art = singles.get("--artifact").ok_or("missing --artifact")?;
        Ok((
            read_external_json::<ToolCatalog>(Path::new(catalog_path), None)?.1,
            ExperimentTransportConfig::Native {
                coding_profile_path: PathBuf::from(prof),
                worker_path: PathBuf::from(worker),
                artifact_path: PathBuf::from(art),
            },
        ))
    };
    #[cfg(not(test))]
    let (catalog, transport) = native_transport()?;
    #[cfg(test)]
    let (catalog, transport) = if let Some(ctrl_path) = singles.get("--controlled-replies") {
        let (_, fixture, _) =
            read_external_json::<ControlledExperimentFixtureV1>(Path::new(ctrl_path), None)?;
        let catalog = match singles.get("--catalog") {
            Some(c) => read_external_json::<ToolCatalog>(Path::new(c), None)?.1,
            None => placeholder_controlled_catalog()?,
        };
        (
            catalog,
            ExperimentTransportConfig::Controlled {
                fixture,
                evidence_origin: EvidenceOriginV1::ImportedRecordedReplies,
                custom_runner_factory: None,
            },
        )
    } else {
        native_transport()?
    };

    let report = run_experiment(ExperimentRequest {
        campaign_id,
        corpus_path: PathBuf::from(corpus_path),
        corpus_freeze_path: PathBuf::from(freeze_path),
        output_dir: PathBuf::from(output_dir),
        catalog,
        annotations_path,
        transport,
    })?;

    let outcome = promotion_outcome_label(report.promotion_receipt.decision.verdict);

    Ok(serde_json::json!({
        "outcome": outcome,
        "action": "experiment",
        "finalized": false,
        "evidence_origin": report.evidence_origin,
        "verdict": report.promotion_receipt.decision.verdict,
        "selected_arm": report.promotion_receipt.decision.selected_arm,
        "architecture_only_finding": report.promotion_receipt.decision.architecture_only_finding,
        "diagnostic_gate": report.diagnostic_gate,
        "promotion_receipt": report.promotion_receipt,
        "experiment_report": report,
    }))
}

fn deferred_public_coding_response(action: &str) -> serde_json::Value {
    serde_json::json!({
        "outcome": "blocked",
        "action": action,
        "status": "deferred_pending_native_promotion",
        "diagnostic": "public contract run/resume deferred until an actual native pilot passes promotion gate",
    })
}

fn run_cmd(args: &[String]) -> Result<serde_json::Value, String> {
    let _ = parse_contract_flags(
        args,
        &[
            "--promotion-receipt",
            "--annotations",
            "--bundle",
            "--workspace",
            "--run-dir",
            "--task-id",
            "--catalog",
            "--profile",
            "--worker",
            "--artifact",
            "--prep-requests-admitted",
            "--total-model-cap",
            "--max-tool-calls",
            "--predecessor-run-dir",
            "--predecessor-task-id",
            "--dev-checks",
        ],
        &[],
    )?;
    Ok(deferred_public_coding_response("run"))
}

fn resume_cmd(args: &[String]) -> Result<serde_json::Value, String> {
    let _ = parse_contract_flags(
        args,
        &[
            "--promotion-receipt",
            "--annotations",
            "--run-dir",
            "--catalog",
        ],
        &[],
    )?;
    Ok(deferred_public_coding_response("resume"))
}

pub(crate) fn run(args: &[String]) -> Result<serde_json::Value, String> {
    let sub = args.first().map(String::as_str).unwrap_or("");
    let rest = args.get(1..).unwrap_or(&[]);
    match sub {
        "validate" => validate_cmd(rest),
        "freeze" => freeze_cmd(rest),
        "audit" => audit_cmd(rest),
        "propose" => propose_cmd(rest),
        "experiment" => experiment_cmd(rest),
        "run" => run_cmd(rest),
        "resume" => resume_cmd(rest),
        "" => Err(
            "expected coding contract subcommand: validate, freeze, audit, propose, experiment, run, or resume"
                .into(),
        ),
        other => Err(format!("unknown coding contract subcommand '{other}'")),
    }
}
