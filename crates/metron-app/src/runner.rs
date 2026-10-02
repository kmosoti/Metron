//! The episode runner.
//!
//! One episode is one bounded run of the system on one inquiry. The runner:
//!
//! 1. asks the scheduler for the next operator and skips it if it is not
//!    applicable;
//! 2. applies it, then **drains the world's receipts and service answers and
//!    journals every one of them before looking at the operator's result**,
//!    so a receipt is recorded whether the operator succeeded, failed or
//!    lied;
//! 3. checks that the operator did only what its kind allows: observe
//!    operators write only the observations frame and are the only ones
//!    that may probe; consult operators write only the consultations frame
//!    and are the only ones that may call a service; transform operators
//!    write only their contract's target frame from views in its source
//!    frames; commit operators write no views;
//! 4. stamps the contract onto every view a transform wrote and journals the
//!    write with the observations it transitively rests on;
//! 5. stops on an answer, an exhausted schedule, the step limit, a stall,
//!    the budget, an operator error, or a contract violation; or
//!    **suspends** when a consult operator is waiting for an answer,
//!    returning a checkpoint from which [`EpisodeRunner::resume`] continues.

use crate::registry::OperatorRegistry;
use crate::schedule::Scheduler;
use metron_core::checkpoint::{EpisodeCheckpoint, PendingRequest};
use metron_core::clock::Clock;
use metron_core::cost::Cost;
use metron_core::evidence::independent_observations;
use metron_core::frame::{consultations_frame, observations_frame};
use metron_core::hash::ContentHash;
use metron_core::id::{EpisodeId, FrameId, OperatorId, ViewId};
use metron_core::inquiry::Inquiry;
use metron_core::journal::{Episode, EpisodeOutcome, JournalEvent, StopReason};
use metron_core::operator::{OperatorError, OperatorKind, OperatorSpec};
use metron_core::receipt::{OperationKind, ResourceReceipt};
use metron_core::rng::Rng;
use metron_core::world::Receipts;
use std::collections::BTreeMap;

/// Limits on an episode that are not part of the inquiry's cost budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunConfig {
    /// Maximum scheduler ticks (applied or skipped operators).
    pub max_steps: u32,
    /// Consecutive applications that change nothing before stopping; `0`
    /// disables stall detection.
    pub stall_limit: u32,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            max_steps: 1_000,
            stall_limit: 0,
        }
    }
}

/// Errors that abort a run before it can be journaled as an outcome.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RunError {
    /// The registry is empty.
    #[error("registry has no operators")]
    EmptyRegistry,
    /// The scheduler named an operator that is not registered.
    #[error("scheduled operator {0} is not registered")]
    UnknownOperator(OperatorId),
}

/// How a run returned.
#[derive(Clone, Debug, PartialEq)]
pub enum RunOutcome {
    /// The episode ended; the journal carries the outcome.
    Completed(Episode),
    /// The episode is waiting for an external service.
    Suspended(Box<EpisodeCheckpoint>),
}

impl RunOutcome {
    /// The episode, if it completed.
    #[must_use]
    pub fn completed(self) -> Option<Episode> {
        match self {
            RunOutcome::Completed(e) => Some(e),
            RunOutcome::Suspended(_) => None,
        }
    }

    /// The journal so far, completed or not.
    #[must_use]
    pub fn episode(&self) -> &Episode {
        match self {
            RunOutcome::Completed(e) => e,
            RunOutcome::Suspended(c) => &c.episode,
        }
    }
}

/// Runs episodes against a world `W`.
pub struct EpisodeRunner<W> {
    registry: OperatorRegistry<W>,
    clock: Box<dyn Clock>,
    config: RunConfig,
}

struct LoopState {
    episode: Episode,
    ticks: u32,
    unchanged_streak: u32,
    rng: Rng,
    cost_at_start: Cost,
}

impl<W: Receipts> EpisodeRunner<W> {
    /// Creates a runner.
    pub fn new(registry: OperatorRegistry<W>, clock: impl Clock + 'static) -> Self {
        Self {
            registry,
            clock: Box::new(clock),
            config: RunConfig::default(),
        }
    }

    /// Sets the run limits.
    #[must_use]
    pub fn with_config(mut self, config: RunConfig) -> Self {
        self.config = config;
        self
    }

    /// The registry.
    #[must_use]
    pub fn registry(&self) -> &OperatorRegistry<W> {
        &self.registry
    }

    /// The run limits.
    #[must_use]
    pub fn config(&self) -> RunConfig {
        self.config
    }

    /// Runs one episode. The inquiry is updated in place; the returned
    /// outcome carries the journal, complete or suspended.
    pub fn run(
        &self,
        episode_id: EpisodeId,
        seed: u64,
        manifest_hash: ContentHash,
        inquiry: &mut Inquiry,
        world: &mut W,
        scheduler: &mut dyn Scheduler,
    ) -> Result<RunOutcome, RunError> {
        if self.registry.is_empty() {
            return Err(RunError::EmptyRegistry);
        }
        let mut episode = Episode::new(episode_id, inquiry.id, seed, manifest_hash);
        episode.append(
            self.clock.now_nanos(),
            JournalEvent::EpisodeStarted {
                episode: episode_id,
                inquiry: inquiry.id,
                question: inquiry.question.clone(),
                manifest_hash,
                seed,
                code_version: metron_core::code_version(),
            },
        );
        let state = LoopState {
            episode,
            ticks: 0,
            unchanged_streak: 0,
            rng: Rng::seed_from_u64(seed),
            cost_at_start: inquiry.spent,
        };
        self.drive(state, inquiry, world, scheduler, None)
    }

    /// Resumes a suspended episode. The pending operator is retried first;
    /// the world must now be able to answer its request. The inquiry is
    /// restored from the checkpoint into `inquiry`.
    pub fn resume(
        &self,
        checkpoint: EpisodeCheckpoint,
        inquiry: &mut Inquiry,
        world: &mut W,
        scheduler: &mut dyn Scheduler,
    ) -> Result<RunOutcome, RunError> {
        if self.registry.is_empty() {
            return Err(RunError::EmptyRegistry);
        }
        let EpisodeCheckpoint {
            mut episode,
            inquiry: saved,
            pending,
            ticks,
            unchanged_streak,
            rng,
            ..
        } = checkpoint;
        *inquiry = saved;
        episode.append(
            self.clock.now_nanos(),
            JournalEvent::Resumed {
                step: ticks,
                request_id: pending.request_id,
            },
        );
        let cost_at_start = inquiry.spent.saturating_sub(
            episode
                .entries
                .iter()
                .filter_map(|e| match &e.event {
                    JournalEvent::OperatorApplied { cost, .. } => Some(*cost),
                    JournalEvent::EpisodeEnded { .. } => None,
                    _ => None,
                })
                .fold(Cost::ZERO, |acc, c| acc + c),
        );
        let state = LoopState {
            episode,
            ticks,
            unchanged_streak,
            rng,
            cost_at_start,
        };
        self.drive(state, inquiry, world, scheduler, Some(pending.operator))
    }

    fn drive(
        &self,
        mut state: LoopState,
        inquiry: &mut Inquiry,
        world: &mut W,
        scheduler: &mut dyn Scheduler,
        mut first: Option<OperatorId>,
    ) -> Result<RunOutcome, RunError> {
        for receipt in world.drain_receipts() {
            state.episode.append(
                self.clock.now_nanos(),
                JournalEvent::Receipt {
                    step: state.ticks,
                    receipt,
                },
            );
        }

        let stop = loop {
            if inquiry.is_answered() {
                break StopReason::Answered;
            }
            if let Some(dimension) = inquiry.budget.exceeded_by(&inquiry.spent) {
                break StopReason::BudgetExceeded { dimension };
            }
            if state.ticks >= self.config.max_steps {
                break StopReason::MaxSteps;
            }
            if self.config.stall_limit > 0 && state.unchanged_streak >= self.config.stall_limit {
                break StopReason::Stalled;
            }
            let operator_id = match first.take() {
                Some(op) => op,
                None => match scheduler.next(inquiry) {
                    Some(op) => op,
                    None => break StopReason::ScheduleExhausted,
                },
            };
            let Some(operator) = self.registry.get(&operator_id) else {
                return Err(RunError::UnknownOperator(operator_id));
            };
            let step = state.ticks;
            if !operator.applicable(inquiry, world) {
                state.ticks += 1;
                state.episode.append(
                    self.clock.now_nanos(),
                    JournalEvent::OperatorSkipped {
                        step,
                        operator: operator_id,
                    },
                );
                continue;
            }

            let spec = operator.spec();
            let input_hash = inquiry.state_hash();
            let versions_before = inquiry.view_versions();
            let observations_before = inquiry.observations.len();
            let answered_before = inquiry.is_answered();
            let snapshot = inquiry.clone();
            let rng_before = state.rng.clone();

            let result = operator.apply(inquiry, world, &mut state.rng);

            let receipts = world.drain_receipts();
            let answers = world.drain_service_answers();
            let receipt_cost = receipts.iter().fold(Cost::ZERO, |acc, r| acc + r.cost);
            for receipt in &receipts {
                state.episode.append(
                    self.clock.now_nanos(),
                    JournalEvent::Receipt {
                        step,
                        receipt: receipt.clone(),
                    },
                );
            }
            for answer in answers {
                state.episode.append(
                    self.clock.now_nanos(),
                    JournalEvent::ServiceAnswered { step, answer },
                );
            }

            let outcome = match result {
                Ok(outcome) => outcome,
                Err(OperatorError::Pending { request_id, .. }) => {
                    let OperatorKind::Consult { service } = &spec.kind else {
                        inquiry.spent += Cost::operator_call() + receipt_cost;
                        state.ticks += 1;
                        break StopReason::OperatorFailed {
                            operator: operator_id,
                            message: "only consult operators may wait for a service".into(),
                        };
                    };
                    *inquiry = snapshot;
                    inquiry.spent += receipt_cost;
                    state.episode.append(
                        self.clock.now_nanos(),
                        JournalEvent::Suspended {
                            step,
                            operator: operator_id.clone(),
                            service: service.clone(),
                            request_id: request_id.clone(),
                        },
                    );
                    return Ok(RunOutcome::Suspended(Box::new(EpisodeCheckpoint {
                        episode: state.episode,
                        inquiry: inquiry.clone(),
                        pending: PendingRequest {
                            request_id,
                            service: service.clone(),
                            operator: operator_id,
                        },
                        ticks: state.ticks,
                        unchanged_streak: state.unchanged_streak,
                        rng: rng_before,
                        scheduler_state: scheduler.state(),
                        world_state: serde_json::Value::Null,
                    })));
                }
                Err(error) => {
                    inquiry.spent += Cost::operator_call() + receipt_cost;
                    state.ticks += 1;
                    break StopReason::OperatorFailed {
                        operator: operator_id,
                        message: error.to_string(),
                    };
                }
            };
            state.ticks += 1;
            let cost = Cost::operator_call() + outcome.work + receipt_cost;
            inquiry.spent += cost;
            inquiry.steps += 1;

            let written = written_views(&versions_before, &inquiry.view_versions());
            if let Some((view, message)) = check_writes(
                &spec,
                &written,
                inquiry,
                &receipts,
                observations_before,
                answered_before,
            ) {
                break StopReason::ContractViolation {
                    operator: operator_id,
                    view,
                    message,
                };
            }

            for view_id in &written {
                if let OperatorKind::Transform { contract } = &spec.kind
                    && let Some(view) = inquiry.views.get_mut(view_id)
                {
                    view.derivation.contract = Some(contract.clone());
                }
                let observations: Vec<_> =
                    inquiry.observation_closure(view_id).into_iter().collect();
                if let Some(view) = inquiry.views.get(view_id) {
                    state.episode.append(
                        self.clock.now_nanos(),
                        JournalEvent::ViewWritten {
                            step,
                            view: view_id.clone(),
                            frame: view.frame.clone(),
                            version: view.version,
                            contract: view.derivation.contract.clone(),
                            observations,
                            content_hash: view.content.content_hash(),
                        },
                    );
                }
            }
            if !answered_before && let Some(answer) = &inquiry.answer {
                state.episode.append(
                    self.clock.now_nanos(),
                    JournalEvent::Answered {
                        step,
                        operator: answer.by.clone(),
                        frame: answer.frame.clone(),
                        content_hash: answer.content.content_hash(),
                        confidence: answer.confidence,
                        independent_observations: independent_observations(&answer.evidence),
                    },
                );
            }

            let output_hash = inquiry.state_hash();
            state.unchanged_streak = if output_hash == input_hash {
                state.unchanged_streak + 1
            } else {
                0
            };
            state.episode.append(
                self.clock.now_nanos(),
                JournalEvent::OperatorApplied {
                    step,
                    operator: operator_id,
                    kind: spec.kind.clone(),
                    status: outcome.status.clone(),
                    cost,
                    input_hash,
                    output_hash,
                    note: outcome.note.clone(),
                },
            );
        };

        let cost = inquiry.spent.saturating_sub(state.cost_at_start);
        let answer_hash = inquiry.answer.as_ref().map(|a| a.content.content_hash());
        state.episode.append(
            self.clock.now_nanos(),
            JournalEvent::EpisodeEnded {
                stop: stop.clone(),
                steps: state.ticks,
                cost,
                answer_hash,
            },
        );
        state.episode.outcome = Some(EpisodeOutcome {
            answered: stop.is_answered(),
            stop,
            steps: state.ticks,
            cost,
        });
        Ok(RunOutcome::Completed(state.episode))
    }
}

fn written_views(
    before: &BTreeMap<ViewId, (FrameId, u32)>,
    after: &BTreeMap<ViewId, (FrameId, u32)>,
) -> Vec<ViewId> {
    after
        .iter()
        .filter(|(id, state)| before.get(*id) != Some(state))
        .map(|(id, _)| id.clone())
        .collect()
}

/// Returns the first violated rule as `(view, message)`.
fn check_writes(
    spec: &OperatorSpec,
    written: &[ViewId],
    inquiry: &Inquiry,
    receipts: &[ResourceReceipt],
    observations_before: usize,
    answered_before: bool,
) -> Option<(ViewId, String)> {
    let no_view = || ViewId::from("-");
    let observed = inquiry.observations.len() > observations_before
        || receipts
            .iter()
            .any(|r| r.operation == OperationKind::OracleProbe);
    let consulted = receipts
        .iter()
        .any(|r| matches!(r.operation, OperationKind::ExternalCall { .. }));
    let committed = !answered_before && inquiry.is_answered();
    for view_id in written {
        if !spec.writes.contains(view_id) {
            return Some((
                view_id.clone(),
                "wrote a view not declared in `writes`".into(),
            ));
        }
    }
    let only_in_frame = |frame: FrameId, what: &str| -> Option<(ViewId, String)> {
        for view_id in written {
            if inquiry.views.get(view_id).is_some_and(|v| v.frame != frame) {
                return Some((
                    view_id.clone(),
                    format!("{what} operator wrote outside the `{frame}` frame"),
                ));
            }
        }
        None
    };
    match &spec.kind {
        OperatorKind::Observe => {
            if let Some(v) = only_in_frame(observations_frame(), "observe") {
                return Some(v);
            }
            if consulted {
                return Some((no_view(), "observe operator consulted a service".into()));
            }
            if committed {
                return Some((no_view(), "observe operator committed an answer".into()));
            }
        }
        OperatorKind::Consult { service } => {
            if let Some(v) = only_in_frame(consultations_frame(), "consult") {
                return Some(v);
            }
            if observed {
                return Some((no_view(), "consult operator obtained observations".into()));
            }
            if committed {
                return Some((no_view(), "consult operator committed an answer".into()));
            }
            for r in receipts {
                if let OperationKind::ExternalCall { service: called } = &r.operation
                    && called != service
                {
                    return Some((
                        no_view(),
                        format!("consult operator for `{service}` called `{called}`"),
                    ));
                }
            }
        }
        OperatorKind::Transform { contract } => {
            if observed {
                return Some((no_view(), "transform operator obtained observations".into()));
            }
            if consulted {
                return Some((no_view(), "transform operator consulted a service".into()));
            }
            if committed {
                return Some((no_view(), "transform operator committed an answer".into()));
            }
            for view_id in written {
                let Some(view) = inquiry.views.get(view_id) else {
                    continue;
                };
                if view.frame != contract.to {
                    return Some((
                        view_id.clone(),
                        format!(
                            "written in frame `{}` but the contract targets `{}`",
                            view.frame, contract.to
                        ),
                    ));
                }
                for input in &view.derivation.inputs {
                    if let Some(src) = inquiry.views.get(input)
                        && !contract.from.contains(&src.frame)
                    {
                        return Some((
                            view_id.clone(),
                            format!(
                                "derived from view `{input}` in frame `{}`, which the contract does not read",
                                src.frame
                            ),
                        ));
                    }
                }
            }
        }
        OperatorKind::Commit => {
            if observed {
                return Some((no_view(), "commit operator obtained observations".into()));
            }
            if consulted {
                return Some((no_view(), "commit operator consulted a service".into()));
            }
            if let Some(view_id) = written.first() {
                return Some((view_id.clone(), "commit operator wrote a view".into()));
            }
        }
    }
    None
}
