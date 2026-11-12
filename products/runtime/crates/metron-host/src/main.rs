//! Composition root for the scripted alpha.0 runtime.
#![forbid(unsafe_code)]
mod coding_acceptance;
mod coding_run;
mod coding_tools;
mod comparison_runner;
mod hardware;
mod installer;
mod local_run;
mod measurement;
mod model_cli;
mod qualification;
mod qualification_capture;
mod qualification_runner;
mod session_identity;
mod toolkit;
use metron_harness::{
    orchestrator::{self, DriveError, DriveStatus, NoObserver},
    planner::{LocalSumPlanner, SumPlanner},
};
use metron_protocol::{
    cli::{CliAccounting, CliApproval, CliAudit, CliCommand, CliHealth, CliOutcome, CliReport},
    domain::{Cognition, Phase, Task},
    store::{
        ApprovalDecision, AuditSnapshot, CommandData, EngineMode, Genesis, RuntimeSnapshot,
        StoreCommand, StoreError, StoreOperation, StorePort, WriteOutcome,
    },
    types::{
        ApprovalId, BoundedText, CommandId, Digest, MAX_INTERNAL, SchemaVersion, SqlU64,
        canonical_bytes, content_digest, parse_envelope, structural_digest,
    },
};
use metron_store_sqlite::SqliteStore;
use std::{env, fs, io::Read, path::PathBuf, process::ExitCode};

fn cognition(
    genesis: &Genesis,
    task: &Task,
) -> Result<Box<dyn Cognition>, (CliOutcome, &'static str)> {
    if let Some(task) = task.coding() {
        let policy = genesis
            .coding_policy
            .as_deref()
            .ok_or((CliOutcome::IntegrityFailure, "missing coding policy"))?;
        return match (genesis.mode, genesis.engine_version.as_str()) {
            (EngineMode::Agent, "coding-agent/v3") => {
                metron_agent::CodingAgent::legacy_v3(task, policy)
                    .map(|c| Box::new(c) as Box<dyn Cognition>)
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid coding policy"))
            }
            (EngineMode::Harness, "coding-harness/v3") => {
                metron_harness::planner::CodingPlanner::legacy_v3(task, policy)
                    .map(|c| Box::new(c) as Box<dyn Cognition>)
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid coding policy"))
            }
            (EngineMode::Agent, "coding-agent/v3.1") => {
                metron_agent::CodingAgent::legacy_v3_1(task, policy)
                    .map(|c| Box::new(c) as Box<dyn Cognition>)
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid coding policy"))
            }
            (EngineMode::Harness, "coding-harness/v3.1") => {
                metron_harness::planner::CodingPlanner::legacy_v3_1(task, policy)
                    .map(|c| Box::new(c) as Box<dyn Cognition>)
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid coding policy"))
            }
            (EngineMode::Agent, "coding-agent/v3.2") => {
                metron_agent::CodingAgent::legacy_v3_2(task, policy)
                    .map(|c| Box::new(c) as Box<dyn Cognition>)
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid coding policy"))
            }
            (EngineMode::Harness, "coding-harness/v3.2") => {
                metron_harness::planner::CodingPlanner::legacy_v3_2(task, policy)
                    .map(|c| Box::new(c) as Box<dyn Cognition>)
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid coding policy"))
            }
            (EngineMode::Agent, "coding-agent/v3.3") => {
                metron_agent::CodingAgent::legacy_v3_3(task, policy)
                    .map(|c| Box::new(c) as Box<dyn Cognition>)
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid coding policy"))
            }
            (EngineMode::Agent, "coding-agent/v3.4") => {
                metron_agent::CodingAgent::legacy_v3_4(task, policy)
                    .map(|c| Box::new(c) as Box<dyn Cognition>)
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid coding policy"))
            }
            (EngineMode::Agent, "coding-agent/v3.5") => {
                metron_agent::CodingAgent::legacy_v3_5(task, policy)
                    .map(|c| Box::new(c) as Box<dyn Cognition>)
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid coding policy"))
            }
            (EngineMode::Agent, "coding-agent/v3.6") => {
                metron_agent::CodingAgent::new(task, policy)
                    .map(|c| Box::new(c) as Box<dyn Cognition>)
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid coding policy"))
            }
            (EngineMode::Harness, "coding-harness/v3.3") => {
                metron_harness::planner::CodingPlanner::legacy_v3_3(task, policy)
                    .map(|c| Box::new(c) as Box<dyn Cognition>)
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid coding policy"))
            }
            (EngineMode::Harness, "coding-harness/v3.4") => {
                metron_harness::planner::CodingPlanner::legacy_v3_4(task, policy)
                    .map(|c| Box::new(c) as Box<dyn Cognition>)
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid coding policy"))
            }
            (EngineMode::Harness, "coding-harness/v3.5") => {
                metron_harness::planner::CodingPlanner::legacy_v3_5(task, policy)
                    .map(|c| Box::new(c) as Box<dyn Cognition>)
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid coding policy"))
            }
            (EngineMode::Harness, "coding-harness/v3.6") => {
                metron_harness::planner::CodingPlanner::new(task, policy)
                    .map(|c| Box::new(c) as Box<dyn Cognition>)
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid coding policy"))
            }
            _ => Err((CliOutcome::IntegrityFailure, "unknown coding engine")),
        };
    }
    match (
        genesis.mode,
        genesis.engine_version.as_str(),
        &genesis.local_profile,
    ) {
        (EngineMode::Agent, "sum-agent/v1", None) => Ok(Box::new(metron_agent::SumAgent)),
        (EngineMode::Harness, "sum-harness/v1", None) => Ok(Box::new(SumPlanner)),
        (EngineMode::Agent, "sum-local-agent/v2", Some(profile)) => Ok(Box::new(
            metron_agent::LocalSumAgent::new(profile)
                .map_err(|_| (CliOutcome::IntegrityFailure, "invalid local profile"))?,
        )),
        (EngineMode::Harness, "sum-local-harness/v2", Some(profile)) => {
            Ok(Box::new(LocalSumPlanner::new(profile).map_err(|_| {
                (CliOutcome::IntegrityFailure, "invalid local profile")
            })?))
        }
        _ => Err((CliOutcome::IntegrityFailure, "unknown engine version")),
    }
}

fn report(command: CliCommand, outcome: CliOutcome, diagnostic: Option<&str>) -> CliReport {
    CliReport {
        schema_version: SchemaVersion::V1,
        command,
        outcome,
        task_id: None,
        phase: None,
        pending_intent: None,
        health: None,
        replay_head: None,
        approval: None,
        accounting: None,
        audit: None,
        diagnostic: diagnostic.and_then(|s| BoundedText::new(s).ok()),
    }
}
fn emit(response: CliReport) -> ExitCode {
    let code = match response.outcome {
        CliOutcome::Succeeded => 0,
        CliOutcome::Rejected => 1,
        CliOutcome::InvalidInput => 2,
        CliOutcome::Denied => 3,
        CliOutcome::Blocked => 4,
        CliOutcome::IntegrityFailure => 5,
        CliOutcome::OperationalFailure => 6,
    };
    match response.encode() {
        Ok(bytes) => {
            println!("{}", String::from_utf8_lossy(&bytes));
            if code != 0
                && let Some(diagnostic) = &response.diagnostic
            {
                eprintln!("{}", diagnostic.as_str());
            }
            ExitCode::from(code)
        }
        Err(_) => {
            let fallback = report(
                response.command,
                CliOutcome::OperationalFailure,
                Some("report encoding failed"),
            );
            let bytes = fallback.encode().expect("bounded fallback report");
            println!("{}", String::from_utf8_lossy(&bytes));
            eprintln!("report encoding failed");
            ExitCode::from(6)
        }
    }
}
fn make_command(
    operation: StoreOperation,
    supplied: Option<CommandId>,
    schema_version: SchemaVersion,
) -> Result<StoreCommand, ()> {
    let id = match supplied {
        Some(id) => id,
        None => {
            let bytes = canonical_bytes(&operation, MAX_INTERNAL).map_err(|_| ())?;
            let digest = structural_digest("command/v1", &[&bytes]).map_err(|_| ())?;
            CommandId::new(digest.as_str()).map_err(|_| ())?
        }
    };
    StoreCommand::new(CommandData {
        schema_version,
        command_id: id,
        operation,
    })
    .map_err(|_| ())
}
fn audit_error(error: StoreError) -> (CliOutcome, &'static str) {
    match error {
        StoreError::Integrity | StoreError::Validation(_) => {
            (CliOutcome::IntegrityFailure, "invalid audit data")
        }
        StoreError::Operational(text) if text.as_str() == "audit resource limit exceeded" => (
            CliOutcome::OperationalFailure,
            "audit resource limit exceeded",
        ),
        StoreError::Busy | StoreError::Operational(_) => {
            (CliOutcome::OperationalFailure, "store unavailable")
        }
    }
}
fn drive_error(error: DriveError) -> (CliOutcome, &'static str) {
    match error {
        DriveError::Store(error) => audit_error(error),
        DriveError::Integrity | DriveError::Invalid | DriveError::Decision(_) => (
            CliOutcome::IntegrityFailure,
            "orchestration integrity failure",
        ),
        DriveError::Operational => (CliOutcome::OperationalFailure, "orchestration unavailable"),
    }
}
fn reported_usage(
    snapshot: &mut impl RuntimeSnapshot,
    intent: &metron_protocol::IntentSnapshot,
) -> Result<Option<metron_protocol::ActualUsage>, StoreError> {
    let mut cursor = None;
    let mut sole = None;
    let mut count = 0usize;
    loop {
        let page = snapshot.evidence(&intent.intent_id, cursor)?;
        for row in page.records {
            if intent.selected_evidence.as_ref() == Some(&row.evidence_id) {
                return Ok(Some(row.receipt.data().usage.clone()));
            }
            count = count.saturating_add(1);
            if count == 1 {
                sole = Some(row.receipt.data().usage.clone());
            }
            if count > 1 && intent.selected_evidence.is_none() {
                return Ok(None);
            }
        }
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    if intent.selected_evidence.is_some() {
        Err(StoreError::Integrity)
    } else {
        Ok(sole)
    }
}
fn flag(args: &[String], name: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
}
fn validate_args(command: CliCommand, args: &[String]) -> bool {
    let options: &[&str] = match command {
        CliCommand::Run => &[
            "--task",
            "--task-family",
            "--run-dir",
            "--mode",
            "--require-publication-approval",
            "--model-profile",
            "--worker",
            "--model-cache",
            "--heldout",
        ],
        CliCommand::Resume => &[
            "--advance-tick",
            "--expected-tick",
            "--command-id",
            "--model-profile",
            "--worker",
            "--model-cache",
            "--heldout",
        ],
        CliCommand::Reconcile => &[
            "--intent",
            "--advance-tick",
            "--expected-tick",
            "--command-id",
        ],
        CliCommand::Approval => &[
            "--request-id",
            "--action-digest",
            "--decision",
            "--command-id",
        ],
        CliCommand::Replay | CliCommand::Verify => &[],
    };
    let mut i = if command == CliCommand::Run {
        0
    } else if args.first().is_some_and(|v| !v.starts_with('-')) {
        1
    } else {
        return false;
    };
    let mut seen = Vec::new();
    while i < args.len() {
        let option = args[i].as_str();
        if !options.contains(&option) || seen.contains(&option) {
            return false;
        }
        seen.push(option);
        i += 1;
        if option != "--require-publication-approval" {
            if i >= args.len() || args[i].starts_with("--") {
                return false;
            }
            i += 1;
        }
    }
    true
}
fn action(command: CliCommand, args: &[String]) -> Result<CliReport, (CliOutcome, &'static str)> {
    if !validate_args(command, args) {
        return Err((CliOutcome::InvalidInput, "invalid arguments"));
    }
    // Objective text is descriptive, not an executable task contract. Until
    // coding is wired to its own effects and verifier, only explicit fixture
    // selection may reach the legacy planner. Check before touching run state
    // or loading an inference profile.
    if command == CliCommand::Run && flag(args, "--task-family").as_deref() != Some("sum-v1") {
        return Err((
            CliOutcome::InvalidInput,
            "unsupported or missing task family; use --task-family sum-v1 only for the legacy fixture",
        ));
    }
    let run_dir = match command {
        CliCommand::Run => flag(args, "--run-dir").map(PathBuf::from),
        _ => args
            .first()
            .filter(|s| !s.starts_with('-'))
            .map(PathBuf::from),
    }
    .ok_or((CliOutcome::InvalidInput, "missing run directory"))?;
    let local = if let Some(profile) = flag(args, "--model-profile") {
        let worker = flag(args, "--worker").ok_or((CliOutcome::InvalidInput, "missing worker"))?;
        let heldout = flag(args, "--heldout")
            .ok_or((CliOutcome::InvalidInput, "missing frozen heldout corpus"))?;
        let cache = flag(args, "--model-cache")
            .map(PathBuf::from)
            .or_else(|| {
                env::var_os("XDG_CACHE_HOME").map(|path| PathBuf::from(path).join("metron/models"))
            })
            .or_else(|| {
                env::var_os("HOME").map(|path| PathBuf::from(path).join(".cache/metron/models"))
            })
            .ok_or((CliOutcome::InvalidInput, "model cache unavailable"))?;
        Some(local_run::load(
            std::path::Path::new(&profile),
            std::path::Path::new(&worker),
            &cache,
            std::path::Path::new(&heldout),
        )?)
    } else {
        if ["--worker", "--model-cache", "--heldout"]
            .iter()
            .any(|name| flag(args, name).is_some())
        {
            return Err((
                CliOutcome::InvalidInput,
                "local options require a selected model profile",
            ));
        }
        None
    };
    let db_path = run_dir.join("runtime.sqlite");
    let mut store = if command == CliCommand::Run {
        let task_path = flag(args, "--task").ok_or((CliOutcome::InvalidInput, "missing task"))?;
        let input = fs::File::open(task_path)
            .map_err(|_| (CliOutcome::InvalidInput, "cannot read task"))?;
        let mut bytes = Vec::new();
        input
            .take((metron_protocol::MAX_ENVELOPE + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| (CliOutcome::InvalidInput, "cannot read task"))?;
        let mut task: Task =
            parse_envelope(&bytes).map_err(|_| (CliOutcome::InvalidInput, "invalid task"))?;
        if task.coding().is_some() {
            return Err((
                CliOutcome::InvalidInput,
                "coding contract cannot select the legacy sum family",
            ));
        }
        if local.is_some() {
            let mut data = task.data().clone();
            data.schema_version = SchemaVersion::V2;
            task = Task::new(data).map_err(|_| (CliOutcome::InvalidInput, "invalid local task"))?;
        }
        let mode = match flag(args, "--mode").as_deref().unwrap_or("agent") {
            "agent" => EngineMode::Agent,
            "harness" => EngineMode::Harness,
            _ => return Err((CliOutcome::InvalidInput, "unknown mode")),
        };
        if run_dir.exists() {
            if !run_dir.is_dir()
                || fs::read_dir(&run_dir)
                    .map_err(|_| {
                        (
                            CliOutcome::OperationalFailure,
                            "cannot inspect run directory",
                        )
                    })?
                    .next()
                    .is_some()
            {
                return Err((CliOutcome::InvalidInput, "run directory is populated"));
            }
        } else {
            fs::create_dir_all(&run_dir).map_err(|_| {
                (
                    CliOutcome::OperationalFailure,
                    "cannot create run directory",
                )
            })?;
        }
        let mut store = SqliteStore::create(&db_path)
            .map_err(|_| (CliOutcome::OperationalFailure, "cannot create database"))?;
        let version = match (mode, local.is_some()) {
            (EngineMode::Agent, false) => "sum-agent/v1",
            (EngineMode::Harness, false) => "sum-harness/v1",
            (EngineMode::Agent, true) => "sum-local-agent/v2",
            (EngineMode::Harness, true) => "sum-local-harness/v2",
        };
        let genesis = Genesis {
            coding_policy: None,
            mode,
            engine_version: BoundedText::new(version)
                .map_err(|_| (CliOutcome::InvalidInput, "invalid version"))?,
            require_publication_approval: args
                .iter()
                .any(|a| a == "--require-publication-approval"),
            policy_digest: content_digest(b"metron-alpha0-host-policy-v1"),
            local_profile: local.as_ref().map(|run| run.selected.profile.clone()),
        };
        let schema_version = task.data().schema_version;
        let command = make_command(
            StoreOperation::CreateTask { task, genesis },
            None,
            schema_version,
        )
        .map_err(|_| (CliOutcome::InvalidInput, "invalid task command"))?;
        if !matches!(
            store.apply(command).map_err(audit_error)?,
            WriteOutcome::Committed(_)
        ) {
            return Err((CliOutcome::Rejected, "task not created"));
        }
        store
    } else if matches!(command, CliCommand::Replay | CliCommand::Verify) {
        SqliteStore::open_read_only(&db_path).map_err(audit_error)?
    } else {
        SqliteStore::open_existing(&db_path).map_err(audit_error)?
    };
    let ids = store.task_ids().map_err(audit_error)?;
    if ids.len() != 1 {
        return Err((CliOutcome::IntegrityFailure, "expected one task"));
    }
    let task_id = ids[0].clone();
    let schema_version = {
        let mut snapshot = store.audit().map_err(audit_error)?;
        snapshot
            .projection(&task_id)
            .map_err(audit_error)?
            .task
            .data()
            .schema_version
    };
    if matches!(command, CliCommand::Resume | CliCommand::Reconcile) {
        let tick = flag(args, "--advance-tick");
        let expected = flag(args, "--expected-tick");
        let id = flag(args, "--command-id");
        if tick.is_some() || expected.is_some() || id.is_some() {
            let parse_tick = |s: Option<String>| {
                s.and_then(|v| v.parse::<u64>().ok())
                    .and_then(|v| SqlU64::new(v).ok())
                    .ok_or((CliOutcome::InvalidInput, "invalid tick arguments"))
            };
            let op = StoreOperation::AdvanceTick {
                expected_tick: parse_tick(expected)?,
                new_tick: parse_tick(tick)?,
            };
            let id = CommandId::new(id.ok_or((CliOutcome::InvalidInput, "missing command id"))?)
                .map_err(|_| (CliOutcome::InvalidInput, "invalid command id"))?;
            if !matches!(
                store
                    .apply(
                        make_command(op, Some(id), schema_version)
                            .map_err(|_| (CliOutcome::InvalidInput, "invalid tick command"))?
                    )
                    .map_err(audit_error)?,
                WriteOutcome::Committed(_)
            ) {
                return Err((CliOutcome::Blocked, "tick conflict"));
            }
        }
    }
    let mut result = report(command, CliOutcome::Succeeded, None);
    if command == CliCommand::Approval {
        let request_id = ApprovalId::new(
            flag(args, "--request-id").ok_or((CliOutcome::InvalidInput, "missing request id"))?,
        )
        .map_err(|_| (CliOutcome::InvalidInput, "invalid request id"))?;
        let action_digest = Digest::new(
            flag(args, "--action-digest")
                .ok_or((CliOutcome::InvalidInput, "missing action digest"))?,
        )
        .map_err(|_| (CliOutcome::InvalidInput, "invalid action digest"))?;
        let decision = match flag(args, "--decision").as_deref() {
            Some("approve") => ApprovalDecision::Approve,
            Some("deny") => ApprovalDecision::Deny,
            _ => return Err((CliOutcome::InvalidInput, "invalid decision")),
        };
        let id = CommandId::new(
            flag(args, "--command-id").ok_or((CliOutcome::InvalidInput, "missing command id"))?,
        )
        .map_err(|_| (CliOutcome::InvalidInput, "invalid command id"))?;
        let op = StoreOperation::RecordApproval {
            request_id,
            action_digest,
            decision,
        };
        if !matches!(
            store
                .apply(
                    make_command(op, Some(id), schema_version)
                        .map_err(|_| (CliOutcome::InvalidInput, "invalid approval"))?
                )
                .map_err(audit_error)?,
            WriteOutcome::Committed(_)
        ) {
            return Err((CliOutcome::Rejected, "approval rejected"));
        }
    } else if command == CliCommand::Reconcile {
        let intent = Digest::new(
            flag(args, "--intent").ok_or((CliOutcome::InvalidInput, "missing intent"))?,
        )
        .map_err(|_| (CliOutcome::InvalidInput, "invalid intent"))?;
        let status = orchestrator::reconcile(&mut store, &task_id, &intent, &run_dir)
            .map_err(drive_error)?;
        if matches!(status, DriveStatus::Blocked | DriveStatus::MultipleEvidence) {
            result.outcome = CliOutcome::Blocked;
            if status == DriveStatus::MultipleEvidence {
                result.diagnostic = Some(
                    BoundedText::new("multiple evidence records remain unresolved")
                        .map_err(|_| (CliOutcome::OperationalFailure, "diagnostic failure"))?,
                );
            }
        }
    } else if matches!(command, CliCommand::Run | CliCommand::Resume) {
        let projection = {
            let mut snapshot = store.audit().map_err(audit_error)?;
            snapshot.projection(&task_id).map_err(audit_error)?
        };
        let genesis = projection.genesis;
        let engine = cognition(&genesis, &projection.task)?;
        if let Some(run) = &local
            && genesis.local_profile.as_ref() != Some(&run.selected.profile)
        {
            return Err((
                CliOutcome::IntegrityFailure,
                "selected profile differs from task genesis",
            ));
        }
        let status = if let Some(run) = &local {
            let mut driver = local_run::driver(run)?;
            orchestrator::drive_with_driver(
                &mut store,
                &task_id,
                engine.as_ref(),
                &run_dir,
                &mut NoObserver,
                &mut driver,
            )
            .map_err(drive_error)?
        } else {
            orchestrator::drive(
                &mut store,
                &task_id,
                engine.as_ref(),
                &run_dir,
                &mut NoObserver,
            )
            .map_err(drive_error)?
        };
        result.outcome = match status {
            DriveStatus::Complete => CliOutcome::Succeeded,
            DriveStatus::Rejected => CliOutcome::Rejected,
            DriveStatus::Denied => CliOutcome::Denied,
            DriveStatus::Blocked | DriveStatus::MultipleEvidence => CliOutcome::Blocked,
        };
    }
    let mut snapshot = store.audit().map_err(audit_error)?;
    let projection = snapshot.projection(&task_id).map_err(audit_error)?;
    if matches!(command, CliCommand::Replay | CliCommand::Verify) {
        let checks = metron_harness::audit::verify(&mut snapshot, &task_id).map_err(audit_error)?;
        let engine = cognition(&projection.genesis, &projection.task)?;
        let behavior = if checks.chain_valid {
            Some(
                metron_harness::replay::behavior(&mut snapshot, &task_id, engine.as_ref())
                    .map_err(audit_error)?,
            )
        } else {
            None
        };
        let artifact = if command == CliCommand::Verify
            && projection.verification.plan.is_some()
            && !projection.verification.unresolved
            && !projection.verification.conflict
        {
            match metron_harness::verify::verify(&run_dir, &projection.verification) {
                Ok((_, accepted)) => Some(accepted),
                Err(metron_harness::verify::VerifyError::InvalidEvidence(_)) => Some(false),
                Err(metron_harness::verify::VerifyError::Operational) => {
                    return Err((CliOutcome::OperationalFailure, "artifact inaccessible"));
                }
            }
        } else {
            None
        };
        let valid = checks.chain_valid
            && checks.accounting_valid
            && checks.projection_valid
            && behavior == Some(true)
            && artifact != Some(false);
        result.audit = Some(CliAudit {
            chain_valid: Some(checks.chain_valid),
            accounting_valid: checks.chain_valid.then_some(checks.accounting_valid),
            projection_valid: checks.chain_valid.then_some(checks.projection_valid),
            behavior_valid: behavior,
            artifact_valid: artifact,
        });
        if !valid {
            result.outcome = CliOutcome::IntegrityFailure;
        }
    }
    let pending = snapshot
        .pending_inbox(&task_id)
        .map_err(audit_error)?
        .is_some();
    let mut cursor = None;
    let mut held = false;
    let mut approval = None;
    let (mut reported_read, mut reported_model, mut reported_publication) = (None, None, None);
    loop {
        let page = snapshot.intents(&task_id, cursor).map_err(audit_error)?;
        for intent in page.records {
            let usage = reported_usage(&mut snapshot, &intent).map_err(audit_error)?;
            match intent.intent.kind {
                metron_protocol::IntentKind::Coding { step } => match step.as_ref() {
                    metron_protocol::coding_runtime::CodingStep::Model { .. } => {
                        reported_model = usage
                    }
                    metron_protocol::coding_runtime::CodingStep::Tool { .. }
                    | metron_protocol::coding_runtime::CodingStep::Freeze { .. } => {
                        reported_publication = usage
                    }
                },
                metron_protocol::IntentKind::ReadFixture => reported_read = usage,
                metron_protocol::IntentKind::RequestModel { .. }
                | metron_protocol::IntentKind::RequestLocalModel { .. } => reported_model = usage,
                metron_protocol::IntentKind::PublishArtifact { .. } => reported_publication = usage,
            }
            held |= intent
                .reservation
                .is_some_and(|r| r.status == metron_protocol::store::ReservationStatus::Held);
            if let Some(a) = intent.approval {
                approval = Some(CliApproval {
                    request_id: a.request_id,
                    action_digest: a.action_digest,
                    decision: a.decision,
                });
            }
        }
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    let v = &projection.verification;
    let breached = projection.task_account.breached || projection.principal_account.breached;
    let incomplete = projection.task_account.incomplete || projection.principal_account.incomplete;
    let integrity_hold = projection.task_account.integrity_hold
        || projection.principal_account.integrity_hold
        || v.conflict;
    result.health = Some(CliHealth {
        verified_success: result.outcome != CliOutcome::IntegrityFailure
            && projection.state.phase == Phase::Completed
            && !pending
            && !v.unresolved
            && !held
            && !breached
            && !incomplete
            && !integrity_hold,
        pending_input: pending,
        unresolved_dispatch: v.unresolved,
        reservations_held: held,
        incomplete,
        breached,
        integrity_hold,
    });
    result.accounting = Some(CliAccounting {
        task_posted: projection.task_account.posted,
        principal_posted: projection.principal_account.posted,
        reported_read,
        reported_model,
        reported_publication,
    });
    result.pending_intent = projection.state.pending_intent.clone();
    result.phase = Some(projection.state.phase);
    result.replay_head = Some(projection.replay_head);
    result.approval = approval;
    result.task_id = Some(task_id);
    Ok(result)
}
fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "coding") {
        return coding_run::execute(&args[1..]);
    }
    if args.first().is_some_and(|arg| arg == "model") {
        return model_cli::execute(&args[1..]);
    }
    if args.first().is_some_and(|arg| arg == "toolkit") {
        return toolkit::execute(&args[1..]);
    }
    let command = match args.first().map(String::as_str) {
        Some("run") => CliCommand::Run,
        Some("resume") => CliCommand::Resume,
        Some("reconcile") => CliCommand::Reconcile,
        Some("approval") => CliCommand::Approval,
        Some("replay") => CliCommand::Replay,
        Some("verify") => CliCommand::Verify,
        _ => {
            return emit(report(
                CliCommand::Run,
                CliOutcome::InvalidInput,
                Some("unknown command"),
            ));
        }
    };
    match action(command, &args[1..]) {
        Ok(result) => emit(result),
        Err((outcome, message)) => emit(report(command, outcome, Some(message))),
    }
}
