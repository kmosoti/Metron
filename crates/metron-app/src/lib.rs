//! Metron application layer: the use cases that drive the core.
//!
//! * [`OperatorRegistry`] — validates and holds operators for a world `W`.
//! * [`Scheduler`], [`FixedSchedule`] and [`SurvivorCountSelector`] —
//!   decide which operator runs next. The selector is the hand-authored
//!   control that ADR 0010 requires before any learned routing.
//! * [`RouterSelector`] and [`RoutingPolicy`] — route among fixed
//!   strategies by the posterior over the promised classes: the
//!   hand-authored Bayes ranking, or policies fitted on the train split
//!   ([`fit_tabular`], [`fit_ridge`]). ADR 0015.
//! * [`propose`] — turns an answered episode into a
//!   [`metron_core::capability::CapabilityCandidate`] whose program is the
//!   episode's trace generalised into a repeatable schedule.
//! * [`EpisodeRunner`] — runs one bounded episode, copies every receipt and
//!   service answer the world issues into the journal, enforces that
//!   operators write only what their kind and contract allow, and suspends
//!   to a checkpoint when a consult operator is waiting for an answer.
//!
//! This crate depends on `metron-core` only. It never sees a hidden target,
//! a verdict, or a file.

pub mod candidates;
pub mod registry;
pub mod router;
pub mod runner;
pub mod schedule;
pub mod selector;

pub use candidates::{
    compose_contracts, compress_trace, executed_trace, program_schedule, propose,
};
pub use registry::{OperatorRegistry, RegistryError};
pub use router::{
    Candidate, Example, Posterior, RouterSelector, RoutingPolicy, bucket, fit_ridge, fit_tabular,
};
pub use runner::{EpisodeRunner, RunConfig, RunError, RunOutcome};
pub use schedule::{FixedSchedule, ScheduleItem, Scheduler};
pub use selector::{SurvivorCountSelector, SurvivorRule, Vocabulary};
