//! The composition root, as a library so that tests can compose exactly
//! what the binary composes.
//!
//! This is the only crate that sees the laboratory, the application layer,
//! the reference operators and the adapters together. It builds a
//! [`ComposedWorld`] (oracle, public knowledge and a service backend behind
//! one identifier counter), registers every operator, turns manifest
//! schedules into app schedules, runs, resumes and replays episodes, and
//! orchestrates headroom measurements and renders the front-page figures
//! from committed reports.

pub mod compose;
pub mod figures;
pub mod headroom;
pub mod promote;
pub mod retrieval;
pub mod run;
pub mod world;

pub use compose::{
    family_labels, load_manifest, registry, resolve_fixture, schedule_from, scheduler_for,
    vocabulary,
};
pub use figures::{Figure, render_figures, stale_figures, write_figures};
pub use headroom::{HeadroomOptions, HeadroomOutcome, run_headroom};
pub use promote::{PromoteOptions, PromoteOutcome, run_promote};
pub use retrieval::{RetrievalOptions, RetrievalOutcome, run_retrieval};
pub use run::{Backend, ReplayReport, RunOptions, RunStatus, answer, replay, resume, run};
pub use world::ComposedWorld;
