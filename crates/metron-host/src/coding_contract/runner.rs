//! Scoped per-effect driver, `ContractSettingsV1` binding, fresh execution, and bound resume
//! built directly over `CodingTools`, `LocalInferenceDriver`, and `orchestrator::drive_coding_with_driver`.

use super::io::{
    create_external_private_dir, inspect_predecessor_journal_read_only, open_external_nofollow,
    read_external_json, write_external_new_json,
};
#[cfg(test)]
use super::io::{ensure_dir_outside, verify_pinned_sqlite_file};
use super::propose::resolve_pinned_artifact;
use super::types::*;
use super::validation::{validate_contract_settings, verify_report_against_run_dir};
use super::verifier::ContractVerifier;
#[cfg(test)]
use crate::coding_acceptance::{EvaluatorRunner, RunnerError};
use crate::{
    coding_run,
    coding_tools::{CodingTools, ToolOutcome, inspect_artifact},
};
use metron_harness::{
    driver::DriverOutcome,
    orchestrator::{self, NoObserver},
    policy::{AuthorizedEffect, EffectDriver},
};
use metron_inference_local::{artifact::verify_artifact, driver::LocalInferenceDriver};
use metron_protocol::{
    ActualUsage, ApprovedRequest, AuditSnapshot, BoundedText, BudgetRequest, DecimalUsage, Digest,
    EngineMode, Genesis, LocalProfile, MAX_MODEL_OUTPUT, MAX_TEXT, PrincipalId, ProducerReceiptId,
    Receipt, ReceiptData, ReceiptOutput, SqlU64, StoreOperation, StorePort, Task, TaskId,
    UsageVector,
    coding::{
        CodingAction, CodingFamily, CodingPolicy, CodingTask, CodingTaskData, CodingTool,
        RustProgram,
    },
    coding_runtime::{CodingObservation, CodingStep, CodingToolOutput, ToolStatus},
    domain::{Cognition, Phase},
    store::{ReservationStatus, RuntimeSnapshot},
    toolkit::ToolCatalog,
};
#[cfg(test)]
use metron_protocol::{coding_runtime::CodingArtifact, content_digest};
use metron_store_sqlite::SqliteStore;
#[cfg(test)]
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

#[cfg(test)]
pub(crate) type CustomRunnerFactory =
    Box<dyn FnMut(&CodingArtifact, &Digest) -> Result<Box<dyn EvaluatorRunner>, RunnerError>>;

/// Bounded coding driver enforcing contract edit scope and preserved/forbidden paths
/// AFTER durable tool admission via normal `ToolStatus::Denied`. All arms retain ordinary
/// compiler/test tool parity (`Cargo`/`Rustfmt`).
pub(crate) struct ContractEffectDriver {
    tools: CodingTools,
    native: Option<LocalInferenceDriver>,
    #[cfg(test)]
    script: VecDeque<String>,
    bundle: ContractBundleV1,
    run_dir: PathBuf,
    pub denied_scope_edits: u32,
    pub denied_dev_runs: u32,
}

impl ContractEffectDriver {
    pub(crate) fn new(
        tools: CodingTools,
        native: Option<LocalInferenceDriver>,
        #[cfg(test)] script: VecDeque<String>,
        bundle: ContractBundleV1,
        run_dir: PathBuf,
    ) -> Self {
        Self {
            tools,
            native,
            #[cfg(test)]
            script,
            bundle,
            run_dir,
            denied_scope_edits: 0,
            denied_dev_runs: 0,
        }
    }

    fn known(
        &self,
        effect: &AuthorizedEffect,
        output: ReceiptOutput,
        model: u64,
        tools: u64,
    ) -> DriverOutcome {
        let prepared = effect.request();
        let receipt = Receipt::new(ReceiptData {
            schema_version: effect.schema_version(),
            producer_receipt_id: ProducerReceiptId::new(prepared.action.dispatch_id.as_str())
                .expect("dispatch_id digest is valid identifier"),
            task_id: prepared.action.task_id.clone(),
            intent_id: prepared.intent_id.clone(),
            dispatch_id: prepared.action.dispatch_id.clone(),
            action_digest: prepared.action_digest.clone(),
            usage: ActualUsage::Known {
                usage: UsageVector {
                    model_requests: DecimalUsage::new(model.to_string()).expect("valid u64"),
                    tool_calls: DecimalUsage::new(tools.to_string()).expect("valid u64"),
                    spend_micros_usd: DecimalUsage::new("0").expect("valid zero"),
                },
            },
            output,
        });
        match receipt {
            Ok(receipt) => DriverOutcome::Receipt(Box::new(receipt)),
            Err(_) => DriverOutcome::Uncertain,
        }
    }

    fn denied_tool_receipt(
        &self,
        effect: &AuthorizedEffect,
        tool: CodingTool,
        reason: &str,
    ) -> DriverOutcome {
        let end = reason.floor_char_boundary(reason.len().min(MAX_MODEL_OUTPUT));
        let body = BoundedText::new(&reason[..end]).expect("bounded denial reason");
        self.known(
            effect,
            ReceiptOutput::Coding {
                observation: Box::new(CodingObservation::Tool {
                    output: CodingToolOutput {
                        tool,
                        status: ToolStatus::Denied,
                        body,
                        truncated: false,
                        exit_code: None,
                        files: Vec::new(),
                        process_tree_closed: true,
                    },
                    memory_selection: None,
                }),
            },
            0,
            1,
        )
    }

    fn uncertain(&self, effect: &AuthorizedEffect, diagnostic: &str) -> DriverOutcome {
        let end = diagnostic.floor_char_boundary(MAX_TEXT);
        let value = serde_json::json!({
            "dispatch_id": effect.request().action.dispatch_id,
            "status": "uncertain",
            "diagnostic": &diagnostic[..end],
        });
        let _ = write_external_new_json(
            &self.run_dir.join(format!(
                "uncertainty-{}.json",
                effect.request().action.dispatch_id.as_str()
            )),
            None,
            &[],
            &value,
        );
        DriverOutcome::Uncertain
    }

    fn failure(&self, effect: &AuthorizedEffect, diagnostic: &str) -> DriverOutcome {
        let end = diagnostic.floor_char_boundary(diagnostic.len().min(MAX_TEXT));
        self.known(
            effect,
            ReceiptOutput::Failure {
                diagnostic: BoundedText::new(&diagnostic[..end]).expect("bounded diagnostic"),
            },
            0,
            0,
        )
    }

    fn check_edit_scope(&self, path: &metron_protocol::coding::WorkspacePath) -> Option<String> {
        let compat = &self.bundle.body.compatibility;
        if compat.forbidden_files.contains(path) {
            return Some(format!(
                "edit denied by contract policy: '{}' is forbidden",
                path.as_str()
            ));
        }
        if compat.preserved_files.contains(path) {
            return Some(format!(
                "edit denied by contract policy: '{}' is preserved",
                path.as_str()
            ));
        }
        if !compat.allowed_edit_paths.is_empty() && !compat.allowed_edit_paths.contains(path) {
            return Some(format!(
                "edit denied by contract policy: '{}' is outside allowed_edit_paths",
                path.as_str()
            ));
        }
        None
    }

    pub(crate) fn shutdown_native(&mut self) {
        if let Some(native) = self.native.as_mut() {
            native.shutdown();
        }
    }
}

impl EffectDriver for ContractEffectDriver {
    type Outcome = DriverOutcome;

    fn execute(&mut self, effect: AuthorizedEffect) -> DriverOutcome {
        let ApprovedRequest::Coding { step, .. } = &effect.request().request else {
            return self.failure(&effect, "unsupported non-coding authorized effect");
        };
        match step.as_ref() {
            CodingStep::Model {
                input_digest: _input_digest,
                ..
            } => {
                if let Some(native) = self.native.as_mut() {
                    return native.execute(effect);
                }
                #[cfg(test)]
                {
                    let Some(content) = self.script.pop_front() else {
                        return self.failure(&effect, "controlled coding script exhausted");
                    };
                    self.known(
                        &effect,
                        ReceiptOutput::Coding {
                            observation: Box::new(CodingObservation::ScriptedModel {
                                input_digest: _input_digest.clone(),
                                content: BoundedText::new(content)
                                    .expect("validated script content"),
                            }),
                        },
                        1,
                        0,
                    )
                }
                #[cfg(not(test))]
                self.failure(&effect, "native model driver required")
            }
            CodingStep::Tool { action, .. } => {
                match action {
                    CodingAction::Edit { path, .. } => {
                        if let Some(reason) = self.check_edit_scope(path) {
                            self.denied_scope_edits = self.denied_scope_edits.saturating_add(1);
                            return self.denied_tool_receipt(&effect, CodingTool::Edit, &reason);
                        }
                    }
                    CodingAction::MemoryPut { .. } | CodingAction::MemorySearch { .. } => {
                        return self.denied_tool_receipt(
                            &effect,
                            action.tool(),
                            "contract harness disables unscoped memory mutations",
                        );
                    }
                    _ => {}
                }
                match self
                    .tools
                    .execute(action, &effect.request().action.dispatch_id)
                {
                    ToolOutcome::Known(output) => self.known(
                        &effect,
                        ReceiptOutput::Coding {
                            observation: Box::new(CodingObservation::Tool {
                                output,
                                memory_selection: None,
                            }),
                        },
                        0,
                        1,
                    ),
                    ToolOutcome::Uncertain(diagnostic) => self.uncertain(&effect, &diagnostic),
                }
            }
            CodingStep::Freeze { .. } => {
                self.shutdown_native();
                match self.tools.freeze(&effect.request().action.dispatch_id) {
                    Ok(artifact) => self.known(
                        &effect,
                        ReceiptOutput::Coding {
                            observation: Box::new(CodingObservation::Artifact { artifact }),
                        },
                        0,
                        1,
                    ),
                    Err(err) => self.uncertain(&effect, &err),
                }
            }
        }
    }
}

pub(crate) struct CodingAttemptRequest {
    pub task_id: TaskId,
    pub bundle_path: PathBuf,
    pub workspace_path: PathBuf,
    pub run_dir: PathBuf,
    pub catalog: ToolCatalog,
    pub dev_checks_enabled: bool,
    pub architecture_scaffold: bool,
    pub prep_requests_admitted: u32,
    pub total_model_cap: u32,
    pub max_tool_calls: u32,
    pub evidence_origin: EvidenceOriginV1,
    pub profile_path: Option<PathBuf>,
    pub worker_path: Option<PathBuf>,
    pub artifact_path: Option<PathBuf>,
    #[cfg(test)]
    pub scripted_replies: Option<Vec<String>>,
    pub predecessor_run_dir: Option<PathBuf>,
    pub predecessor_task_id: Option<TaskId>,
    #[cfg(test)]
    pub custom_verifier_runner: Option<CustomRunnerFactory>,
    #[cfg(test)]
    pub allowed_metadata_roots: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
pub(crate) struct ContractCodingOutcome {
    pub outcome: &'static str,
    pub accepted: bool,
    pub model_requests_used: u64,
    pub tool_calls_used: u64,
    #[cfg(test)]
    pub denied_scope_edits: u32,
    #[cfg(test)]
    pub denied_dev_runs: u32,
    pub unknown_or_open_reservations: u32,
    pub report_verified: bool,
}

fn build_coding_policy(
    principal: &PrincipalId,
    project: &BoundedText<128>,
    ws_abs: &Path,
) -> Result<CodingPolicy, String> {
    let allowed_tools = vec![
        CodingTool::Plan,
        CodingTool::List,
        CodingTool::Read,
        CodingTool::Edit,
        CodingTool::Run,
        CodingTool::Finish,
    ];
    let allowed_programs = vec![RustProgram::Cargo, RustProgram::Rustfmt];
    Ok(CodingPolicy {
        schema_version: 1,
        principal_id: principal.clone(),
        project_id: project.clone(),
        workspace_root: BoundedText::new(ws_abs.display().to_string())
            .map_err(|e| format!("workspace path bounded: {e:?}"))?,
        memory_collection: None,
        allowed_tools,
        allowed_programs,
        require_mutation_approval: false,
        max_file_bytes: 1024 * 1024,
        max_output_bytes: MAX_MODEL_OUTPUT as u32,
        command_timeout_ms: 60_000,
        command_memory_bytes: SqlU64::new(1024 * 1024 * 1024)
            .map_err(|e| format!("command memory: {e:?}"))?,
        max_processes: 32,
    })
}

fn finalize_run_outcome(
    run_abs: &Path,
    ws_fd: Option<&std::fs::File>,
    (bundle, bundle_file_sha): (&ContractBundleV1, &Digest),
    task_id: &TaskId,
    engine: &dyn Cognition,
    store: &mut SqliteStore,
    (denied_scope_edits, denied_dev_runs): (u32, u32),
) -> Result<ContractCodingOutcome, String> {
    let mut snapshot = store
        .audit()
        .map_err(|e| format!("final audit snapshot: {e:?}"))?;
    let audit = metron_harness::audit::verify(&mut snapshot, task_id)
        .map_err(|e| format!("final audit: {e:?}"))?;
    let behavior = metron_harness::replay::behavior(&mut snapshot, task_id, engine)
        .map_err(|e| format!("final behavior replay: {e:?}"))?;
    let projection = snapshot
        .projection(task_id)
        .map_err(|e| format!("final projection: {e:?}"))?;

    let mut open_reservations = 0u32;
    let (mut model_admissions, mut tool_admissions) = (0u64, 0u64);
    let mut cursor = None;
    loop {
        let page = snapshot
            .intents(task_id, cursor)
            .map_err(|e| format!("final intents scan: {e:?}"))?;
        for rec in page.records {
            if rec.reservation.is_some()
                && let Some(prepared) = &rec.prepared
                && let ApprovedRequest::Coding { step, .. } = &prepared.request
            {
                match step.as_ref() {
                    CodingStep::Model { .. } => model_admissions += 1,
                    CodingStep::Tool { .. } | CodingStep::Freeze { .. } => tool_admissions += 1,
                }
            }
            if rec
                .reservation
                .as_ref()
                .is_some_and(|r| r.status == ReservationStatus::Held)
            {
                open_reservations = open_reservations.saturating_add(1);
            }
        }
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }

    let report_path = run_abs.join("contract-report.json");
    let (accepted, report_verified) = if report_path.exists() {
        let (_, report, _) = read_external_json::<ContractReportV1>(&report_path, ws_fd)?;
        verify_report_against_run_dir(&report, bundle, bundle_file_sha, run_abs, ws_fd)?;
        (report.body.accepted, true)
    } else {
        (false, false)
    };

    let integrity_ok = audit.chain_valid
        && audit.accounting_valid
        && audit.projection_valid
        && behavior
        && (projection.state.phase != Phase::Completed || (report_verified && accepted));

    let artifact_valid = match projection
        .verification
        .publication
        .as_ref()
        .map(|r| &r.data().output)
    {
        Some(ReceiptOutput::Coding { observation }) => match observation.as_ref() {
            CodingObservation::Artifact { artifact } => {
                inspect_artifact(run_abs, artifact).unwrap_or(false)
            }
            _ => false,
        },
        _ => true,
    };

    let outcome = if !integrity_ok || !artifact_valid {
        "integrity_failure"
    } else if projection.state.phase == Phase::Completed && accepted {
        "completed"
    } else if projection.state.phase == Phase::Rejected {
        "rejected"
    } else {
        "blocked"
    };

    let _ = (&denied_scope_edits, &denied_dev_runs);
    Ok(ContractCodingOutcome {
        outcome,
        accepted: outcome == "completed",
        model_requests_used: model_admissions,
        tool_calls_used: tool_admissions,
        #[cfg(test)]
        denied_scope_edits,
        #[cfg(test)]
        denied_dev_runs,
        unknown_or_open_reservations: open_reservations,
        report_verified,
    })
}

pub(crate) fn execute_contract_coding_attempt(
    req: CodingAttemptRequest,
) -> Result<ContractCodingOutcome, String> {
    let (ws_fd, ws_abs) = open_external_nofollow(&req.workspace_path, true)?;
    if req.run_dir.exists() {
        return Err(format!(
            "fresh run_dir '{}' already exists (use resume or new dir)",
            req.run_dir.display()
        ));
    }

    let mut forbidden = Vec::new();
    let pred_fd_holder = if let Some(pred_dir) = &req.predecessor_run_dir {
        let (pred_fd, _) =
            inspect_predecessor_journal_read_only(pred_dir, req.predecessor_task_id.as_ref())?;
        Some(pred_fd)
    } else {
        None
    };
    if let Some(fd) = &pred_fd_holder {
        forbidden.push((fd, "the immutable predecessor run directory"));
    }

    let (_, bundle, bundle_file_sha) =
        read_external_json::<ContractBundleV1>(&req.bundle_path, Some(&ws_fd))?;
    if bundle.schema_version != CONTRACT_SCHEMA_VERSION
        || bundle.body.schema_version != CONTRACT_SCHEMA_VERSION
    {
        return Err("unsupported ContractBundleV1 schema version".into());
    }
    if bundle_body_digest(&bundle.body)? != bundle.bundle_digest {
        return Err("ContractBundleV1 digest mismatch".into());
    }
    if build_contract_objective(&bundle.bundle_digest, &bundle.body).map_err(|(_, m)| m)?
        != bundle.objective
    {
        return Err("ContractBundleV1 objective mismatch".into());
    }

    if let Some(lineage) = &bundle.body.lineage
        && lineage.task_id != req.task_id
    {
        return Err("coding attempt task_id must match bundle.body.lineage.task_id".into());
    }

    if req.prep_requests_admitted > req.total_model_cap {
        return Err(format!(
            "prep_requests_admitted {} exceeds total_model_cap {}",
            req.prep_requests_admitted, req.total_model_cap
        ));
    }
    let remaining_model_requests = req
        .total_model_cap
        .saturating_sub(req.prep_requests_admitted);
    if remaining_model_requests == 0 {
        return Err("zero coding model requests remain after preparation budget".into());
    }

    // Re-check actual workspace tree at the execution boundary BEFORE creating run_dir/SQLite
    // or dispatching any model or tool effects.
    let (_, actual_snapshot) = super::io::collect_workspace_snapshot(&ws_abs)?;
    let bound_baseline: std::collections::BTreeMap<&str, &Digest> = bundle
        .body
        .baseline
        .files
        .iter()
        .map(|file| (file.path.as_str(), &file.sha256))
        .collect();
    if actual_snapshot.len() != bound_baseline.len()
        || actual_snapshot
            .iter()
            .any(|(path, digest)| bound_baseline.get(path.as_str()) != Some(&digest))
    {
        return Err(
            "execution rejected: current workspace does not match bound initial baseline snapshot (stale, missing, or unexpected file) before any dispatch"
                .into(),
        );
    }

    let (_run_fd, run_abs) = create_external_private_dir(&req.run_dir, Some(&ws_fd), &forbidden)?;

    req.catalog
        .validate()
        .map_err(|e| format!("tool catalog invalid: {e:?}"))?;
    let checker_impl_digest = contract_verifier_implementation_digest(&req.catalog)?;
    let expected_eval_digest = expected_contract_evaluator_digest(&bundle, &checker_impl_digest)?;

    let (profile_opt, model_profile_digest) = match req.evidence_origin {
        EvidenceOriginV1::NativeTeamSession => {
            let prof_path = req
                .profile_path
                .as_ref()
                .ok_or_else(|| "native coding attempt requires profile_path".to_string())?;
            let (_, profile, _) = read_external_json::<LocalProfile>(prof_path, Some(&ws_fd))?;
            profile
                .validate()
                .map_err(|e| format!("coding profile invalid: {e:?}"))?;
            let prof_digest = profile
                .digest()
                .map_err(|e| format!("coding profile digest: {e:?}"))?;
            (Some(profile), prof_digest)
        }
        #[cfg(not(test))]
        EvidenceOriginV1::ImportedRecordedReplies | EvidenceOriginV1::ScriptedTestDriver => {
            return Err("scripted effect execution is test-only".into());
        }
        #[cfg(test)]
        EvidenceOriginV1::ImportedRecordedReplies | EvidenceOriginV1::ScriptedTestDriver => {
            if req.scripted_replies.is_none() {
                return Err("controlled/scripted coding attempt requires scripted_replies".into());
            }
            (None, content_digest(b"coding-scripted/v1"))
        }
    };

    let principal = PrincipalId::new("contract-operator").map_err(|e| format!("{e:?}"))?;
    let project = bundle.body.contract_id.clone();
    let max_steps =
        u8::try_from((remaining_model_requests + req.max_tool_calls + 8).min(64)).unwrap_or(64);
    let task = CodingTask::new(CodingTaskData {
        schema_version: 3,
        family: CodingFamily::CodingV1,
        task_id: req.task_id.clone(),
        principal_id: principal.clone(),
        project_id: project.clone(),
        objective: bundle.objective.clone(),
        acceptance_digest: bundle.bundle_digest.clone(),
        tool_catalog_digest: req
            .catalog
            .digest()
            .map_err(|e| format!("catalog digest: {e:?}"))?,
        model_profile_digest,
        memory_collection: None,
        budget: BudgetRequest {
            max_model_requests: SqlU64::new(u64::from(remaining_model_requests))
                .map_err(|e| format!("model budget: {e:?}"))?,
            max_tool_calls: SqlU64::new(u64::from(req.max_tool_calls))
                .map_err(|e| format!("tool budget: {e:?}"))?,
            max_spend_micros_usd: SqlU64::new(0).map_err(|e| format!("{e:?}"))?,
            max_elapsed_ms: None,
        },
        max_decision_steps: max_steps,
    })
    .map_err(|e| format!("CodingTask::new: {e:?}"))?;
    let policy = build_coding_policy(&principal, &project, &ws_abs)?;

    let settings = ContractSettingsV1 {
        schema_version: CONTRACT_SCHEMA_VERSION,
        task: task.clone(),
        policy: policy.clone(),
        catalog: req.catalog.clone(),
        bundle_digest: bundle.bundle_digest.clone(),
        bundle_file_sha256: bundle_file_sha.clone(),
        checker_implementation_digest: checker_impl_digest.clone(),
        host_executable_sha256: current_host_executable_digest()?,
        expected_evaluator_digest: expected_eval_digest,
        evidence_origin: req.evidence_origin,
        dev_checks_enabled: req.dev_checks_enabled,
        architecture_scaffold: req.architecture_scaffold,
        prep_requests_admitted: req.prep_requests_admitted,
        remaining_coding_model_requests: remaining_model_requests,
        profile: profile_opt.clone(),
        worker: req.worker_path.clone(),
        artifact: req.artifact_path.clone(),
        predecessor_run_dir: req.predecessor_run_dir.clone(),
        predecessor_task_id: req.predecessor_task_id.clone(),
    };
    validate_contract_settings(&settings)?;
    write_external_new_json(
        &run_abs.join("bundle.json"),
        Some(&ws_fd),
        &forbidden,
        &bundle,
    )?;
    write_external_new_json(
        &run_abs.join("contract-settings.json"),
        Some(&ws_fd),
        &forbidden,
        &settings,
    )?;

    let native_driver = match (&profile_opt, &req.worker_path, &req.artifact_path) {
        (Some(profile), Some(w), Some(a)) => {
            let (_, w_abs) = open_external_nofollow(w, false)?;
            let (_, a_abs) = open_external_nofollow(a, false)?;
            let art_kind = resolve_pinned_artifact(profile)?;
            verify_artifact(&a_abs, art_kind)
                .map_err(|e| format!("verify coding artifact {}: {e}", a_abs.display()))?;
            Some(
                LocalInferenceDriver::new(w_abs, a_abs, profile.clone(), None)
                    .map_err(|e| format!("LocalInferenceDriver::new: {e}"))?,
            )
        }
        (None, None, None) => None,
        _ => return Err("inconsistent profile/worker/artifact bindings for coding driver".into()),
    };

    let genesis = Genesis {
        mode: EngineMode::Agent,
        engine_version: BoundedText::new("coding-agent/v3.10").map_err(|e| format!("{e:?}"))?,
        require_publication_approval: false,
        policy_digest: policy
            .digest(&task)
            .map_err(|e| format!("policy digest: {e:?}"))?,
        local_profile: profile_opt,
        coding_policy: Some(Box::new(policy.clone())),
    };
    let task_enum =
        Task::new_coding(task.clone()).map_err(|e| format!("Task::new_coding: {e:?}"))?;
    let engine = crate::cognition(&genesis, &task_enum).map_err(|(_, e)| e.to_string())?;

    let sqlite_path = run_abs.join("runtime.sqlite");
    let mut store =
        SqliteStore::create(&sqlite_path).map_err(|e| format!("create run sqlite: {e:?}"))?;
    store
        .apply(
            coding_run::command(StoreOperation::CreateTask {
                task: task_enum,
                genesis,
            })
            .map_err(|e| format!("CreateTask command: {e:?}"))?,
        )
        .map_err(|e| format!("CreateTask apply: {e:?}"))?;

    let tools = CodingTools::new(&task, &policy, &req.catalog, &ws_abs, &run_abs)?;
    #[cfg(test)]
    let script = req
        .scripted_replies
        .unwrap_or_default()
        .into_iter()
        .collect();
    let mut driver = ContractEffectDriver::new(
        tools,
        native_driver,
        #[cfg(test)]
        script,
        bundle.clone(),
        run_abs.clone(),
    );

    #[cfg(not(test))]
    let mut verifier = ContractVerifier::with_verifier_runner(
        bundle.clone(),
        bundle_file_sha.clone(),
        req.catalog.clone(),
        run_abs.clone(),
    )?;
    #[cfg(test)]
    let mut verifier = match req.custom_verifier_runner {
        Some(make) => ContractVerifier::with_custom_runner(
            bundle.clone(),
            bundle_file_sha.clone(),
            checker_impl_digest,
            run_abs.clone(),
            if req.allowed_metadata_roots.is_empty() {
                vec![PathBuf::from("/workspace")]
            } else {
                req.allowed_metadata_roots
            },
            make,
        ),
        None => ContractVerifier::with_verifier_runner(
            bundle.clone(),
            bundle_file_sha.clone(),
            req.catalog.clone(),
            run_abs.clone(),
        )?,
    };

    let drive_res = orchestrator::drive_coding_with_driver(
        &mut store,
        &req.task_id,
        engine.as_ref(),
        &mut NoObserver,
        &mut driver,
        &mut verifier,
    );
    driver.shutdown_native();
    drive_res.map_err(|e| format!("drive_coding_with_driver: {e:?}"))?;

    finalize_run_outcome(
        &run_abs,
        Some(&ws_fd),
        (&bundle, &bundle_file_sha),
        &req.task_id,
        engine.as_ref(),
        &mut store,
        (driver.denied_scope_edits, driver.denied_dev_runs),
    )
}

#[cfg(test)]
pub(crate) fn resume_contract_coding_attempt(
    run_dir: &Path,
    expected_catalog: Option<&ToolCatalog>,
    scripted_replies: Option<Vec<String>>,
    custom_verifier_runner: Option<CustomRunnerFactory>,
    allowed_metadata_roots: Vec<PathBuf>,
) -> Result<ContractCodingOutcome, String> {
    let (run_fd, run_abs) = open_external_nofollow(run_dir, true)?;
    let before_id = verify_pinned_sqlite_file(&run_fd, &run_abs)?;

    let (_, settings, _) =
        read_external_json::<ContractSettingsV1>(&run_abs.join("contract-settings.json"), None)?;
    validate_contract_settings(&settings)?;
    if let Some(catalog) = expected_catalog
        && settings.catalog.digest().ok() != catalog.digest().ok()
    {
        return Err("resume rejected: runtime catalog differs from settings.catalog".into());
    }

    let (_, bundle, bundle_file_sha) =
        read_external_json::<ContractBundleV1>(&run_abs.join("bundle.json"), None)?;
    if bundle_body_digest(&bundle.body)? != bundle.bundle_digest
        || bundle.bundle_digest != settings.bundle_digest
        || bundle_file_sha != settings.bundle_file_sha256
    {
        return Err(
            "resume rejected: persisted bundle.json does not match ContractSettingsV1".into(),
        );
    }

    let current_impl = contract_verifier_implementation_digest(&settings.catalog)?;
    let current_eval = expected_contract_evaluator_digest(&bundle, &current_impl)?;
    if settings.checker_implementation_digest != current_impl
        || settings.expected_evaluator_digest != current_eval
    {
        return Err(
            "resume rejected: checker implementation or evaluator binding changed since creation"
                .into(),
        );
    }

    let ws_path = Path::new(settings.policy.workspace_root.as_str());
    let (ws_fd, ws_abs) = open_external_nofollow(ws_path, true)?;
    ensure_dir_outside(&run_fd, &ws_fd, "the candidate workspace")?;

    let sqlite_path = run_abs.join("runtime.sqlite");
    let mut store =
        SqliteStore::open_existing(&sqlite_path).map_err(|e| format!("open sqlite: {e:?}"))?;
    let task_id = &settings.task.data().task_id;
    let projection = {
        let mut snap = store.audit().map_err(|e| format!("resume audit: {e:?}"))?;
        let proj = snap
            .projection(task_id)
            .map_err(|e| format!("resume projection: {e:?}"))?;
        let engine = crate::cognition(&proj.genesis, &proj.task).map_err(|(_, e)| e.to_string())?;
        let checks = metron_harness::audit::verify(&mut snap, task_id)
            .map_err(|e| format!("resume verify: {e:?}"))?;
        let behavior = metron_harness::replay::behavior(&mut snap, task_id, engine.as_ref())
            .map_err(|e| format!("resume replay: {e:?}"))?;
        if !checks.chain_valid || !checks.accounting_valid || !checks.projection_valid || !behavior
        {
            return Err("resume rejected: existing journal failed audit or behavior replay".into());
        }
        proj
    };
    let (after_fd, _) = open_external_nofollow(run_dir, true)?;
    let after_id = verify_pinned_sqlite_file(&after_fd, &run_abs)?;
    if after_id != before_id {
        return Err("resume rejected: run_dir SQLite identity changed".into());
    }
    if projection.task.coding() != Some(&settings.task) {
        return Err(
            "resume rejected: journal CodingTask differs from contract-settings.json".into(),
        );
    }
    let engine =
        crate::cognition(&projection.genesis, &projection.task).map_err(|(_, e)| e.to_string())?;

    let native_driver = match (&settings.profile, &settings.worker, &settings.artifact) {
        (Some(profile), Some(w), Some(a)) => {
            let (_, w_abs) = open_external_nofollow(w, false)?;
            let (_, a_abs) = open_external_nofollow(a, false)?;
            let art_kind = resolve_pinned_artifact(profile)?;
            verify_artifact(&a_abs, art_kind)
                .map_err(|e| format!("verify resume artifact: {e}"))?;
            Some(
                LocalInferenceDriver::new(w_abs, a_abs, profile.clone(), None)
                    .map_err(|e| format!("resume driver: {e}"))?,
            )
        }
        (None, None, None) => None,
        _ => return Err("resume rejected: inconsistent driver bindings in settings".into()),
    };

    let observed_model = projection.task_account.posted.model_requests.get() as usize;
    let remaining_script: VecDeque<String> = scripted_replies
        .unwrap_or_default()
        .into_iter()
        .skip(observed_model)
        .collect();

    let tools = CodingTools::new(
        &settings.task,
        &settings.policy,
        &settings.catalog,
        &ws_abs,
        &run_abs,
    )?;
    let mut driver = ContractEffectDriver::new(
        tools,
        native_driver,
        remaining_script,
        bundle.clone(),
        run_abs.clone(),
    );

    let mut verifier = match custom_verifier_runner {
        Some(make) => ContractVerifier::with_custom_runner(
            bundle.clone(),
            bundle_file_sha.clone(),
            current_impl,
            run_abs.clone(),
            if allowed_metadata_roots.is_empty() {
                vec![PathBuf::from("/workspace")]
            } else {
                allowed_metadata_roots
            },
            make,
        ),
        None => ContractVerifier::with_verifier_runner(
            bundle.clone(),
            bundle_file_sha.clone(),
            settings.catalog.clone(),
            run_abs.clone(),
        )?,
    };

    let drive_res = orchestrator::drive_coding_with_driver(
        &mut store,
        task_id,
        engine.as_ref(),
        &mut NoObserver,
        &mut driver,
        &mut verifier,
    );
    driver.shutdown_native();
    drive_res.map_err(|e| format!("resume drive_coding_with_driver: {e:?}"))?;

    finalize_run_outcome(
        &run_abs,
        Some(&ws_fd),
        (&bundle, &bundle_file_sha),
        task_id,
        engine.as_ref(),
        &mut store,
        (driver.denied_scope_edits, driver.denied_dev_runs),
    )
}

/// Re-derive retained admission counts and reject scripted model evidence for native captures.
/// This is local host provenance checking, not cryptographic worker attestation.
pub(crate) fn audit_retained_admissions(
    run_dir: &Path,
    settings: &ContractSettingsV1,
) -> Result<(u64, u64, bool), String> {
    let (_, inspection) = super::io::inspect_predecessor_journal_read_only(
        run_dir,
        Some(&settings.task.data().task_id),
    )?;
    let mut store = SqliteStore::open_read_only(run_dir.join("runtime.sqlite"))
        .map_err(|e| format!("retained store: {e:?}"))?;
    let mut snapshot = store
        .audit()
        .map_err(|e| format!("retained snapshot: {e:?}"))?;
    let projection = snapshot
        .projection(&inspection.task_id)
        .map_err(|e| format!("retained projection: {e:?}"))?;
    if projection.task
        != Task::new_coding(settings.task.clone()).map_err(|e| format!("retained task: {e:?}"))?
        || projection.genesis.local_profile != settings.profile
    {
        return Err("retained task/profile disagrees with captured settings".into());
    }
    let (mut models, mut tools) = (0u64, 0u64);
    let mut cursor = None;
    loop {
        let page = snapshot
            .intents(&inspection.task_id, cursor)
            .map_err(|e| format!("retained intents: {e:?}"))?;
        for intent in page.records {
            if intent.reservation.is_some()
                && let Some(prepared) = &intent.prepared
                && let ApprovedRequest::Coding { step, .. } = &prepared.request
            {
                match step.as_ref() {
                    CodingStep::Model { .. } => models += 1,
                    CodingStep::Tool { .. } | CodingStep::Freeze { .. } => tools += 1,
                }
            }
            if settings.evidence_origin == EvidenceOriginV1::NativeTeamSession {
                let mut evidence_cursor = None;
                loop {
                    let evidence = snapshot
                        .evidence(&intent.intent_id, evidence_cursor)
                        .map_err(|e| format!("retained evidence: {e:?}"))?;
                    for record in evidence.records {
                        if matches!(&record.receipt.data().output, ReceiptOutput::Coding { observation } if matches!(observation.as_ref(), CodingObservation::ScriptedModel { .. }))
                        {
                            return Err("native capture contains scripted model evidence".into());
                        }
                    }
                    evidence_cursor = evidence.next;
                    if evidence_cursor.is_none() {
                        break;
                    }
                }
            }
        }
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    Ok((models, tools, inspection.unknown_hold))
}

#[cfg(test)]
mod native_catalog_tests {
    use super::*;

    #[test]
    fn experimental_policy_is_supported_by_the_pinned_native_codec() {
        let policy = build_coding_policy(
            &PrincipalId::new("operator").unwrap(),
            &BoundedText::new("native-contract").unwrap(),
            Path::new("/contract-workspace"),
        )
        .unwrap();
        for edit_available in [false, true] {
            metron_protocol::native_xml_coding_v10_grammar(&policy.allowed_tools, edit_available)
                .expect("the experimental policy must fit the pinned native tool catalog");
        }
        assert!(policy.allowed_tools.contains(&CodingTool::Run));
        assert!(policy.allowed_programs.contains(&RustProgram::Cargo));
    }
}
