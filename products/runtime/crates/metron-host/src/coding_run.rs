//! Explicit experimental coding entry point. Qualification is separate from
//! attempting a task; a completed task is not a production-profile selection.
use crate::{
    coding_acceptance::{self, AcceptanceSpec},
    coding_tools::{CodingTools, ToolOutcome, VerifierRunner},
};
use metron_harness::{
    driver::DriverOutcome,
    orchestrator::{self, CodingVerifier, DriveError, NoObserver},
    policy::{AuthorizedEffect, EffectDriver},
};
use metron_inference_local::{
    artifact::{Artifact, sha256_file, verify_artifact},
    driver::LocalInferenceDriver,
};
use metron_protocol::{coding::*, toolkit::ToolCatalog, *};
use metron_store_sqlite::{
    SqliteStore,
    memory::{MemoryError, MemoryInput, MemoryStore, Scope},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::VecDeque,
    fs::{self, File},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::ExitCode,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    task: CodingTask,
    policy: CodingPolicy,
    catalog: ToolCatalog,
    acceptance: AcceptanceSpec,
    profile: Option<LocalProfile>,
    worker: Option<PathBuf>,
    artifact: Option<PathBuf>,
    script: Option<Vec<String>>,
    memory_db: Option<PathBuf>,
    mode: EngineMode,
    #[serde(default = "legacy_context_version")]
    context_version: u8,
}
fn legacy_context_version() -> u8 {
    1
}
fn read<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let fd = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|e| e.to_string())?;
    let file = File::from(fd);
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("input must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take((MAX_INTERNAL + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    parse_internal(&bytes).map_err(|e| format!("invalid bounded input: {e:?}"))
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())?;
    File::open(path.parent().ok_or("record has no parent")?)
        .and_then(|p| p.sync_all())
        .map_err(|e| e.to_string())
}
fn scope(settings: &Settings) -> Result<Option<Scope>, String> {
    settings
        .task
        .data()
        .memory_collection
        .as_ref()
        .map(|collection| {
            Scope::new(
                settings.task.data().principal_id.as_str(),
                settings.task.data().project_id.as_str(),
                collection.as_str(),
            )
            .map_err(|e| e.to_string())
        })
        .transpose()
}
impl Settings {
    fn validate(&self, execution: bool) -> Result<(), String> {
        if !matches!(self.context_version, 1..=4) {
            return Err("unsupported coding context version".into());
        }
        self.policy
            .validate(&self.task)
            .map_err(|e| format!("invalid coding policy: {e:?}"))?;
        if self
            .catalog
            .digest()
            .map_err(|e| format!("catalog: {e:?}"))?
            != self.task.data().tool_catalog_digest
            || self
                .acceptance
                .digest()
                .map_err(|e| format!("acceptance: {e:?}"))?
                != self.task.data().acceptance_digest
        {
            return Err("task/config digest mismatch".into());
        }
        if self.task.data().memory_collection.is_some() != self.memory_db.is_some() {
            return Err("memory scope requires an explicit private memory database".into());
        }
        match (&self.profile, &self.script, &self.worker, &self.artifact) {
            (Some(profile), None, Some(worker), Some(artifact)) => {
                profile.validate().map_err(|e| format!("profile: {e:?}"))?;
                if !is_local_coding_recipe(profile.prompt_recipe_digest.as_str())
                    || profile.digest().map_err(|e| format!("profile: {e:?}"))?
                        != self.task.data().model_profile_digest
                {
                    return Err("coding profile/recipe mismatch".into());
                }
                if execution {
                    if !worker.is_absolute()
                        || !artifact.is_absolute()
                        || sha256_file(worker).map_err(|e| e.to_string())?
                            != profile.backend_build_digest.as_str()
                    {
                        return Err("worker identity mismatch".into());
                    }
                    if profile.artifact_digest.as_str() != Artifact::Control.sha256() {
                        return Err("coding experiment requires pinned LFM3 Q4_K_M".into());
                    }
                    verify_artifact(artifact, Artifact::Control).map_err(|e| e.to_string())?;
                    crate::hardware::validate_cpu(
                        profile.cpu_identity.as_str(),
                        &profile.cpu_features_digest,
                    )
                    .map_err(|e| e.to_string())?;
                    crate::hardware::validate_gpu(&profile.device).map_err(|e| e.to_string())?;
                }
            }
            (None, Some(script), None, None)
                if !script.is_empty()
                    && script.len() <= 64
                    && script.iter().all(|s| s.len() <= MAX_MODEL_OUTPUT)
                    && self.task.data().model_profile_digest
                        == content_digest(b"coding-scripted/v1") => {}
            _ => return Err("choose exactly one native profile or controlled script".into()),
        }
        Ok(())
    }
    fn genesis(&self) -> Result<Genesis, String> {
        Ok(Genesis {
            mode: self.mode,
            engine_version: BoundedText::new(match (self.mode, self.context_version) {
                (EngineMode::Agent, 1) => "coding-agent/v3",
                (EngineMode::Harness, 1) => "coding-harness/v3",
                (EngineMode::Agent, 2) => "coding-agent/v3.1",
                (EngineMode::Harness, 2) => "coding-harness/v3.1",
                (EngineMode::Agent, 3) => "coding-agent/v3.2",
                (EngineMode::Harness, 3) => "coding-harness/v3.2",
                (EngineMode::Agent, 4) => "coding-agent/v3.3",
                (EngineMode::Harness, 4) => "coding-harness/v3.3",
                _ => return Err("unsupported coding context version".into()),
            })
            .map_err(|e| format!("engine: {e:?}"))?,
            require_publication_approval: self.policy.require_mutation_approval,
            policy_digest: self
                .policy
                .digest(&self.task)
                .map_err(|e| format!("policy: {e:?}"))?,
            local_profile: self.profile.clone(),
            coding_policy: Some(Box::new(self.policy.clone())),
        })
    }
}
fn command(operation: StoreOperation) -> Result<StoreCommand, String> {
    let bytes = canonical_bytes(&operation, MAX_INTERNAL).map_err(|e| format!("command: {e:?}"))?;
    StoreCommand::new(CommandData {
        schema_version: SchemaVersion::V3,
        command_id: CommandId::new(content_digest(&bytes).as_str())
            .map_err(|e| format!("command id: {e:?}"))?,
        operation,
    })
    .map_err(|e| format!("command: {e:?}"))
}
struct Driver {
    tools: CodingTools,
    native: Option<LocalInferenceDriver>,
    script: VecDeque<String>,
    memory: Option<(MemoryStore, Scope)>,
    store_path: PathBuf,
    run_dir: PathBuf,
    policy: CodingPolicy,
}
impl Driver {
    fn uncertain(&self, effect: &AuthorizedEffect, diagnostic: &str) -> DriverOutcome {
        let end = diagnostic.floor_char_boundary(MAX_TEXT);
        let value = serde_json::json!({"dispatch_id":effect.request().action.dispatch_id,"status":"uncertain","diagnostic":&diagnostic[..end]});
        let _ = write_new(
            &self.run_dir.join(format!(
                "uncertainty-{}.json",
                effect.request().action.dispatch_id.as_str()
            )),
            value.to_string().as_bytes(),
        );
        DriverOutcome::Uncertain
    }
    fn memory_live(&mut self) -> Result<bool, MemoryError> {
        match self.memory.as_mut() {
            Some((store, scope)) => store.has_grant(scope),
            None => Ok(true),
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
                .expect("digest is bounded identifier"),
            task_id: prepared.action.task_id.clone(),
            intent_id: prepared.intent_id.clone(),
            dispatch_id: prepared.action.dispatch_id.clone(),
            action_digest: prepared.action_digest.clone(),
            usage: ActualUsage::Known {
                usage: UsageVector {
                    model_requests: DecimalUsage::new(model.to_string()).expect("bounded counter"),
                    tool_calls: DecimalUsage::new(tools.to_string()).expect("bounded counter"),
                    spend_micros_usd: DecimalUsage::new("0").expect("zero"),
                },
            },
            output,
        });
        match receipt {
            Ok(receipt) => DriverOutcome::Receipt(Box::new(receipt)),
            Err(_) => DriverOutcome::Uncertain,
        }
    }
    fn failure(&self, effect: &AuthorizedEffect, diagnostic: &str) -> DriverOutcome {
        self.known(
            effect,
            ReceiptOutput::Failure {
                diagnostic: BoundedText::new(diagnostic).expect("static bounded diagnostic"),
            },
            0,
            0,
        )
    }
    fn selected_source(&self, task: &TaskId, id: &Digest) -> bool {
        let Ok(mut store) = SqliteStore::open_read_only(&self.store_path) else {
            return false;
        };
        let Ok(mut snapshot) = store.audit() else {
            return false;
        };
        let mut cursor = None;
        loop {
            let Ok(page) = snapshot.intents(task, cursor) else {
                return false;
            };
            for intent in page.records {
                if intent.selected_evidence.as_ref() == Some(id) {
                    return true;
                }
            }
            cursor = page.next;
            if cursor.is_none() {
                return false;
            }
        }
    }
    fn memory_action(
        &mut self,
        action: &CodingAction,
        dispatch: &Digest,
        task: &TaskId,
    ) -> ToolOutcome {
        if let CodingAction::MemoryPut {
            source_evidence: Some(id),
            ..
        } = action
            && !self.selected_source(task, id)
        {
            return self.memory_output(
                action,
                ToolStatus::Denied,
                "source evidence is not selected for this task",
                false,
            );
        }
        let Some((store, scope)) = self.memory.as_mut() else {
            return self.memory_output(action, ToolStatus::Denied, "memory not granted", false);
        };
        let result=match action {
            CodingAction::MemoryPut{content,source_evidence}=>store.put(scope,&MemoryInput{dispatch:dispatch.clone(),source_task:task.clone(),source_evidence:source_evidence.clone(),body:content.as_str().into()}).map(|entry|serde_json::json!({"stored":entry.content_hash,"trust":entry.trust,"source_task":entry.source_task})),
            CodingAction::MemorySearch{query,limit}=>store.search(scope,query.as_str(),usize::from((*limit).min(8))).map(|entries|serde_json::json!({"notes":entries.into_iter().map(|e|serde_json::json!({"content":e.body,"content_hash":e.content_hash,"trust":e.trust,"source_task":e.source_task,"source_evidence":e.source_evidence})).collect::<Vec<_>>()})),
            _=>return self.memory_output(action,ToolStatus::Denied,"not a memory action",false),
        };
        match result {
            Ok(value) => {
                let text = value.to_string();
                self.memory_output(action, ToolStatus::Succeeded, &text, false)
            }
            Err(MemoryError::Denied) => self.memory_output(
                action,
                ToolStatus::Denied,
                "memory grant revoked or absent",
                false,
            ),
            Err(MemoryError::InvalidInput) => {
                self.memory_output(action, ToolStatus::Failed, "invalid memory input", false)
            }
            Err(_) => {
                ToolOutcome::Uncertain("memory transaction or dispatch identity uncertain".into())
            }
        }
    }
    fn memory_output(
        &self,
        action: &CodingAction,
        status: ToolStatus,
        text: &str,
        truncated: bool,
    ) -> ToolOutcome {
        let end = text.floor_char_boundary(self.policy.max_output_bytes as usize);
        ToolOutcome::Known(CodingToolOutput {
            tool: action.tool(),
            status,
            body: BoundedText::new(&text[..end]).expect("policy-bounded output"),
            truncated: truncated || end < text.len(),
            exit_code: None,
            files: vec![],
            process_tree_closed: true,
        })
    }
}
impl EffectDriver for Driver {
    type Outcome = DriverOutcome;
    fn execute(&mut self, effect: AuthorizedEffect) -> DriverOutcome {
        let ApprovedRequest::Coding { step, .. } = &effect.request().request else {
            return self.failure(&effect, "unsupported coding request");
        };
        match step.as_ref() {
            CodingStep::Model { input_digest, .. } => {
                match self.memory_live() {
                    Ok(true) => {}
                    Ok(false) => {
                        return self.failure(&effect, "memory revoked before model dispatch");
                    }
                    Err(_) => return DriverOutcome::Uncertain,
                }
                if let Some(native) = self.native.as_mut() {
                    return native.execute(effect);
                }
                let Some(content) = self.script.pop_front() else {
                    return self.failure(&effect, "controlled script exhausted");
                };
                self.known(
                    &effect,
                    ReceiptOutput::Coding {
                        observation: Box::new(CodingObservation::ScriptedModel {
                            input_digest: input_digest.clone(),
                            content: BoundedText::new(content).expect("validated script"),
                        }),
                    },
                    1,
                    0,
                )
            }
            CodingStep::Tool { action, .. } => {
                let outcome = if matches!(
                    action,
                    CodingAction::MemoryPut { .. } | CodingAction::MemorySearch { .. }
                ) {
                    self.memory_action(
                        action,
                        &effect.request().action.dispatch_id,
                        &effect.request().action.task_id,
                    )
                } else {
                    self.tools
                        .execute(action, &effect.request().action.dispatch_id)
                };
                match outcome {
                    ToolOutcome::Known(output) => self.known(
                        &effect,
                        ReceiptOutput::Coding {
                            observation: Box::new(CodingObservation::Tool { output }),
                        },
                        0,
                        1,
                    ),
                    ToolOutcome::Uncertain(diagnostic) => self.uncertain(&effect, &diagnostic),
                }
            }
            CodingStep::Freeze { .. } => {
                match self.tools.freeze(&effect.request().action.dispatch_id) {
                    Ok(artifact) => self.known(
                        &effect,
                        ReceiptOutput::Coding {
                            observation: Box::new(CodingObservation::Artifact { artifact }),
                        },
                        0,
                        1,
                    ),
                    Err(_) => DriverOutcome::Uncertain,
                }
            }
        }
    }
}
struct Verifier {
    spec: AcceptanceSpec,
    catalog: ToolCatalog,
    run_dir: PathBuf,
}
impl CodingVerifier for Verifier {
    fn verify(
        &mut self,
        projection: &Projection,
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
        let mut runner = VerifierRunner::new(
            &self.catalog,
            artifact,
            &self.run_dir,
            &publication.data().dispatch_id,
        )
        .map_err(|_| DriveError::Operational)?;
        let expected_evaluator =
            coding_acceptance::evaluator_identity(&self.spec).map_err(|_| DriveError::Integrity)?;
        let report = coding_acceptance::evaluate(&self.spec, task, artifact, &mut runner)
            .map_err(|_| DriveError::Integrity)?;
        if report.evaluator_digest != expected_evaluator {
            return Err(DriveError::Integrity);
        }
        write_new(
            &self
                .run_dir
                .join(format!("acceptance-{}.json", report.report_digest.as_str())),
            &canonical_bytes(&report, MAX_INTERNAL).map_err(|_| DriveError::Invalid)?,
        )
        .map_err(|_| DriveError::Operational)?;
        if report
            .checks
            .iter()
            .any(|check| check.status == coding_acceptance::CheckStatus::Uncertain)
        {
            return Err(DriveError::Operational);
        }
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
                acceptance_digest: report.spec_digest,
                evaluator_digest: report.evaluator_digest,
                report_digest: report.report_digest,
            },
            report.accepted,
        ))
    }
}
// Inspect the retained private report without executing a candidate or consulting a model.
fn acceptance_report_digest(
    report: &coding_acceptance::AcceptanceReport,
) -> Result<Digest, String> {
    #[derive(Serialize)]
    struct Body<'a> {
        spec_digest: &'a Digest,
        task_digest: &'a Digest,
        tree_digest: &'a Digest,
        evaluator_digest: &'a Digest,
        checks: &'a [coding_acceptance::Check],
        accepted: bool,
    }
    let body = Body {
        spec_digest: &report.spec_digest,
        task_digest: &report.task_digest,
        tree_digest: &report.tree_digest,
        evaluator_digest: &report.evaluator_digest,
        checks: &report.checks,
        accepted: report.accepted,
    };
    let bytes = canonical_bytes(&body, MAX_INTERNAL).map_err(|e| format!("report body: {e:?}"))?;
    structural_digest("coding-acceptance-report/v1", &[&bytes])
        .map_err(|e| format!("report digest: {e:?}"))
}
fn inspect_acceptance_report(
    snapshot: &mut impl AuditSnapshot,
    projection: &Projection,
    run_dir: &Path,
) -> Result<Option<bool>, String> {
    let mut selected = None;
    let mut after = None;
    let mut seen = 0usize;
    loop {
        let page = snapshot
            .journal_page(after)
            .map_err(|e| format!("report journal: {e:?}"))?;
        for record in page.records {
            seen += 1;
            if seen > MAX_AUDIT_RECORDS {
                return Err("report audit capacity exceeded".into());
            }
            let JournalEvent::CommandCommitted { command, result } = &record.data().event;
            if let StoreOperation::RecordCodingVerification {
                task_id,
                selection,
                accepted,
                ..
            } = &command.data().operation
                && task_id == &projection.task.data().task_id
                && matches!(result, StoreResult::VerificationRecorded)
            {
                selected = Some((selection.clone(), *accepted));
            }
        }
        let Some(next) = page.next else { break };
        after = Some(next);
    }
    let Some((selection, accepted)) = selected else {
        return Ok((projection.state.phase == Phase::Completed).then_some(false));
    };
    let file = run_dir.join(format!(
        "acceptance-{}.json",
        selection.report_digest.as_str()
    ));
    let Ok(report) = read::<coding_acceptance::AcceptanceReport>(&file) else {
        return Ok(Some(false));
    };
    let Some(task) = projection.task.coding() else {
        return Ok(Some(false));
    };
    let digest = acceptance_report_digest(&report)?;
    Ok(Some(
        digest == selection.report_digest
            && report.report_digest == digest
            && report.task_digest == task.digest().map_err(|e| format!("task digest: {e:?}"))?
            && report.spec_digest == task.data().acceptance_digest
            && report.spec_digest == selection.acceptance_digest
            && report.tree_digest == selection.artifact_digest
            && report.evaluator_digest == selection.evaluator_digest
            && report.accepted == accepted
            && !report.checks.is_empty()
            && report
                .checks
                .iter()
                .all(|c| c.status != coding_acceptance::CheckStatus::Uncertain)
            && (!accepted
                || report
                    .checks
                    .iter()
                    .all(|c| c.status == coding_acceptance::CheckStatus::Passed)),
    ))
}
fn event_capture(path: &Path) -> Result<metron_inference_local::driver::EventSink, String> {
    use metron_inference_local::driver::WorkerEvent;
    use std::sync::{Arc, Mutex};
    let file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    let file = Arc::new(Mutex::new(file));
    let began = std::time::Instant::now();
    Ok(Arc::new(move |event| {
        let value = match event {
            WorkerEvent::Spawned { pid, .. } => serde_json::json!({"kind":"spawned","pid":pid}),
            WorkerEvent::Dispatching { dispatch_id, .. } => {
                serde_json::json!({"kind":"dispatching","dispatch_id":dispatch_id})
            }
            WorkerEvent::Arrived {
                dispatch_id,
                in_flight,
            } => {
                serde_json::json!({"kind":"arrived","dispatch_id":dispatch_id,"in_flight":in_flight})
            }
            WorkerEvent::Sent { dispatch_id } => {
                serde_json::json!({"kind":"sent","dispatch_id":dispatch_id})
            }
            WorkerEvent::Loading { dispatch_id } => {
                serde_json::json!({"kind":"loading","dispatch_id":dispatch_id})
            }
            WorkerEvent::Loaded { dispatch_id } => {
                serde_json::json!({"kind":"loaded","dispatch_id":dispatch_id})
            }
            WorkerEvent::Tokenized {
                dispatch_id,
                token_count,
                token_ids_digest,
                rendered_digest,
            } => {
                serde_json::json!({"kind":"tokenized","dispatch_id":dispatch_id,"token_count":token_count,"token_ids_digest":token_ids_digest,"rendered_digest":rendered_digest})
            }
            WorkerEvent::Started { dispatch_id } => {
                serde_json::json!({"kind":"started","dispatch_id":dispatch_id})
            }
            WorkerEvent::Delta {
                dispatch_id,
                sequence,
                text,
                reasoning,
                ..
            } => {
                serde_json::json!({"kind":"delta","dispatch_id":dispatch_id,"sequence":sequence,"text":text,"reasoning":reasoning})
            }
            WorkerEvent::Completed {
                dispatch_id,
                prompt_time_us,
                completion_time_us,
                ..
            } => {
                serde_json::json!({"kind":"completed","dispatch_id":dispatch_id,"prompt_time_us":prompt_time_us,"completion_time_us":completion_time_us})
            }
            WorkerEvent::Exited { pid } => serde_json::json!({"kind":"exited","pid":pid}),
        };
        if let Ok(mut file) = file.lock() {
            let line = serde_json::json!({"elapsed_ms":began.elapsed().as_millis(),"event":value});
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }
    }))
}
fn flag(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}
fn required(args: &[String], name: &str) -> Result<String, String> {
    flag(args, name).ok_or_else(|| format!("missing {name}"))
}
fn run(args: &[String]) -> Result<serde_json::Value, String> {
    let action = args.first().map(String::as_str).unwrap_or("");
    if action == "identities" {
        let catalog: ToolCatalog = read(Path::new(&required(args, "--catalog")?))?;
        let profile: LocalProfile = read(Path::new(&required(args, "--profile")?))?;
        let acceptance: AcceptanceSpec = read(Path::new(&required(args, "--acceptance")?))?;
        return Ok(
            serde_json::json!({"outcome":"succeeded","tool_catalog_digest":catalog.digest().map_err(|e|format!("catalog: {e:?}"))?,"model_profile_digest":profile.digest().map_err(|e|format!("profile: {e:?}"))?,"acceptance_digest":acceptance.digest().map_err(|e|format!("acceptance: {e:?}"))?,"evaluator_digest":coding_acceptance::evaluator_identity(&acceptance).map_err(|e|format!("evaluator: {e:?}"))?,"coding_recipe_digest":profile.prompt_recipe_digest}),
        );
    }
    if matches!(action, "memory-grant" | "memory-revoke") {
        let path = PathBuf::from(required(args, "--db")?);
        let mut store = if path.exists() {
            MemoryStore::open_existing(&path)
        } else {
            MemoryStore::create(&path)
        }
        .map_err(|e| e.to_string())?;
        let scope = Scope::new(
            required(args, "--principal")?,
            required(args, "--project")?,
            required(args, "--collection")?,
        )
        .map_err(|e| e.to_string())?;
        if action == "memory-grant" {
            store.grant(&scope)
        } else {
            store.revoke(&scope)
        }
        .map_err(|e| e.to_string())?;
        return Ok(serde_json::json!({"outcome":"succeeded","action":action}));
    }
    let run_dir = PathBuf::from(required(args, "--run-dir")?);
    let fresh = action == "experiment";
    if !fresh && !matches!(action, "resume" | "audit") {
        return Err(
            "expected coding experiment, resume, audit, memory-grant or memory-revoke".into(),
        );
    }
    let settings: Settings = if fresh {
        if !args.iter().any(|a| a == "--experimental") {
            return Err("unqualified coding attempts require --experimental".into());
        }
        Settings {
            context_version: 4,
            task: read(Path::new(&required(args, "--task")?))?,
            policy: read(Path::new(&required(args, "--policy")?))?,
            catalog: read(Path::new(&required(args, "--catalog")?))?,
            acceptance: read(Path::new(&required(args, "--acceptance")?))?,
            profile: flag(args, "--profile")
                .map(|p| read(Path::new(&p)))
                .transpose()?,
            worker: flag(args, "--worker").map(PathBuf::from),
            artifact: flag(args, "--artifact").map(PathBuf::from),
            script: flag(args, "--script")
                .map(|p| read(Path::new(&p)))
                .transpose()?,
            memory_db: flag(args, "--memory-db").map(PathBuf::from),
            mode: match flag(args, "--mode").as_deref() {
                None | Some("agent") => EngineMode::Agent,
                Some("harness") => EngineMode::Harness,
                _ => return Err("invalid coding mode".into()),
            },
        }
    } else {
        read(&run_dir.join("coding-settings.json"))?
    };
    settings.validate(action != "audit")?;
    let workspace = PathBuf::from(settings.policy.workspace_root.as_str());
    if !workspace.is_absolute()
        || !run_dir.is_absolute()
        || workspace.starts_with(&run_dir)
        || run_dir.starts_with(&workspace)
    {
        return Err("workspace and private run directory must be separate absolute paths".into());
    }
    if action != "audit" {
        let canonical_workspace = fs::canonicalize(&workspace).map_err(|e| e.to_string())?;
        let canonical_run = if fresh {
            fs::canonicalize(run_dir.parent().ok_or("invalid run path")?)
                .map_err(|e| e.to_string())?
                .join(run_dir.file_name().ok_or("invalid run path")?)
        } else {
            fs::canonicalize(&run_dir).map_err(|e| e.to_string())?
        };
        if canonical_workspace.starts_with(&canonical_run)
            || canonical_run.starts_with(&canonical_workspace)
        {
            return Err("workspace aliases private run directory".into());
        }
        if let Some(memory) = &settings.memory_db
            && fs::canonicalize(memory)
                .map_err(|e| e.to_string())?
                .starts_with(&canonical_workspace)
        {
            return Err("memory database must be outside model workspace".into());
        }
    }
    if fresh {
        if let Some(scope) = scope(&settings)? {
            let mut memory =
                MemoryStore::open_existing(settings.memory_db.as_ref().ok_or("missing memory DB")?)
                    .map_err(|e| e.to_string())?;
            if !memory.has_grant(&scope).map_err(|e| e.to_string())? {
                return Err("memory scope needs explicit operator grant".into());
            }
        }
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&run_dir)
            .map_err(|e| e.to_string())?;
        write_new(
            &run_dir.join("coding-settings.json"),
            &canonical_bytes(&settings, MAX_INTERNAL).map_err(|e| format!("settings: {e:?}"))?,
        )?;
        write_new(
            &run_dir.join("task.json"),
            &canonical_bytes(
                &Task::new_coding(settings.task.clone()).map_err(|e| format!("task: {e:?}"))?,
                MAX_ENVELOPE,
            )
            .map_err(|e| format!("task: {e:?}"))?,
        )?;
    }
    let store_path = run_dir.join("runtime.sqlite");
    let mut store = if fresh {
        SqliteStore::create(&store_path)
    } else if action == "audit" {
        SqliteStore::open_read_only(&store_path)
    } else {
        SqliteStore::open_existing(&store_path)
    }
    .map_err(|e| format!("store: {e:?}"))?;
    if fresh {
        store
            .apply(command(StoreOperation::CreateTask {
                task: Task::new_coding(settings.task.clone())
                    .map_err(|e| format!("task: {e:?}"))?,
                genesis: settings.genesis()?,
            })?)
            .map_err(|e| format!("create: {e:?}"))?;
    }
    let task_id = &settings.task.data().task_id;
    let projection = {
        let mut snapshot = store.audit().map_err(|e| format!("snapshot: {e:?}"))?;
        snapshot
            .projection(task_id)
            .map_err(|e| format!("projection: {e:?}"))?
    };
    if projection.task.coding() != Some(&settings.task)
        || projection.genesis != settings.genesis()?
    {
        return Err("recorded task/settings mismatch".into());
    }
    let engine =
        crate::cognition(&projection.genesis, &projection.task).map_err(|(_, e)| e.to_string())?;
    if action != "audit" {
        let tools = CodingTools::new(
            &settings.task,
            &settings.policy,
            &settings.catalog,
            &workspace,
            &run_dir,
        )?;
        let native = match (&settings.profile, &settings.worker, &settings.artifact) {
            (Some(p), Some(w), Some(a)) => Some(
                LocalInferenceDriver::new(
                    w.clone(),
                    a.clone(),
                    p.clone(),
                    Some(event_capture(&run_dir.join(format!(
                        "worker-events-revision-{}.jsonl",
                        projection.state.revision.get()
                    )))?),
                )
                .map_err(|e| e.to_string())?,
            ),
            _ => None,
        };
        // Never replay a script from the start: resume consumes only replies
        // after model receipts already committed for this task.
        let observed = projection.task_account.posted.model_requests.get() as usize;
        let script = settings
            .script
            .clone()
            .unwrap_or_default()
            .into_iter()
            .skip(observed)
            .collect();
        let memory = scope(&settings)?
            .map(|scope| {
                MemoryStore::open_existing(
                    settings.memory_db.as_ref().expect("validated memory path"),
                )
                .map(|store| (store, scope))
                .map_err(|e| e.to_string())
            })
            .transpose()?;
        let mut driver = Driver {
            tools,
            native,
            script,
            memory,
            store_path: store_path.clone(),
            run_dir: run_dir.clone(),
            policy: settings.policy.clone(),
        };
        let mut verifier = Verifier {
            spec: settings.acceptance.clone(),
            catalog: settings.catalog.clone(),
            run_dir: run_dir.clone(),
        };
        let result = orchestrator::drive_coding_with_driver(
            &mut store,
            task_id,
            engine.as_ref(),
            &run_dir,
            &mut NoObserver,
            &mut driver,
            &mut verifier,
        );
        if let Some(native) = driver.native.as_mut() {
            native.shutdown();
        }
        result.map_err(|e| format!("coding drive: {e:?}"))?;
    }
    let mut snapshot = store
        .audit()
        .map_err(|e| format!("audit snapshot: {e:?}"))?;
    let audit = metron_harness::audit::verify(&mut snapshot, task_id)
        .map_err(|e| format!("audit: {e:?}"))?;
    let behavior = metron_harness::replay::behavior(&mut snapshot, task_id, engine.as_ref())
        .map_err(|e| format!("behavior replay: {e:?}"))?;
    let projection = snapshot
        .projection(task_id)
        .map_err(|e| format!("final projection: {e:?}"))?;
    let acceptance_report_valid = inspect_acceptance_report(&mut snapshot, &projection, &run_dir)?;
    let artifact_valid = match projection
        .verification
        .publication
        .as_ref()
        .map(|r| &r.data().output)
    {
        Some(ReceiptOutput::Coding { observation }) => match observation.as_ref() {
            CodingObservation::Artifact { artifact } => {
                Some(crate::coding_tools::inspect_artifact(&run_dir, artifact).unwrap_or(false))
            }
            _ => Some(false),
        },
        _ => None,
    };
    Ok(
        serde_json::json!({"outcome":if !audit.chain_valid||!audit.accounting_valid||!audit.projection_valid||!behavior||artifact_valid==Some(false)||acceptance_report_valid==Some(false) {"integrity_failure"}else if projection.state.phase==Phase::Completed {"completed"}else if projection.state.phase==Phase::Rejected {"rejected"}else{"blocked"},"experimental":true,"production_profile_selected":false,"mode":settings.mode,"task_id":task_id,"phase":projection.state.phase,"replay_head":projection.replay_head,"accounting":projection.task_account,"audit":{"chain_valid":audit.chain_valid,"accounting_valid":audit.accounting_valid,"projection_valid":audit.projection_valid},"behavior_replay":behavior,"artifact_valid":artifact_valid,"acceptance_report_valid":acceptance_report_valid,"model":settings.profile.as_ref().map(|p|&p.auxiliary_repository),"artifact_repository":settings.profile.as_ref().map(|p|&p.artifact_repository),"device":settings.profile.as_ref().map(|p|&p.device)}),
    )
}
pub fn execute(args: &[String]) -> ExitCode {
    match run(args) {
        Ok(report) => {
            let success = report["outcome"] == "completed" || report["outcome"] == "succeeded";
            println!("{report}");
            ExitCode::from(if success { 0 } else { 4 })
        }
        Err(error) => {
            println!(
                "{}",
                serde_json::json!({"outcome":"error","diagnostic":error})
            );
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metron_protocol::toolkit::{ToolId, ToolPin};
    pub(super) fn setup(
        name: &str,
        mode: EngineMode,
        allowed: Vec<CodingTool>,
        approval: bool,
    ) -> (PathBuf, Settings, SqliteStore) {
        setup_at(&std::env::temp_dir(), name, mode, allowed, approval)
    }
    fn setup_at(
        parent: &Path,
        name: &str,
        mode: EngineMode,
        allowed: Vec<CodingTool>,
        approval: bool,
    ) -> (PathBuf, Settings, SqliteStore) {
        let root = parent.join(format!("metron-code-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let workspace = root.join("workspace");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&workspace)
            .unwrap();
        let run = root.join("run");
        fs::DirBuilder::new().mode(0o700).create(&run).unwrap();
        let catalog = ToolCatalog {
            schema_version: 1,
            tools: vec![ToolPin {
                tool: ToolId::Cargo,
                executable: BoundedText::new("/unavailable-test-tool").unwrap(),
                sha256: content_digest(b"unused"),
                version_prefix: BoundedText::new("unused").unwrap(),
            }],
        };
        let acceptance = AcceptanceSpec {
            version: 1,
            kind: coding_acceptance::AcceptanceKind::CalculatorV1,
        };
        let task=CodingTask::new(CodingTaskData{schema_version:3,family:CodingFamily::CodingV1,task_id:TaskId::new(name).unwrap(),principal_id:PrincipalId::new("operator").unwrap(),project_id:BoundedText::new("tests").unwrap(),objective:BoundedText::new("Exercise controlled coding proposals; completion requires the independent evaluator").unwrap(),acceptance_digest:acceptance.digest().unwrap(),tool_catalog_digest:catalog.digest().unwrap(),model_profile_digest:content_digest(b"coding-scripted/v1"),memory_collection:None,budget:BudgetRequest{max_model_requests:SqlU64::new(8).unwrap(),max_tool_calls:SqlU64::new(8).unwrap(),max_spend_micros_usd:SqlU64::new(0).unwrap(),max_elapsed_ms:None},max_decision_steps:16}).unwrap();
        let policy = CodingPolicy {
            schema_version: 1,
            principal_id: task.data().principal_id.clone(),
            project_id: task.data().project_id.clone(),
            workspace_root: BoundedText::new(workspace.to_str().unwrap()).unwrap(),
            memory_collection: None,
            allowed_tools: allowed,
            allowed_programs: vec![],
            require_mutation_approval: approval,
            max_file_bytes: 65536,
            max_output_bytes: 4096,
            command_timeout_ms: 1000,
            command_memory_bytes: SqlU64::new(256 * 1024 * 1024).unwrap(),
            max_processes: 16,
        };
        let settings = Settings {
            context_version: 4,
            task,
            policy,
            catalog,
            acceptance,
            profile: None,
            worker: None,
            artifact: None,
            script: Some(vec!["placeholder".into()]),
            memory_db: None,
            mode,
        };
        let mut store = SqliteStore::create(run.join("runtime.sqlite")).unwrap();
        store
            .apply(
                command(StoreOperation::CreateTask {
                    task: Task::new_coding(settings.task.clone()).unwrap(),
                    genesis: settings.genesis().unwrap(),
                })
                .unwrap(),
            )
            .unwrap();
        (root, settings, store)
    }
    pub(super) fn driver(root: &Path, s: &Settings, replies: &[&str]) -> Driver {
        Driver {
            tools: CodingTools::new(
                &s.task,
                &s.policy,
                &s.catalog,
                &root.join("workspace"),
                &root.join("run"),
            )
            .unwrap(),
            native: None,
            script: replies.iter().map(|s| (*s).into()).collect(),
            memory: None,
            store_path: root.join("run/runtime.sqlite"),
            run_dir: root.join("run"),
            policy: s.policy.clone(),
        }
    }
    pub(super) fn assert_audits(store: &mut SqliteStore, s: &Settings) {
        let mut snapshot = store.audit().unwrap();
        let result = metron_harness::audit::verify(&mut snapshot, &s.task.data().task_id).unwrap();
        assert!(
            result.chain_valid && result.accounting_valid && result.projection_valid,
            "{result:?}"
        );
        let projection = snapshot.projection(&s.task.data().task_id).unwrap();
        let engine = crate::cognition(&projection.genesis, &projection.task).unwrap();
        assert!(
            metron_harness::replay::behavior(
                &mut snapshot,
                &s.task.data().task_id,
                engine.as_ref()
            )
            .unwrap()
        );
    }
    #[test]
    fn both_modes_record_plan_edit_finish_without_self_certifying_success() {
        for mode in [EngineMode::Agent, EngineMode::Harness] {
            let name = if mode == EngineMode::Agent {
                "agent-code"
            } else {
                "harness-code"
            };
            let (root, s, mut store) = setup(
                name,
                mode,
                vec![CodingTool::Edit, CodingTool::Finish],
                false,
            );
            let mut driver = driver(
                &root,
                &s,
                &[
                    r#"{"tool":"plan","steps":["Create a controlled note"]}"#,
                    r#"{"tool":"edit","path":"README.md","expected_digest":null,"content":"Controlled proposal only\n"}"#,
                    r#"{"tool":"finish","summary":"request verification"}"#,
                ],
            );
            let task = Task::new_coding(s.task.clone()).unwrap();
            let engine = crate::cognition(&s.genesis().unwrap(), &task).unwrap();
            let status = orchestrator::drive_with_driver(
                &mut store,
                &s.task.data().task_id,
                engine.as_ref(),
                &root.join("run"),
                &mut NoObserver,
                &mut driver,
            )
            .unwrap();
            assert_eq!(status, orchestrator::DriveStatus::Blocked);
            let p = store
                .audit()
                .unwrap()
                .projection(&s.task.data().task_id)
                .unwrap();
            assert_eq!(p.state.phase, Phase::AwaitingVerification);
            assert_eq!(p.task_account.posted.model_requests.get(), 3);
            assert_eq!(p.task_account.posted.tool_calls.get(), 2);
            assert_eq!(p.task_account.reserved.model_requests.get(), 0);
            assert_eq!(
                fs::read_to_string(root.join("workspace/README.md")).unwrap(),
                "Controlled proposal only\n"
            );
            assert_audits(&mut store, &s);
            drop(driver);
            fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn denied_mutation_precedes_approval_and_never_runs_a_tool() {
        let (root, s, mut store) = setup(
            "denied-edit",
            EngineMode::Agent,
            vec![CodingTool::Read, CodingTool::Finish],
            true,
        );
        let mut driver = driver(
            &root,
            &s,
            &[
                r#"{"tool":"plan","steps":["Try a forbidden edit"]}"#,
                r#"{"tool":"edit","path":"denied.txt","expected_digest":null,"content":"must not exist"}"#,
            ],
        );
        let task = Task::new_coding(s.task.clone()).unwrap();
        let engine = crate::cognition(&s.genesis().unwrap(), &task).unwrap();
        assert_eq!(
            orchestrator::drive_with_driver(
                &mut store,
                &s.task.data().task_id,
                engine.as_ref(),
                &root.join("run"),
                &mut NoObserver,
                &mut driver
            )
            .unwrap(),
            orchestrator::DriveStatus::Denied
        );
        assert!(!root.join("workspace/denied.txt").exists());
        let mut snap = store.audit().unwrap();
        let p = snap.projection(&s.task.data().task_id).unwrap();
        assert_eq!(p.task_account.posted.model_requests.get(), 2);
        assert_eq!(p.task_account.posted.tool_calls.get(), 0);
        let page = snap.intents(&s.task.data().task_id, None).unwrap();
        assert!(page.records.iter().all(|i| i.approval.is_none()));
        drop(snap);
        assert_audits(&mut store, &s);
        drop(driver);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[ignore = "requires pinned local tools, user systemd, bubblewrap and disk scratch"]
    fn admitted_shared_toolkit_records_real_receipts_and_replays() {
        let catalog_path =
            std::env::var("METRON_CODING_PROBE_CATALOG").expect("pinned probe catalog");
        let (root, mut s, old_store) = setup_at(
            Path::new(&catalog_path)
                .parent()
                .expect("disk-backed catalog directory"),
            "shared-toolkit",
            EngineMode::Harness,
            vec![
                CodingTool::Edit,
                CodingTool::Search,
                CodingTool::JjSnapshot,
                CodingTool::JjDiff,
                CodingTool::Smt,
                CodingTool::Lean,
                CodingTool::Finish,
            ],
            false,
        );
        drop(old_store);
        s.catalog = read(Path::new(&catalog_path)).unwrap();
        let mut data = s.task.data().clone();
        data.tool_catalog_digest = s.catalog.digest().unwrap();
        s.task = CodingTask::new(data).unwrap();
        s.policy.command_timeout_ms = 10000;
        s.policy.command_memory_bytes = SqlU64::new(1024 * 1024 * 1024).unwrap();
        s.policy.max_processes = 64;
        let db = root.join("run/shared-runtime.sqlite");
        let mut store = SqliteStore::create(&db).unwrap();
        store
            .apply(
                command(StoreOperation::CreateTask {
                    task: Task::new_coding(s.task.clone()).unwrap(),
                    genesis: s.genesis().unwrap(),
                })
                .unwrap(),
            )
            .unwrap();
        fs::write(root.join("workspace/README.md"), "seed").unwrap();
        let edit=serde_json::json!({"tool":"edit","path":"README.md","expected_digest":content_digest(b"seed"),"content":"edited"}).to_string();
        let replies = [
            r#"{"tool":"plan","steps":["Inspect and change a note using the shared toolkit"]}"#,
            r#"{"tool":"jj_snapshot"}"#,
            &edit,
            r#"{"tool":"search","query":"edited","directory":null}"#,
            r#"{"tool":"jj_diff"}"#,
            r#"{"tool":"smt","problem":"(set-logic QF_UF) (declare-const p Bool) (assert p) (check-sat)"}"#,
            r#"{"tool":"lean","statement":"1 + 1 = 2","proof":"  decide"}"#,
            r#"{"tool":"finish","summary":"request independent review"}"#,
        ];
        let mut driver = driver(&root, &s, &replies);
        driver.store_path = db;
        let task = Task::new_coding(s.task.clone()).unwrap();
        let engine = crate::cognition(&s.genesis().unwrap(), &task).unwrap();
        assert_eq!(
            orchestrator::drive_with_driver(
                &mut store,
                &s.task.data().task_id,
                engine.as_ref(),
                &root.join("run"),
                &mut NoObserver,
                &mut driver
            )
            .unwrap(),
            orchestrator::DriveStatus::Blocked
        );
        let mut snapshot = store.audit().unwrap();
        let p = snapshot.projection(&s.task.data().task_id).unwrap();
        assert_eq!(p.state.phase, Phase::AwaitingVerification);
        assert_eq!(p.task_account.posted.model_requests.get(), 8);
        assert_eq!(p.task_account.posted.tool_calls.get(), 7);
        let mut tools = Vec::new();
        let mut cursor = None;
        loop {
            let page = snapshot.intents(&s.task.data().task_id, cursor).unwrap();
            for intent in page.records {
                for e in snapshot.evidence(&intent.intent_id, None).unwrap().records {
                    if let ReceiptOutput::Coding { observation } = &e.receipt.data().output
                        && let CodingObservation::Tool { output } = observation.as_ref()
                    {
                        assert_eq!(
                            output.status,
                            ToolStatus::Succeeded,
                            "{:?}: {}",
                            output.tool,
                            output.body.as_str()
                        );
                        assert!(output.process_tree_closed);
                        tools.push(output.tool);
                    }
                }
            }
            let Some(next) = page.next else { break };
            cursor = Some(next);
        }
        assert_eq!(
            tools,
            vec![
                CodingTool::JjSnapshot,
                CodingTool::Edit,
                CodingTool::Search,
                CodingTool::JjDiff,
                CodingTool::Smt,
                CodingTool::Lean
            ]
        );
        drop(snapshot);
        assert_audits(&mut store, &s);
        drop(driver);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn audit_is_read_only_and_does_not_require_the_original_workspace() {
        let (root, s, store) = setup(
            "audit-missing-workspace",
            EngineMode::Harness,
            vec![CodingTool::Finish],
            false,
        );
        write_new(
            &root.join("run/coding-settings.json"),
            &canonical_bytes(&s, MAX_INTERNAL).unwrap(),
        )
        .unwrap();
        drop(store);
        fs::remove_dir(root.join("workspace")).unwrap();
        let db = root.join("run/runtime.sqlite");
        let before = fs::read(&db).unwrap();
        let report = run(&[
            "audit".into(),
            "--run-dir".into(),
            root.join("run").to_str().unwrap().into(),
        ])
        .unwrap();
        assert_eq!(report["audit"]["chain_valid"], true);
        assert_eq!(report["audit"]["accounting_valid"], true);
        assert_eq!(report["audit"]["projection_valid"], true);
        assert_eq!(report["behavior_replay"], true);
        assert_eq!(fs::read(db).unwrap(), before);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn malformed_or_unplanned_model_output_is_charged_and_rejected() {
        for (name, reply) in [
            ("invalid-json", "not json"),
            (
                "unplanned",
                r#"{"tool":"finish","summary":"I declare success"}"#,
            ),
        ] {
            let (root, s, mut store) =
                setup(name, EngineMode::Harness, vec![CodingTool::Finish], false);
            let mut driver = driver(&root, &s, &[reply]);
            let task = Task::new_coding(s.task.clone()).unwrap();
            let engine = crate::cognition(&s.genesis().unwrap(), &task).unwrap();
            assert_eq!(
                orchestrator::drive_with_driver(
                    &mut store,
                    &s.task.data().task_id,
                    engine.as_ref(),
                    &root.join("run"),
                    &mut NoObserver,
                    &mut driver
                )
                .unwrap(),
                orchestrator::DriveStatus::Rejected
            );
            let p = store
                .audit()
                .unwrap()
                .projection(&s.task.data().task_id)
                .unwrap();
            assert_eq!(p.task_account.posted.model_requests.get(), 1);
            assert_eq!(p.task_account.posted.tool_calls.get(), 0);
            assert_audits(&mut store, &s);
            drop(driver);
            fs::remove_dir_all(root).unwrap();
        }
    }
}

#[cfg(test)]
mod recovery_tests;
