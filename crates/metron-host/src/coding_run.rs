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
    memory::{
        MemoryError, MemoryInput, MemoryStore, Scope, SourceAnnotationInput, SourceAnnotationKind,
    },
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
    #[serde(default)]
    resource_budget_required: bool,
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
/// Replace only complete settings; stale temporary bytes never become authority.
fn replace_budget_settings(run_dir: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    struct OwnedTemp(PathBuf);
    impl Drop for OwnedTemp {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let directory = File::open(run_dir).map_err(|e| e.to_string())?;
    for _ in 0..32 {
        let nonce = NEXT_TEMP
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| "budget settings temporary counter exhausted")?;
        let path = run_dir.join(format!(
            ".coding-settings-budget-{}-{nonce}.tmp",
            std::process::id()
        ));
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("budget settings temporary file: {error}")),
        };
        let owned = OwnedTemp(path);
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| format!("budget settings write: {e}"))?;
        fs::rename(&owned.0, run_dir.join("coding-settings.json"))
            .map_err(|e| format!("budget settings replace: {e}"))?;
        // A failure here leaves the complete replacement visible but durability unknown.
        // The caller returns before constructing any model driver or dispatching effects.
        directory
            .sync_all()
            .map_err(|e| format!("budget settings directory sync: {e}"))?;
        return Ok(());
    }
    Err(
        "budget settings temporary name collisions; existing files preserved, retry required"
            .into(),
    )
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
        if !matches!(self.context_version, 1..=11) {
            return Err("unsupported coding context version".into());
        }
        if self
            .memory_db
            .as_ref()
            .is_some_and(|path| !path.is_absolute())
        {
            return Err("memory database path must be absolute".into());
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
                (EngineMode::Agent, 5) => "coding-agent/v3.4",
                (EngineMode::Harness, 5) => "coding-harness/v3.4",
                (EngineMode::Agent, 6) => "coding-agent/v3.5",
                (EngineMode::Harness, 6) => "coding-harness/v3.5",
                (EngineMode::Agent, 7) => "coding-agent/v3.6",
                (EngineMode::Harness, 7) => "coding-harness/v3.6",
                (EngineMode::Agent, 8) => "coding-agent/v3.7",
                (EngineMode::Agent, 11) => "coding-agent/v3.10",
                (EngineMode::Harness, 11) => "coding-harness/v3.10",
                (EngineMode::Agent, 10) => "coding-agent/v3.9",
                (EngineMode::Harness, 10) => "coding-harness/v3.9",
                (EngineMode::Agent, 9) => "coding-agent/v3.8",
                (EngineMode::Harness, 9) => "coding-harness/v3.8",
                (EngineMode::Harness, 8) => "coding-harness/v3.7",
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
    context_version: u8,
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
    fn memory_search_v9(
        &mut self,
        action: &CodingAction,
    ) -> (ToolOutcome, Option<MemorySelection>) {
        let CodingAction::MemorySearch { query, limit } = action else {
            unreachable!()
        };
        let Some((store, scope)) = self.memory.as_mut() else {
            return (
                self.memory_output(action, ToolStatus::Denied, "memory not granted", false),
                None,
            );
        };
        let page =
            match store.search_candidates(scope, query.as_str(), usize::from((*limit).min(8))) {
                Ok(page) => page,
                Err(MemoryError::Denied) => {
                    return (
                        self.memory_output(
                            action,
                            ToolStatus::Denied,
                            "memory grant revoked or absent",
                            false,
                        ),
                        None,
                    );
                }
                Err(MemoryError::InvalidInput) => {
                    return (
                        self.memory_output(
                            action,
                            ToolStatus::Failed,
                            "invalid memory input",
                            false,
                        ),
                        None,
                    );
                }
                Err(_) => {
                    return (
                        ToolOutcome::Uncertain("memory transaction uncertain".into()),
                        None,
                    );
                }
            };
        let mut selection = MemorySelection {
            sources: Vec::new(),
            requested_limit: *limit,
            candidate_limit: page.requested_limit as u8,
            output_budget_bytes: self.policy.max_output_bytes.min(700),
            excerpt_limit_bytes: 256,
            has_more: page.has_more,
            omitted_known: 0,
        };
        let mut excerpts = Vec::new();
        let budget = (self.policy.max_output_bytes as usize).min(700);
        for (i, entry) in page.entries.into_iter().enumerate() {
            let annotations: Vec<_> = entry
                .annotations
                .into_iter()
                .map(|annotation| {
                    let (stale, related_dispatch) = match annotation.kind {
                        SourceAnnotationKind::Stale => (true, None),
                        SourceAnnotationKind::ConflictsWith { related_dispatch } => {
                            (false, Some(related_dispatch))
                        }
                    };
                    MemoryAnnotationRef {
                        dispatch: annotation.dispatch,
                        source_evidence: annotation.source_evidence,
                        reason_digest: content_digest(annotation.reason.as_bytes()),
                        target_dispatch: annotation.target_dispatch,
                        related_dispatch,
                        stale,
                    }
                })
                .collect();
            let end = entry.body.floor_char_boundary(entry.body.len().min(256));
            let excerpt = &entry.body[..end];
            let visible = serde_json::json!({"slot":i,"excerpt":excerpt,"unverified":true,"stale":annotations.iter().any(|a|a.stale),"conflict":annotations.iter().any(|a|a.related_dispatch.is_some()),"truncated":end<entry.body.len()});
            let mut candidate = excerpts.clone();
            candidate.push(visible);
            let remaining = (page.requested_limit - candidate.len()) as u8;
            let value = serde_json::json!({"notes":candidate,"omitted_known":remaining,"has_more":page.has_more});
            let include = value.to_string().len() <= budget;
            if include {
                excerpts = candidate;
            }
            selection.sources.push(MemorySource {
                slot: i as u8,
                dispatch: entry.dispatch,
                content_hash: entry.content_hash,
                source_task: entry.source_task,
                source_evidence: entry.source_evidence,
                trust: BoundedText::new(entry.trust).expect("known trust label"),
                annotations,
                body_bytes: entry.body.len() as u32,
                excerpt: include.then(|| BoundedText::new(excerpt).expect("bounded excerpt")),
            });
        }
        selection.omitted_known = selection
            .sources
            .iter()
            .filter(|s| s.excerpt.is_none())
            .count() as u8;
        let value = serde_json::json!({"notes":excerpts,"omitted_known":selection.omitted_known,"has_more":selection.has_more}).to_string();
        if value.len() > budget || selection.validate().is_err() {
            return (
                self.memory_output(
                    action,
                    ToolStatus::Failed,
                    "memory result exceeds output budget",
                    false,
                ),
                None,
            );
        }
        (
            self.memory_output(action, ToolStatus::Succeeded, &value, false),
            Some(selection),
        )
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
                let (outcome, memory_selection) = if self.context_version >= 9
                    && matches!(action, CodingAction::MemorySearch { .. })
                {
                    self.memory_search_v9(action)
                } else if matches!(
                    action,
                    CodingAction::MemoryPut { .. } | CodingAction::MemorySearch { .. }
                ) {
                    (
                        self.memory_action(
                            action,
                            &effect.request().action.dispatch_id,
                            &effect.request().action.task_id,
                        ),
                        None,
                    )
                } else {
                    (
                        self.tools
                            .execute(action, &effect.request().action.dispatch_id),
                        None,
                    )
                };
                match outcome {
                    ToolOutcome::Known(output) => self.known(
                        &effect,
                        ReceiptOutput::Coding {
                            observation: Box::new(CodingObservation::Tool {
                                output,
                                memory_selection,
                            }),
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
fn external_resource_budget_path(path: &Path, workspace: &Path) -> Result<PathBuf, String> {
    // A model may edit any workspace file, including a formerly operator-written policy.
    // Resolve both boundaries before accepting policy contents; budgeted execution requires
    // an existing workspace so this check never creates state merely to validate authority.
    let workspace = fs::canonicalize(workspace)
        .map_err(|e| format!("resource budget requires an existing workspace: {e}"))?;
    if !workspace.is_dir() {
        return Err("resource budget requires a workspace directory".into());
    }
    let resolved = fs::canonicalize(path).map_err(|e| format!("resource budget path: {e}"))?;
    if resolved.starts_with(&workspace) {
        return Err("resource budget must resolve outside the mutable workspace".into());
    }
    Ok(resolved)
}
fn resource_budget_flag(args: &[String]) -> Result<Option<String>, String> {
    if args
        .iter()
        .any(|arg| arg.starts_with("--resource-budget") && arg != "--resource-budget")
    {
        return Err("expected --resource-budget FILE".into());
    }
    let positions = args
        .iter()
        .enumerate()
        .filter(|(_, arg)| arg.as_str() == "--resource-budget")
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    match positions.as_slice() {
        [] => Ok(None),
        [i] => args
            .get(i + 1)
            .filter(|value| !value.is_empty() && !value.starts_with("--"))
            .cloned()
            .map(Some)
            .ok_or("--resource-budget requires FILE".into()),
        _ => Err("duplicate --resource-budget".into()),
    }
}
fn flag(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}
fn required(args: &[String], name: &str) -> Result<String, String> {
    flag(args, name).ok_or_else(|| format!("missing {name}"))
}
fn annotate_memory(args: &[String]) -> Result<serde_json::Value, String> {
    let run_dir = PathBuf::from(required(args, "--run-dir")?);
    let settings: Settings = read(&run_dir.join("coding-settings.json"))?;
    settings.validate(false)?;
    let scope = scope(&settings)?.ok_or("task has no memory scope")?;
    let evidence =
        Digest::new(required(args, "--source-evidence")?).map_err(|_| "invalid source evidence")?;
    let mut runtime = SqliteStore::open_read_only(run_dir.join("runtime.sqlite"))
        .map_err(|_| "cannot open source journal")?;
    let mut snapshot = runtime.audit().map_err(|_| "cannot audit source journal")?;
    let task_id = &settings.task.data().task_id;
    let projection = snapshot
        .projection(task_id)
        .map_err(|_| "missing source task")?;
    if projection.task.coding() != Some(&settings.task)
        || projection.genesis != settings.genesis()?
    {
        return Err("source task/settings mismatch".into());
    }
    let audit =
        metron_harness::audit::verify(&mut snapshot, task_id).map_err(|_| "source audit failed")?;
    if !audit.chain_valid || !audit.accounting_valid || !audit.projection_valid {
        return Err("source audit invalid".into());
    }
    let engine =
        crate::cognition(&projection.genesis, &projection.task).map_err(|(_, e)| e.to_string())?;
    if !metron_harness::replay::behavior(&mut snapshot, task_id, engine.as_ref())
        .map_err(|_| "source replay failed")?
    {
        return Err("source replay invalid".into());
    }
    let mut found = false;
    let mut cursor = None;
    loop {
        let page = snapshot
            .intents(task_id, cursor)
            .map_err(|_| "source intents unavailable")?;
        for intent in page.records {
            if intent.selected_evidence.as_ref() != Some(&evidence) {
                continue;
            }
            let IntentKind::Coding { step } = &intent.intent.kind else {
                continue;
            };
            let CodingStep::Tool { action, .. } = step.as_ref() else {
                continue;
            };
            if matches!(
                action,
                CodingAction::MemoryPut { .. } | CodingAction::MemorySearch { .. }
            ) {
                continue;
            }
            let mut evidence_cursor = None;
            loop {
                let evidence_page = snapshot
                    .evidence(&intent.intent_id, evidence_cursor)
                    .map_err(|_| "source evidence unavailable")?;
                for item in evidence_page.records {
                    if item.evidence_id != evidence || !item.selected || item.conflict {
                        continue;
                    }
                    if let ReceiptOutput::Coding { observation } = &item.receipt.data().output
                        && let CodingObservation::Tool { output, .. } = observation.as_ref()
                        && output.tool == action.tool()
                        && output.status == ToolStatus::Succeeded
                        && output.process_tree_closed
                    {
                        found = true;
                    }
                }
                evidence_cursor = evidence_page.next;
                if evidence_cursor.is_none() {
                    break;
                }
            }
        }
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    if !found {
        return Err("source is not selected ordinary tool evidence".into());
    }
    let kind = match required(args, "--kind")?.as_str() {
        "stale" => SourceAnnotationKind::Stale,
        "conflicts-with" => SourceAnnotationKind::ConflictsWith {
            related_dispatch: Digest::new(required(args, "--related-dispatch")?)
                .map_err(|_| "invalid related dispatch")?,
        },
        _ => return Err("invalid annotation kind".into()),
    };
    let input = SourceAnnotationInput {
        dispatch: Digest::new(required(args, "--dispatch")?).map_err(|_| "invalid dispatch")?,
        target_dispatch: Digest::new(required(args, "--target-dispatch")?)
            .map_err(|_| "invalid target dispatch")?,
        kind,
        source_evidence: evidence,
        reason: required(args, "--reason")?,
    };
    let mut memory = MemoryStore::open_existing(
        settings
            .memory_db
            .as_ref()
            .ok_or("missing memory database")?,
    )
    .map_err(|_| "cannot open memory database")?;
    memory
        .annotate_source(&scope, &input)
        .map_err(|_| "memory annotation denied or invalid")?;
    Ok(serde_json::json!({"outcome":"succeeded","dispatch":input.dispatch}))
}

fn context_version_for_recipe(recipe: Option<&str>) -> u8 {
    match recipe {
        Some(
            metron_protocol::LOCAL_CODING_V8_RECIPE_SHA
            | metron_protocol::LOCAL_CODING_V9_RECIPE_SHA
            | metron_protocol::LOCAL_CODING_V10_RECIPE_SHA,
        ) => 11,
        Some(metron_protocol::LOCAL_CODING_V7_RECIPE_SHA) => 10,
        Some(
            metron_protocol::LOCAL_CODING_V4_RECIPE_SHA
            | metron_protocol::LOCAL_CODING_V5_RECIPE_SHA
            | metron_protocol::LOCAL_CODING_V6_RECIPE_SHA,
        )
        | None => 9,
        Some(metron_protocol::LOCAL_CODING_V3_RECIPE_SHA) => 6,
        Some(_) => 4,
    }
}

/// Operator commands never initialize settings, model adapters or effect drivers.
fn operator(args: &[String]) -> Result<serde_json::Value, String> {
    let action = args[0].as_str();
    let options: &[&str] = match action {
        "inspect" => &["--run-dir"],
        "approval" => &[
            "--run-dir",
            "--request-id",
            "--action-digest",
            "--decision",
            "--command-id",
        ],
        "advance-tick" => &["--run-dir", "--expected-tick", "--new-tick", "--command-id"],
        "reconcile" => &[
            "--run-dir",
            "--intent",
            "--evidence",
            "--expected-evidence-revision",
            "--expected-epoch",
            "--command-id",
        ],
        _ => return Err("unknown operator action".into()),
    };
    let mut seen = Vec::new();
    for pair in args[1..].chunks(2) {
        if pair.len() != 2
            || !options.contains(&pair[0].as_str())
            || seen.contains(&pair[0].as_str())
            || pair[1].starts_with("--")
        {
            return Err("invalid operator arguments".into());
        }
        seen.push(pair[0].as_str());
    }
    if seen.len() != options.len() {
        return Err("missing operator arguments".into());
    }
    let number = |name| -> Result<SqlU64, String> {
        SqlU64::new(
            required(args, name)?
                .parse::<u64>()
                .map_err(|_| "invalid logical value")?,
        )
        .map_err(|e| format!("{e:?}"))
    };
    let digest = |name| Digest::new(required(args, name)?).map_err(|e| format!("{e:?}"));
    let path = PathBuf::from(required(args, "--run-dir")?).join("runtime.sqlite");
    // Prove journal identity and integrity without writable access, even when
    // settings are missing or contain unusable worker/model paths.
    let mut readonly = SqliteStore::open_read_only(&path).map_err(|e| format!("{e:?}"))?;
    let ids = readonly.task_ids().map_err(|e| format!("{e:?}"))?;
    if ids.len() != 1 {
        return Err("expected one coding task".into());
    }
    let task_id = &ids[0];
    let mut snapshot = readonly.audit().map_err(|e| format!("{e:?}"))?;
    let projection = snapshot.projection(task_id).map_err(|e| format!("{e:?}"))?;
    if projection.task.coding().is_none() {
        return Err("legacy execution removed; historical journals are read-only".into());
    }
    let engine =
        crate::cognition(&projection.genesis, &projection.task).map_err(|(_, e)| e.to_string())?;
    let checks =
        metron_harness::audit::verify(&mut snapshot, task_id).map_err(|e| format!("{e:?}"))?;
    if !checks.chain_valid
        || !checks.accounting_valid
        || !checks.projection_valid
        || !metron_harness::replay::behavior(&mut snapshot, task_id, engine.as_ref())
            .map_err(|e| format!("{e:?}"))?
    {
        return Err("coding journal integrity failure".into());
    }
    if action == "inspect" {
        let mut intents = Vec::new();
        let mut cursor = None;
        loop {
            let page = snapshot
                .intents(task_id, cursor)
                .map_err(|e| format!("{e:?}"))?;
            for intent in page.records {
                let mut evidence = Vec::new();
                let mut evidence_cursor = None;
                loop {
                    let rows = snapshot
                        .evidence(&intent.intent_id, evidence_cursor)
                        .map_err(|e| format!("{e:?}"))?;
                    evidence.extend(rows.records.into_iter().map(|row| row.evidence_id));
                    evidence_cursor = rows.next;
                    if evidence_cursor.is_none() {
                        break;
                    }
                }
                intents.push(serde_json::json!({"intent_id":intent.intent_id,"status":intent.status,"approval":intent.approval,"evidence_revision":intent.evidence_revision,"reconciliation_epoch":intent.reconciliation_epoch,"reconciliation_lease":intent.reconciliation_lease,"evidence":evidence}));
            }
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        return Ok(
            serde_json::json!({"outcome":"succeeded","action":action,"task_id":task_id,"tick":projection.tick,"intents":intents}),
        );
    }
    let command_id =
        CommandId::new(required(args, "--command-id")?).map_err(|e| format!("{e:?}"))?;
    let operation = match action {
        "approval" => StoreOperation::RecordApproval {
            request_id: ApprovalId::new(required(args, "--request-id")?)
                .map_err(|e| format!("{e:?}"))?,
            action_digest: digest("--action-digest")?,
            decision: match required(args, "--decision")?.as_str() {
                "approve" => ApprovalDecision::Approve,
                "deny" => ApprovalDecision::Deny,
                _ => return Err("invalid decision".into()),
            },
        },
        "advance-tick" => StoreOperation::AdvanceTick {
            expected_tick: number("--expected-tick")?,
            new_tick: number("--new-tick")?,
        },
        "reconcile" => StoreOperation::ClaimReconciliation {
            task_id: TaskId::new("operator-placeholder").map_err(|e| format!("{e:?}"))?,
            intent_id: digest("--intent")?,
            worker: WorkerId::new("coding-operator").map_err(|e| format!("{e:?}"))?,
            expected_epoch: number("--expected-epoch")?,
            expected_evidence_revision: number("--expected-evidence-revision")?,
        },
        _ => unreachable!(),
    };
    let operation = if let StoreOperation::ClaimReconciliation {
        intent_id,
        worker,
        expected_epoch,
        expected_evidence_revision,
        ..
    } = operation
    {
        let intent = snapshot
            .intent(&intent_id)
            .map_err(|e| format!("{e:?}"))?
            .ok_or("missing intent")?;
        if &intent.intent.task_id != task_id {
            return Err("intent belongs to another task".into());
        }
        let selected = digest("--evidence")?;
        let mut cursor = None;
        let mut found = false;
        loop {
            let page = snapshot
                .evidence(&intent_id, cursor)
                .map_err(|e| format!("{e:?}"))?;
            found |= page.records.iter().any(|row| row.evidence_id == selected);
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        if !found {
            return Err("selected receipt is absent".into());
        }
        StoreOperation::ClaimReconciliation {
            task_id: task_id.clone(),
            intent_id,
            worker,
            expected_epoch,
            expected_evidence_revision,
        }
    } else {
        operation
    };
    drop(snapshot);
    drop(readonly);
    let mut store = SqliteStore::open_existing(&path).map_err(|e| format!("{e:?}"))?;
    let apply = |store: &mut SqliteStore, id, operation| {
        store
            .apply(
                StoreCommand::new(CommandData {
                    schema_version: projection.task.data().schema_version,
                    command_id: id,
                    operation,
                })
                .map_err(|e| format!("{e:?}"))?,
            )
            .map_err(|e| format!("{e:?}"))
    };
    let result = apply(&mut store, command_id.clone(), operation.clone())?;
    let result = if let StoreOperation::ClaimReconciliation {
        intent_id,
        worker,
        expected_evidence_revision,
        ..
    } = operation
    {
        match result {
            WriteOutcome::Committed(reply) => {
                let StoreResult::ReconciliationClaimed { lease_epoch, .. } = reply.result else {
                    return Err("unexpected reconciliation result".into());
                };
                // Separate deterministic identity makes interrupted retries idempotent.
                let settlement_id = content_digest(
                    format!(
                        "coding-reconciliation-settlement/v1:{}",
                        command_id.as_str()
                    )
                    .as_bytes(),
                );
                apply(
                    &mut store,
                    CommandId::new(settlement_id.as_str()).map_err(|e| format!("{e:?}"))?,
                    StoreOperation::SettleReconciliation {
                        task_id: task_id.clone(),
                        intent_id,
                        worker,
                        lease_epoch,
                        expected_evidence_revision,
                        evidence_id: digest("--evidence")?,
                        reconciled_receipt: None,
                    },
                )?
            }
            other => other,
        }
    } else {
        result
    };
    Ok(
        serde_json::json!({"outcome": if matches!(result, WriteOutcome::Committed(_)) { "succeeded" } else { "blocked" }, "action": action, "result": result}),
    )
}

fn run(args: &[String]) -> Result<serde_json::Value, String> {
    let resource_budget_path = resource_budget_flag(args)?;
    let action = args.first().map(String::as_str).unwrap_or("");
    if matches!(
        action,
        "approval" | "advance-tick" | "reconcile" | "inspect"
    ) {
        return operator(args);
    }
    if action == "memory-annotate" {
        return annotate_memory(args);
    }
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
    if !fresh && action == "resume" {
        // Inspect only immutable journal state before settings validation can
        // initialize a worker, tool adapter or writable store.
        let mut old = SqliteStore::open_read_only(run_dir.join("runtime.sqlite"))
            .map_err(|e| format!("{e:?}"))?;
        let ids = old.task_ids().map_err(|e| format!("{e:?}"))?;
        if ids.len() != 1
            || old
                .audit()
                .map_err(|e| format!("{e:?}"))?
                .projection(&ids[0])
                .map_err(|e| format!("{e:?}"))?
                .task
                .coding()
                .is_none()
        {
            return Err("legacy execution removed; historical journals are read-only".into());
        }
    }
    let mut settings: Settings = if fresh {
        if !args.iter().any(|a| a == "--experimental") {
            return Err("unqualified coding attempts require --experimental".into());
        }
        let profile: Option<LocalProfile> = flag(args, "--profile")
            .map(|p| read(Path::new(&p)))
            .transpose()?;
        let context_version =
            context_version_for_recipe(profile.as_ref().map(|p| p.prompt_recipe_digest.as_str()));
        Settings {
            resource_budget_required: resource_budget_path.is_some(),
            context_version,
            task: read(Path::new(&required(args, "--task")?))?,
            policy: read(Path::new(&required(args, "--policy")?))?,
            catalog: read(Path::new(&required(args, "--catalog")?))?,
            acceptance: read(Path::new(&required(args, "--acceptance")?))?,
            profile,
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
    if action != "audit" && settings.resource_budget_required && resource_budget_path.is_none() {
        return Err("this run requires --resource-budget FILE on every execution/resume".into());
    }
    let resource_policy = if action != "audit" {
        resource_budget_path
            .as_ref()
            .map(|path| {
                let resolved = external_resource_budget_path(
                    Path::new(path),
                    Path::new(settings.policy.workspace_root.as_str()),
                )?;
                let profile = settings
                    .profile
                    .as_ref()
                    .ok_or("resource budget requires native profile")?;
                let policy = crate::resource_budget::policy(
                    &resolved,
                    settings.task.data().task_id.clone(),
                    settings.task.data().principal_id.clone(),
                )?;
                policy
                    .validate(profile)
                    .map_err(|e| format!("resource budget: {e:?}"))?;
                Ok::<_, String>(policy)
            })
            .transpose()?
    } else {
        None
    };
    let require_budget_update =
        !fresh && !settings.resource_budget_required && resource_budget_path.is_some();
    settings.resource_budget_required |= resource_budget_path.is_some();
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
    if require_budget_update && action != "audit" {
        replace_budget_settings(
            &run_dir,
            &canonical_bytes(&settings, MAX_INTERNAL).map_err(|e| format!("settings: {e:?}"))?,
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
        let native = native.map(|driver| match resource_policy {
            Some(policy) => driver.with_resource_budget(policy),
            None => driver,
        });
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
            context_version: settings.context_version,
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
    pub(super) fn setup_at(
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
            resource_budget_required: false,
            context_version: 8,
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
            context_version: s.context_version,
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
    fn recipe_routes_preserve_legacy_context_versions() {
        assert_eq!(
            context_version_for_recipe(Some(LOCAL_CODING_V10_RECIPE_SHA)),
            11
        );
        assert_eq!(
            context_version_for_recipe(Some(LOCAL_CODING_V9_RECIPE_SHA)),
            11
        );
        assert_eq!(
            context_version_for_recipe(Some(LOCAL_CODING_V8_RECIPE_SHA)),
            11
        );
        assert_eq!(
            context_version_for_recipe(Some(LOCAL_CODING_V7_RECIPE_SHA)),
            10
        );
        for recipe in [
            LOCAL_CODING_V4_RECIPE_SHA,
            LOCAL_CODING_V5_RECIPE_SHA,
            LOCAL_CODING_V6_RECIPE_SHA,
        ] {
            assert_eq!(context_version_for_recipe(Some(recipe)), 9);
        }
        assert_eq!(context_version_for_recipe(None), 9);
        assert_eq!(
            context_version_for_recipe(Some(LOCAL_CODING_V3_RECIPE_SHA)),
            6
        );
        assert_eq!(
            context_version_for_recipe(Some(LOCAL_CODING_V2_RECIPE_SHA)),
            4
        );
        assert_eq!(context_version_for_recipe(Some(LOCAL_CODING_RECIPE_SHA)), 4);
    }

    #[test]
    fn native_v9_root_aliases_are_admitted_and_audited_in_both_modes() {
        fn profile() -> LocalProfile {
            let artifact =
                Digest::new("3793e1e6e2266919d6e015287221b41100175a98eddaba92e111aed3b05c93c4")
                    .unwrap();
            LocalProfile {
                schema_version: SchemaVersion::V2,
                auxiliary_repository: BoundedText::new("LiquidAI/LFM3-1.7B").unwrap(),
                auxiliary_revision: BoundedText::new("1da06bec6a819da10b54c67e5c90972b46d81871")
                    .unwrap(),
                artifact_repository: BoundedText::new("LiquidAI/LFM3-1.7B-GGUF").unwrap(),
                artifact_revision: BoundedText::new("cd0c74c575b5c416f3011b5b4c298077f26ca8c2")
                    .unwrap(),
                artifact_name: BoundedText::new("LFM3-1.7B-Q4_K_M.gguf").unwrap(),
                embedded_tokenizer_digest: gguf_lfm3_corrected_tokenizer_digest(&artifact).unwrap(),
                artifact_digest: artifact,
                artifact_bytes: SqlU64::new(1099627808).unwrap(),
                template_digest: content_digest(b"template"),
                prompt_recipe_digest: Digest::new(LOCAL_CODING_V9_RECIPE_SHA).unwrap(),
                backend_revision: BoundedText::new("fb4cdc3730dc64b366724e7637a205a8b7747499")
                    .unwrap(),
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
        struct RecordedCalls {
            inner: Driver,
            profile: LocalProfile,
            calls: VecDeque<(&'static str, &'static str)>,
        }
        impl EffectDriver for RecordedCalls {
            type Outcome = DriverOutcome;
            fn execute(&mut self, effect: AuthorizedEffect) -> DriverOutcome {
                if let ApprovedRequest::Coding { step, .. } = &effect.request().request
                    && let CodingStep::Model { input_digest, .. } = step.as_ref()
                {
                    let (name, args) = self.calls.pop_front().expect("recorded call");
                    let native = NativeCallEvidence::capture(
                        1,
                        vec![(name.to_owned(), args.to_owned())],
                        None,
                    )
                    .unwrap();
                    let content = native
                        .action_content_for_recipe(&self.profile.prompt_recipe_digest)
                        .unwrap();
                    let observed = LocalObservation {
                        schema_version: SchemaVersion::V2,
                        profile_digest: self.profile.digest().unwrap(),
                        request_digest: effect.request().request_digest.clone(),
                        input_digest: input_digest.clone(),
                        content: BoundedText::new(content).unwrap(),
                        reasoning: None,
                        native_calls: Some(native),
                        prompt_tokens: 1,
                        completion_tokens: 1,
                        submission_cohort: SubmissionCohort {
                            id: BoundedText::new(effect.request().action.dispatch_id.as_str())
                                .unwrap(),
                            size: 1,
                            position: 0,
                        },
                        backend_batch: BatchProvenance::Unknown,
                        response: Some(LocalResponse::Text),
                    };
                    return self.inner.known(
                        &effect,
                        ReceiptOutput::LocalModel {
                            observation: Box::new(observed),
                        },
                        1,
                        0,
                    );
                }
                self.inner.execute(effect)
            }
        }
        for mode in [EngineMode::Agent, EngineMode::Harness] {
            let name = if mode == EngineMode::Agent {
                "root-v9-agent"
            } else {
                "root-v9-harness"
            };
            let (root, mut settings, original) = setup(
                name,
                mode,
                vec![CodingTool::List, CodingTool::Run, CodingTool::Finish],
                false,
            );
            drop(original);
            let profile = profile();
            profile.validate().unwrap();
            settings.context_version = 11;
            settings.policy.allowed_programs = vec![RustProgram::Cargo];
            settings.profile = Some(profile.clone());
            settings.script = None;
            let mut data = settings.task.data().clone();
            data.model_profile_digest = profile.digest().unwrap();
            settings.task = CodingTask::new(data).unwrap();
            fs::write(root.join("workspace/note.txt"), "fixture\n").unwrap();
            let mut store = SqliteStore::create(root.join("run/root-v9.sqlite")).unwrap();
            store
                .apply(
                    command(StoreOperation::CreateTask {
                        task: Task::new_coding(settings.task.clone()).unwrap(),
                        genesis: settings.genesis().unwrap(),
                    })
                    .unwrap(),
                )
                .unwrap();
            let mut driver = RecordedCalls {
                inner: driver(&root, &settings, &[]),
                profile,
                calls: VecDeque::from([
                    ("plan", r#"{"steps":["inspect"]}"#),
                    ("list", r#"{"directory":"."}"#),
                    (
                        "run",
                        r#"{"program":"cargo","args":["--version"],"directory":"."}"#,
                    ),
                    ("finish", r#"{"summary":"request verification"}"#),
                ]),
            };
            driver.inner.store_path = root.join("run/root-v9.sqlite");
            let genesis = settings.genesis().unwrap();
            let task = Task::new_coding(settings.task.clone()).unwrap();
            let engine = crate::cognition(&genesis, &task).unwrap();
            assert_eq!(
                orchestrator::drive_with_driver(
                    &mut store,
                    &settings.task.data().task_id,
                    engine.as_ref(),
                    &mut NoObserver,
                    &mut driver
                )
                .unwrap(),
                orchestrator::DriveStatus::Blocked
            );
            let mut snapshot = store.audit().unwrap();
            let projection = snapshot.projection(&settings.task.data().task_id).unwrap();
            assert_eq!(projection.state.phase, Phase::AwaitingVerification);
            let mut list = 0;
            let mut run = 0;
            let mut raw_dot = 0;
            let mut tool_receipts = Vec::new();
            for intent in snapshot
                .intents(&settings.task.data().task_id, None)
                .unwrap()
                .records
            {
                if let IntentKind::Coding { step } = &intent.intent.kind {
                    match step.as_ref() {
                        CodingStep::Tool {
                            action: CodingAction::List { directory },
                            ..
                        } => {
                            assert_eq!(directory, &None);
                            list += 1;
                        }
                        CodingStep::Tool {
                            action: CodingAction::Run { directory, .. },
                            ..
                        } => {
                            assert_eq!(directory, &None);
                            run += 1;
                        }
                        _ => {}
                    }
                }
                for evidence in snapshot.evidence(&intent.intent_id, None).unwrap().records {
                    if let ReceiptOutput::LocalModel { observation } =
                        &evidence.receipt.data().output
                    {
                        let native = observation.native_calls.as_ref().unwrap();
                        if native.calls[0]
                            .arguments
                            .as_str()
                            .contains(r#""directory":".""#)
                        {
                            raw_dot += 1;
                        }
                    }
                    if let ReceiptOutput::Coding { observation } = &evidence.receipt.data().output
                        && let CodingObservation::Tool { output, .. } = observation.as_ref()
                    {
                        tool_receipts.push((output.tool, output.status.clone()));
                    }
                }
            }
            assert_eq!((list, run, raw_dot), (1, 1, 2));
            assert!(tool_receipts.contains(&(CodingTool::List, ToolStatus::Succeeded)));
            // The portable fixture deliberately has no executable image; this is a real
            // denied receipt, not a successful process execution or an SDK inference.
            assert!(tool_receipts.contains(&(CodingTool::Run, ToolStatus::Denied)));
            drop(snapshot);
            assert_audits(&mut store, &settings);
            drop(driver);
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn detailed_feedback_invalid_then_valid_read_audits_in_both_modes() {
        for mode in [EngineMode::Agent, EngineMode::Harness] {
            let name = if mode == EngineMode::Agent {
                "detail-agent"
            } else {
                "detail-harness"
            };
            let (root, mut settings, old_store) = setup(
                name,
                mode,
                vec![CodingTool::Read, CodingTool::Finish],
                false,
            );
            drop(old_store);
            settings.context_version = 11;
            fs::write(root.join("workspace/note.txt"), "controlled read fixture\n").unwrap();
            let mut store = SqliteStore::create(root.join("run/detail.sqlite")).unwrap();
            store
                .apply(
                    command(StoreOperation::CreateTask {
                        task: Task::new_coding(settings.task.clone()).unwrap(),
                        genesis: settings.genesis().unwrap(),
                    })
                    .unwrap(),
                )
                .unwrap();
            let bad = r#"{"tool":"read","path":null,"first_line":1,"line_count":2}"#;
            let mut worker = driver(
                &root,
                &settings,
                &[
                    r#"{"tool":"plan","steps":["read the note"]}"#,
                    bad,
                    r#"{"tool":"read","path":"note.txt","first_line":1,"line_count":2}"#,
                    r#"{"tool":"finish","summary":"request independent verification"}"#,
                ],
            );
            worker.store_path = root.join("run/detail.sqlite");
            let task = Task::new_coding(settings.task.clone()).unwrap();
            let genesis = settings.genesis().unwrap();
            let engine = crate::cognition(&genesis, &task).unwrap();
            assert_eq!(
                orchestrator::drive_with_driver(
                    &mut store,
                    &settings.task.data().task_id,
                    engine.as_ref(),
                    &mut NoObserver,
                    &mut worker
                )
                .unwrap(),
                orchestrator::DriveStatus::Blocked
            );
            let mut snapshot = store.audit().unwrap();
            let projection = snapshot.projection(&settings.task.data().task_id).unwrap();
            assert_eq!(projection.state.phase, Phase::AwaitingVerification);
            assert_eq!(projection.task_account.posted.model_requests.get(), 4);
            assert_eq!(projection.task_account.posted.tool_calls.get(), 2);
            let mut reads = 0;
            let mut feedback = false;
            for intent in snapshot
                .intents(&settings.task.data().task_id, None)
                .unwrap()
                .records
            {
                if let IntentKind::Coding { step } = &intent.intent.kind {
                    match step.as_ref() {
                        CodingStep::Model { context, .. } => {
                            let value: serde_json::Value =
                                serde_json::from_str(context.as_str()).unwrap();
                            if let Some(detail) =
                                value.get("model_feedback").and_then(|f| f.get("detail"))
                            {
                                assert_eq!(detail["rejected_preview"], bad);
                                assert_eq!(detail["truncated"], false);
                                let reason = detail["reason"].as_str().unwrap();
                                assert!(reason.contains("null") && reason.contains("string"));
                                feedback = true;
                            }
                        }
                        CodingStep::Tool {
                            action: CodingAction::Read { path, .. },
                            ..
                        } => {
                            assert_eq!(path.as_str(), "note.txt");
                            reads += 1;
                        }
                        _ => {}
                    }
                }
            }
            assert!(feedback);
            assert_eq!(reads, 1);
            drop(snapshot);
            assert_audits(&mut store, &settings);
            drop(worker);
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn paired_both_modes_replay_controlled_compiler_feedback_and_correction() {
        struct ControlledCompile {
            inner: Driver,
            saw_feedback: bool,
        }
        impl EffectDriver for ControlledCompile {
            type Outcome = DriverOutcome;
            fn execute(&mut self, effect: AuthorizedEffect) -> DriverOutcome {
                if let ApprovedRequest::Coding { step, .. } = &effect.request().request {
                    if let CodingStep::Model {
                        previous_tool_turn: Some(turn),
                        ..
                    } = step.as_ref()
                        && let metron_protocol::coding_runtime::CompletedToolTurn::Complete {
                            action: CodingAction::Run { .. },
                            observation,
                            ..
                        } = turn.as_ref()
                    {
                        self.saw_feedback = observation
                            .body
                            .as_str()
                            .contains("controlled compiler error");
                    }
                    if let CodingStep::Tool {
                        action: CodingAction::Run { .. },
                        ..
                    } = step.as_ref()
                    {
                        return self.inner.known(
                            &effect,
                            ReceiptOutput::Coding {
                                observation: Box::new(CodingObservation::Tool {
                                    output: CodingToolOutput {
                                        tool: CodingTool::Run,
                                        status: ToolStatus::Failed,
                                        body: BoundedText::new(
                                            "controlled compiler error: expected delimiter",
                                        )
                                        .unwrap(),
                                        truncated: false,
                                        exit_code: Some(1),
                                        files: vec![],
                                        process_tree_closed: true,
                                    },
                                    memory_selection: None,
                                }),
                            },
                            0,
                            1,
                        );
                    }
                }
                self.inner.execute(effect)
            }
        }
        for mode in [EngineMode::Agent, EngineMode::Harness] {
            let (root, mut settings, old_store) = setup(
                if mode == EngineMode::Agent {
                    "paired-agent"
                } else {
                    "paired-harness"
                },
                mode,
                vec![CodingTool::Edit, CodingTool::Run, CodingTool::Finish],
                false,
            );
            drop(old_store);
            settings.context_version = 10;
            settings.policy.allowed_programs = vec![metron_protocol::coding::RustProgram::Cargo];
            let mut store = SqliteStore::create(root.join("run/paired.sqlite")).unwrap();
            store
                .apply(
                    command(StoreOperation::CreateTask {
                        task: Task::new_coding(settings.task.clone()).unwrap(),
                        genesis: settings.genesis().unwrap(),
                    })
                    .unwrap(),
                )
                .unwrap();
            let digest = content_digest(b"fn main( {");
            let correction = format!(
                r#"{{"tool":"edit","path":"test.rs","expected_digest":"{}","content":"fn main() {{}}"}}"#,
                digest.as_str()
            );
            let mut worker = ControlledCompile {
                inner: driver(
                    &root,
                    &settings,
                    &[
                        r#"{"tool":"plan","steps":["write and check"]}"#,
                        r#"{"tool":"edit","path":"test.rs","expected_digest":null,"content":"fn main( {"}"#,
                        r#"{"tool":"run","program":"cargo","args":["test","--offline"],"directory":null}"#,
                        &correction,
                        r#"{"tool":"finish","summary":"request verification"}"#,
                    ],
                ),
                saw_feedback: false,
            };
            let task = Task::new_coding(settings.task.clone()).unwrap();
            let engine = crate::cognition(&settings.genesis().unwrap(), &task).unwrap();
            assert_eq!(
                orchestrator::drive_with_driver(
                    &mut store,
                    &settings.task.data().task_id,
                    engine.as_ref(),
                    &mut NoObserver,
                    &mut worker
                )
                .unwrap(),
                orchestrator::DriveStatus::Blocked
            );
            assert!(worker.saw_feedback);
            let mut snapshot = store.audit().unwrap();
            let projection = snapshot.projection(&settings.task.data().task_id).unwrap();
            assert_eq!(projection.state.phase, Phase::AwaitingVerification);
            assert_eq!(projection.task_account.posted.model_requests.get(), 5);
            assert_eq!(projection.task_account.posted.tool_calls.get(), 4);
            drop(snapshot);
            assert_audits(&mut store, &settings);
            drop(store);
            drop(worker);
            fs::remove_dir_all(root).unwrap();
        }
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
                        && let CodingObservation::Tool {
                            output,
                            memory_selection: None,
                        } = observation.as_ref()
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
    fn budget_requirement_survives_resume_and_missing_flag_never_mutates_journal() {
        let (root, mut settings, store) = setup(
            "budget-required",
            EngineMode::Harness,
            vec![CodingTool::Finish],
            false,
        );
        settings.resource_budget_required = true;
        write_new(
            &root.join("run/coding-settings.json"),
            &canonical_bytes(&settings, MAX_INTERNAL).unwrap(),
        )
        .unwrap();
        drop(store);
        let db = root.join("run/runtime.sqlite");
        let before = fs::read(&db).unwrap();
        let args = vec![
            "resume".into(),
            "--run-dir".into(),
            root.join("run").to_str().unwrap().into(),
        ];
        assert!(
            run(&args)
                .unwrap_err()
                .contains("requires --resource-budget")
        );
        assert_eq!(fs::read(&db).unwrap(), before);
        assert!(!root.join("run/worker-events-revision-0.jsonl").exists());
        let mut malformed = args;
        malformed.push("--resource-budget".into());
        assert!(run(&malformed).unwrap_err().contains("requires FILE"));
        assert_eq!(fs::read(&db).unwrap(), before);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn budget_settings_retry_ignores_untrusted_crash_remnants_and_cleans_owned_temps() {
        let root = std::env::temp_dir().join(format!(
            "metron-budget-settings-retry-{}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        let settings = root.join("coding-settings.json");
        let stale = root.join("coding-settings-budget-pending.json");
        let replacement = br#"{"resource_budget_required":true}"#;
        let crashed_unique = root.join(format!(
            ".coding-settings-budget-{}-0.tmp",
            std::process::id()
        ));
        fs::write(&crashed_unique, b"partial crashed unique attempt").unwrap();
        for remnant in [
            replacement.as_slice(),
            br#"{"resource_budget_required":"#.as_slice(),
            br#"{"unrelated_config":"must never replace settings"}"#.as_slice(),
        ] {
            fs::write(&settings, br#"{"resource_budget_required":false}"#).unwrap();
            fs::write(&stale, remnant).unwrap();
            replace_budget_settings(&root, replacement).unwrap();
            assert_eq!(fs::read(&settings).unwrap(), replacement);
            assert_eq!(fs::read(&stale).unwrap(), remnant);
            assert_eq!(fs::read_dir(&root).unwrap().count(), 3);
            assert_eq!(
                fs::read(&crashed_unique).unwrap(),
                b"partial crashed unique attempt"
            );
        }
        // A stale symlink is not followed, overwritten or removed by a retry.
        fs::remove_file(&stale).unwrap();
        let sentinel = root.join("operator-config.json");
        fs::write(&sentinel, b"untouched operator configuration").unwrap();
        std::os::unix::fs::symlink(&sentinel, &stale).unwrap();
        replace_budget_settings(&root, replacement).unwrap();
        assert_eq!(
            fs::read(&sentinel).unwrap(),
            b"untouched operator configuration"
        );
        assert!(
            fs::symlink_metadata(&stale)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_file(&crashed_unique).unwrap();
        // A rename failure cleans only this attempt's temporary file and permits retry.
        fs::remove_file(&settings).unwrap();
        fs::create_dir(&settings).unwrap();
        fs::write(settings.join("preserved"), b"original state").unwrap();
        assert!(
            replace_budget_settings(&root, replacement)
                .unwrap_err()
                .contains("replace")
        );
        assert_eq!(
            fs::read(settings.join("preserved")).unwrap(),
            b"original state"
        );
        assert!(!fs::read_dir(&root).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".coding-settings-budget-")
        }));
        fs::remove_file(settings.join("preserved")).unwrap();
        fs::remove_dir(&settings).unwrap();
        replace_budget_settings(&root, replacement).unwrap();
        assert_eq!(fs::read(&settings).unwrap(), replacement);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn external_budget_resolution_reads_the_external_target_not_the_mutable_alias() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!(
            "metron-external-budget-resolution-{}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        let workspace = root.join("workspace");
        fs::create_dir(&workspace).unwrap();
        let external = root.join("operator-policy.json");
        fs::write(&external, b"operator bounds").unwrap();
        let alias = workspace.join("alias.json");
        symlink(&external, &alias).unwrap();
        let resolved = external_resource_budget_path(&alias, &workspace).unwrap();
        assert_eq!(resolved, fs::canonicalize(&external).unwrap());
        fs::remove_file(&alias).unwrap();
        fs::write(&alias, b"replacement model content").unwrap();
        assert_eq!(fs::read(&resolved).unwrap(), b"operator bounds");
        fs::write(&external, b"updated operator bounds").unwrap();
        assert_eq!(
            fs::read(external_resource_budget_path(&external, &workspace).unwrap()).unwrap(),
            b"updated operator bounds"
        );
        assert!(external_resource_budget_path(&external, &root.join("missing-workspace")).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn invalid_budget_opt_in_does_not_rewrite_existing_run_settings() {
        let (root, settings, store) = setup(
            "invalid-budget-opt-in",
            EngineMode::Harness,
            vec![CodingTool::Finish],
            false,
        );
        let path = root.join("run/coding-settings.json");
        write_new(&path, &canonical_bytes(&settings, MAX_INTERNAL).unwrap()).unwrap();
        drop(store);
        let before_settings = fs::read(&path).unwrap();
        let before_journal = fs::read(root.join("run/runtime.sqlite")).unwrap();
        let result = run(&[
            "resume".into(),
            "--run-dir".into(),
            root.join("run").to_str().unwrap().into(),
            "--resource-budget".into(),
            root.join("missing.json").to_str().unwrap().into(),
        ]);
        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), before_settings);
        assert_eq!(
            fs::read(root.join("run/runtime.sqlite")).unwrap(),
            before_journal
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn resource_budget_flag_rejects_duplicates_and_flag_values() {
        assert!(resource_budget_flag(&["--resource-budget".into(), "--worker".into()]).is_err());
        assert!(
            resource_budget_flag(&[
                "--resource-budget".into(),
                "a".into(),
                "--resource-budget".into(),
                "b".into()
            ])
            .is_err()
        );
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

#[cfg(test)]
#[path = "coding_run/tool_security_tests.rs"]
mod tool_security_tests;

#[cfg(test)]
mod native_file_tests;

#[cfg(test)]
mod crash_tests;
