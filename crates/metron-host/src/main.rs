//! Host composition for coding execution and read-only historical audit.
#![forbid(unsafe_code)]
mod coding_acceptance;
mod coding_run;
mod coding_tools;
mod comparison_runner;
mod hardware;
mod installer;
mod legacy_publication;
mod legacy_sum;
mod legacy_verify;
mod measurement;
mod model_cli;
mod qualification;
mod qualification_capture;
mod qualification_runner;
mod resource_budget;
mod session_identity;
mod toolkit;
use metron_protocol::{
    cli::{CliAccounting, CliApproval, CliAudit, CliCommand, CliHealth, CliOutcome, CliReport},
    domain::{Cognition, Phase, Task},
    store::{AuditSnapshot, EngineMode, Genesis, RuntimeSnapshot, StoreError, StorePort},
    types::{BoundedText, SchemaVersion},
};
use metron_store_sqlite::SqliteStore;
use std::{env, path::PathBuf, process::ExitCode};

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
                metron_agent::CodingAgent::legacy_v3_6(task, policy)
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
                metron_harness::planner::CodingPlanner::legacy_v3_6(task, policy)
                    .map(|c| Box::new(c) as Box<dyn Cognition>)
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid coding policy"))
            }
            (EngineMode::Agent, "coding-agent/v3.7") => {
                metron_agent::CodingAgent::legacy_v3_7(task, policy)
                    .map(|c| Box::new(c) as Box<dyn Cognition>)
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid coding policy"))
            }
            (EngineMode::Harness, "coding-harness/v3.7") => {
                metron_harness::planner::CodingPlanner::legacy_v3_7(task, policy)
                    .map(|c| Box::new(c) as Box<dyn Cognition>)
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid coding policy"))
            }
            (EngineMode::Agent, "coding-agent/v3.10") => {
                metron_agent::CodingAgent::new(task, policy)
                    .map(|c| Box::new(c) as Box<dyn Cognition>)
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid coding policy"))
            }
            (EngineMode::Harness, "coding-harness/v3.10") => {
                metron_harness::planner::CodingPlanner::new(task, policy)
                    .map(|c| Box::new(c) as Box<dyn Cognition>)
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid coding policy"))
            }
            (EngineMode::Agent, "coding-agent/v3.9") => {
                metron_agent::CodingAgent::legacy_v3_9(task, policy)
                    .map(|c| Box::new(c) as Box<dyn Cognition>)
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid coding policy"))
            }
            (EngineMode::Harness, "coding-harness/v3.9") => {
                metron_harness::planner::CodingPlanner::legacy_v3_9(task, policy)
                    .map(|c| Box::new(c) as Box<dyn Cognition>)
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid coding policy"))
            }
            (EngineMode::Agent, "coding-agent/v3.8") => {
                metron_agent::CodingAgent::legacy_v3_8(task, policy)
                    .map(|c| Box::new(c) as Box<dyn Cognition>)
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid coding policy"))
            }
            (EngineMode::Harness, "coding-harness/v3.8") => {
                metron_harness::planner::CodingPlanner::legacy_v3_8(task, policy)
                    .map(|c| Box::new(c) as Box<dyn Cognition>)
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid coding policy"))
            }
            _ => Err((CliOutcome::IntegrityFailure, "unknown coding engine")),
        };
    }
    Err((CliOutcome::IntegrityFailure, "coding task required"))
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
fn action(command: CliCommand, args: &[String]) -> Result<CliReport, (CliOutcome, &'static str)> {
    // Removed legacy execution is rejected before opening a database, loading a
    // worker, reading profiles or touching any writable run state.
    if !matches!(command, CliCommand::Replay | CliCommand::Verify) {
        return Err((
            CliOutcome::InvalidInput,
            "legacy execution removed; use coding",
        ));
    }
    if args.len() != 1 || args[0].starts_with('-') {
        return Err((CliOutcome::InvalidInput, "expected one run directory"));
    }
    let run_dir = PathBuf::from(&args[0]);
    let mut store =
        SqliteStore::open_read_only(run_dir.join("runtime.sqlite")).map_err(audit_error)?;
    let ids = store.task_ids().map_err(audit_error)?;
    if ids.len() != 1 {
        return Err((CliOutcome::IntegrityFailure, "expected one task"));
    }
    let task_id = ids[0].clone();
    let mut result = report(command, CliOutcome::Succeeded, None);
    let mut snapshot = store.audit().map_err(audit_error)?;
    let projection = snapshot.projection(&task_id).map_err(audit_error)?;
    if matches!(command, CliCommand::Replay | CliCommand::Verify) {
        let checks = metron_harness::audit::verify(&mut snapshot, &task_id).map_err(audit_error)?;
        let engine = if projection.task.coding().is_some() {
            cognition(&projection.genesis, &projection.task)?
        } else {
            legacy_sum::reader(&projection.genesis)
                .map_err(|reason| (CliOutcome::IntegrityFailure, reason))?
        };
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
            match legacy_verify::verify(&run_dir, &projection.verification) {
                Ok((_, accepted)) => Some(accepted),
                Err(legacy_verify::VerifyError::InvalidEvidence(_)) => Some(false),
                Err(legacy_verify::VerifyError::Operational) => {
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
