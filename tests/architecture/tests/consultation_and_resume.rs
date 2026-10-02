//! Milestone M6: an LLM can propose, never decide.
//!
//! Consultations go through the `ExternalService` port, are receipted and
//! journaled in full, suspend the episode when unanswered, resume once a
//! Claude Code session (here: the test) has written the answer, and replay
//! from the journal without the session. A proposal that disagrees with an
//! observation never reaches the truth-table frame.

use metron_adapters::StubService;
use metron_app::{EpisodeRunner, FixedSchedule, RunConfig, RunOutcome, ScheduleItem};
use metron_cli::run::Backend;
use metron_cli::{ComposedWorld, RunOptions, RunStatus, answer, registry, replay, resume, run};
use metron_core::clock::ManualClock;
use metron_core::hash::ContentHash;
use metron_core::id::{EpisodeId, InquiryId};
use metron_core::inquiry::Inquiry;
use metron_core::journal::{JournalEvent, StopReason};
use metron_core::receipt::OperationKind;
use metron_lab::{BooleanFixture, LabWorld, Protocol};
use metron_operators::{CommitTruthTable, FormulaToTable, LlmProposeFormula, ProbeNextUnobserved};
use std::fs;
use std::path::PathBuf;

const MAJORITY3: &str = r#"{"name":"majority-3","arity":3,"rows":"00010111"}"#;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("metron-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn consult_schedule(probes_first: u32) -> FixedSchedule {
    FixedSchedule::new([
        ScheduleItem::times(ProbeNextUnobserved::ID, probes_first),
        ScheduleItem::once(LlmProposeFormula::ID),
        ScheduleItem::once(FormulaToTable::ID),
        ScheduleItem::once(CommitTruthTable::ID),
    ])
}

fn run_with_stub(
    answer_text: &str,
    probes_first: u32,
) -> (metron_core::journal::Episode, Inquiry, ComposedWorld) {
    let fixture = BooleanFixture::from_json(MAJORITY3).unwrap();
    let lab = LabWorld::new(
        fixture.seal().unwrap(),
        Protocol {
            max_probes: Some(8),
        },
    );
    let mut world =
        ComposedWorld::new(lab).with_backend(Box::new(StubService::constant("llm", answer_text)));
    let mut inquiry = Inquiry::new(InquiryId(1), world.lab().question());
    let runner =
        EpisodeRunner::new(registry().unwrap(), ManualClock::new(1)).with_config(RunConfig {
            max_steps: 64,
            stall_limit: 0,
        });
    let mut schedule = consult_schedule(probes_first);
    let outcome = runner
        .run(
            EpisodeId(1),
            3,
            ContentHash::of_bytes(b"m"),
            &mut inquiry,
            &mut world,
            &mut schedule,
        )
        .unwrap();
    let RunOutcome::Completed(episode) = outcome else {
        panic!("stub service never suspends");
    };
    (episode, inquiry, world)
}

#[test]
fn consultations_are_receipted_and_journaled_in_full() {
    let (episode, inquiry, world) = run_with_stub("(x0 & x1) | (x0 & x2) | (x1 & x2)", 3);
    episode.verify().unwrap();
    let external: Vec<_> = episode
        .receipts()
        .filter(|r| matches!(r.operation, OperationKind::ExternalCall { .. }))
        .collect();
    assert_eq!(external.len(), 1, "one receipt per consultation");
    assert_eq!(inquiry.spent.external_calls, 1, "the call is charged");
    let answers: Vec<_> = episode.service_answers().collect();
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0].receipt, external[0].id);
    assert_eq!(answers[0].answered_by, "stub");
    assert!(
        answers[0].request.as_json().unwrap()["prompt"]
            .as_str()
            .unwrap()
            .contains("-> ")
    );
    let verdict = world.lab().judge(&inquiry);
    assert!(
        verdict.correct,
        "the verified proposal is right: {verdict:?}"
    );
    assert_eq!(verdict.probes_answered, 3);
}

#[test]
fn a_proposal_refuted_by_an_observation_never_becomes_an_answer() {
    // x0 alone disagrees with maj3 on row 1 (x0=1, x1=0, x2=0 -> 0).
    let (episode, inquiry, _) = run_with_stub("x0", 3);
    assert!(
        !inquiry.is_answered(),
        "nothing to commit after a refuted proposal"
    );
    assert!(inquiry.view_str("table.complete").is_none());
    assert!(
        inquiry.view_str("consultations.llm").is_some(),
        "the proposal is kept, in its own frame"
    );
    let note = episode.entries.iter().find_map(|e| match &e.event {
        JournalEvent::OperatorApplied { operator, note, .. }
            if operator.as_str() == FormulaToTable::ID =>
        {
            Some(note.clone())
        }
        _ => None,
    });
    assert!(note.unwrap().contains("refuted"));
    // With no observations, any proposal is consistent; it is committed but
    // the laboratory judges it wrong.
    let (_, inquiry, world) = run_with_stub("x0", 0);
    assert!(inquiry.is_answered());
    assert!(!world.lab().judge(&inquiry).correct);
}

#[test]
fn suspend_answer_resume_and_replay_through_the_file_bridge() {
    let dir = temp_dir("bridge");
    let fixture_path = dir.join("majority3.json");
    fs::write(&fixture_path, MAJORITY3).unwrap();
    let manifest_path = dir.join("manifest.json");
    fs::write(
        &manifest_path,
        format!(
            r#"{{
  "name": "bridge-test", "seed": 5,
  "lab": {{"family": "hidden-boolean-function", "fixture": "{}", "max_probes": 8}},
  "system": {{"schedule": [
      {{"operator": "probe-next-unobserved", "repeat": 4}},
      {{"operator": "llm-propose-formula"}},
      {{"operator": "formula-to-table"}},
      {{"operator": "commit-truth-table"}}
  ], "budget": {{"max_oracle_probes": 8}}, "max_steps": 32}}
}}"#,
            fixture_path.display()
        ),
    )
    .unwrap();
    let options = RunOptions {
        results_root: dir.join("results"),
        backend: Backend::ClaudeCode,
        ..RunOptions::default()
    };
    let status = run(&manifest_path, &options).unwrap();
    let RunStatus::Suspended {
        dir: run_dir,
        request_id,
        request_path,
        prompt,
    } = status
    else {
        panic!("expected suspension while the request is unanswered");
    };
    assert!(request_path.exists());
    assert!(run_dir.join("checkpoint.json").exists());
    assert!(prompt.unwrap().contains("Propose the simplest formula"));

    // Resuming without an answer suspends again on the same request.
    let again = resume(&run_dir).unwrap();
    assert!(matches!(&again, RunStatus::Suspended { request_id: r, .. } if *r == request_id));

    // The "session" answers.
    answer(
        &run_dir,
        None,
        "(x0 & x1) | (x0 & x2) | (x1 & x2)",
        "test-session",
    )
    .unwrap();
    let done = resume(&run_dir).unwrap();
    let RunStatus::Completed {
        record, verdict, ..
    } = done
    else {
        panic!("expected completion after the answer");
    };
    assert!(verdict.correct);
    assert!(
        !run_dir.join("checkpoint.json").exists(),
        "checkpoint is removed on completion"
    );
    let events: Vec<&JournalEvent> = record.episode.entries.iter().map(|e| &e.event).collect();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, JournalEvent::Suspended { .. }))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, JournalEvent::Resumed { .. }))
    );
    assert_eq!(record.episode.service_answers().count(), 1);
    assert_eq!(
        record.episode.service_answers().next().unwrap().answered_by,
        "test-session"
    );
    assert_eq!(
        record
            .episode
            .receipts()
            .filter(|r| r.operation == OperationKind::OracleProbe)
            .count(),
        4
    );
    assert!(matches!(
        record.episode.outcome.as_ref().unwrap().stop,
        StopReason::Answered
    ));
    record.episode.verify().unwrap();

    // Replay from the journal, without the session.
    let report = replay(&run_dir).unwrap();
    assert!(report.matches, "{report:?}");
    assert!(report.answer_matches);
    let _ = fs::remove_dir_all(&dir);
}
