//! The episode runner.
//!
//! One episode is one bounded run of the system on one inquiry. The runner:
//!
//! 1. asks the scheduler for the next operator and skips it if it is not
//!    applicable;
//! 2. applies it, then **drains the world's receipts and journals every one
//!    of them before looking at the operator's result**, so a receipt is
//!    recorded whether the operator succeeded, failed or lied;
//! 3. checks that the operator wrote only what its kind allows: observe
//!    operators write only the observations frame, transform operators write
//!    only their contract's target frame from views in its source frames,
//!    commit operators write no views, and only observe operators may obtain
//!    observations;
//! 4. stamps the contract onto every view a transform wrote and journals the
//!    write with the observations it transitively rests on;
//! 5. stops on an answer, an exhausted schedule, the step limit, a stall,
//!    the budget, an operator error, or a contract violation.

use crate::registry::OperatorRegistry;
use crate::schedule::Scheduler;
use metron_core::clock::Clock;
use metron_core::cost::Cost;
use metron_core::evidence::independent_observations;
use metron_core::frame::observations_frame;
use metron_core::hash::ContentHash;
use metron_core::id::{EpisodeId, FrameId, OperatorId, ViewId};
use metron_core::inquiry::Inquiry;
use metron_core::journal::{Episode, EpisodeOutcome, JournalEvent, StopReason};
use metron_core::operator::{OperatorKind, OperatorSpec};
use metron_core::rng::Rng;
use metron_core::world::Oracle;
use std::collections::BTreeMap;
use std::time::Instant;

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

/// Runs episodes against a world `W`.
pub struct EpisodeRunner<W> {
    registry: OperatorRegistry<W>,
    clock: Box<dyn Clock>,
    config: RunConfig,
}

impl<W: Oracle> EpisodeRunner<W> {
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
    /// episode carries the complete journal.
    pub fn run(
        &self,
        episode_id: EpisodeId,
        seed: u64,
        manifest_hash: ContentHash,
        inquiry: &mut Inquiry,
        world: &mut W,
        scheduler: &mut dyn Scheduler,
    ) -> Result<Episode, RunError> {
        if self.registry.is_empty() {
            return Err(RunError::EmptyRegistry);
        }
        let mut episode = Episode::new(episode_id, inquiry.id, seed, manifest_hash);
        let mut rng = Rng::seed_from_u64(seed);
        let cost_at_start = inquiry.spent;
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
        // Receipts issued before the episode (none expected) are journaled
        // at step 0 rather than lost.
        for receipt in world.drain_receipts() {
            episode.append(
                self.clock.now_nanos(),
                JournalEvent::Receipt { step: 0, receipt },
            );
        }

        let mut ticks: u32 = 0;
        let mut unchanged_streak: u32 = 0;
        let stop = loop {
            if inquiry.is_answered() {
                break StopReason::Answered;
            }
            if let Some(dimension) = inquiry.budget.exceeded_by(&inquiry.spent) {
                break StopReason::BudgetExceeded { dimension };
            }
            if ticks >= self.config.max_steps {
                break StopReason::MaxSteps;
            }
            if self.config.stall_limit > 0 && unchanged_streak >= self.config.stall_limit {
                break StopReason::Stalled;
            }
            let Some(operator_id) = scheduler.next(inquiry) else {
                break StopReason::ScheduleExhausted;
            };
            let Some(operator) = self.registry.get(&operator_id) else {
                return Err(RunError::UnknownOperator(operator_id));
            };
            let step = ticks;
            ticks += 1;
            if !operator.applicable(inquiry, world) {
                episode.append(
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

            let started = Instant::now();
            let result = operator.apply(inquiry, world, &mut rng);
            let wall_nanos = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);

            let receipts = world.drain_receipts();
            let receipt_cost = receipts.iter().fold(Cost::ZERO, |acc, r| acc + r.cost);
            let receipt_count = receipts.len();
            for receipt in receipts {
                episode.append(
                    self.clock.now_nanos(),
                    JournalEvent::Receipt { step, receipt },
                );
            }

            let outcome = match result {
                Ok(outcome) => outcome,
                Err(error) => {
                    inquiry.spent += Cost::operator_call() + receipt_cost;
                    break StopReason::OperatorFailed {
                        operator: operator_id,
                        message: error.to_string(),
                    };
                }
            };
            let cost = Cost::operator_call() + outcome.work + receipt_cost;
            inquiry.spent += cost;
            inquiry.steps += 1;

            let written = written_views(&versions_before, &inquiry.view_versions());
            if let Some((view, message)) = check_writes(
                &spec,
                &written,
                inquiry,
                receipt_count,
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
                    episode.append(
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
                episode.append(
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
            unchanged_streak = if output_hash == input_hash {
                unchanged_streak + 1
            } else {
                0
            };
            episode.append(
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
            let _ = wall_nanos; // wall time is recorded on receipts and entries, not used for decisions
        };

        let cost = inquiry.spent.saturating_sub(cost_at_start);
        let answer_hash = inquiry.answer.as_ref().map(|a| a.content.content_hash());
        episode.append(
            self.clock.now_nanos(),
            JournalEvent::EpisodeEnded {
                stop: stop.clone(),
                steps: ticks,
                cost,
                answer_hash,
            },
        );
        episode.outcome = Some(EpisodeOutcome {
            answered: stop.is_answered(),
            stop,
            steps: ticks,
            cost,
        });
        Ok(episode)
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

/// Returns the first violated write rule as `(view, message)`.
fn check_writes(
    spec: &OperatorSpec,
    written: &[ViewId],
    inquiry: &Inquiry,
    receipt_count: usize,
    observations_before: usize,
    answered_before: bool,
) -> Option<(ViewId, String)> {
    let no_view = || ViewId::from("-");
    let observed = inquiry.observations.len() > observations_before;
    let committed = !answered_before && inquiry.is_answered();
    for view_id in written {
        if !spec.writes.contains(view_id) {
            return Some((
                view_id.clone(),
                "wrote a view not declared in `writes`".into(),
            ));
        }
    }
    match &spec.kind {
        OperatorKind::Observe => {
            let frame = observations_frame();
            for view_id in written {
                if inquiry.views.get(view_id).is_some_and(|v| v.frame != frame) {
                    return Some((
                        view_id.clone(),
                        format!("observe operator wrote outside the `{frame}` frame"),
                    ));
                }
            }
            if committed {
                return Some((no_view(), "observe operator committed an answer".into()));
            }
        }
        OperatorKind::Transform { contract } => {
            if receipt_count > 0 || observed {
                return Some((no_view(), "transform operator obtained observations".into()));
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
            if receipt_count > 0 || observed {
                return Some((no_view(), "commit operator obtained observations".into()));
            }
            if let Some(view_id) = written.first() {
                return Some((view_id.clone(), "commit operator wrote a view".into()));
            }
        }
    }
    None
}
