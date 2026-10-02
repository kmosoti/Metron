//! The composition root, as a library so that tests can compose exactly
//! what the binary composes.
//!
//! This is the only crate that sees the laboratory, the application layer,
//! the reference operators and the adapters together. It builds a
//! [`ComposedWorld`] (oracle, public knowledge and a service backend behind
//! one identifier counter), registers every operator, turns manifest
//! schedules into app schedules, runs, resumes and replays episodes, and
//! orchestrates headroom measurements.

pub mod compose;
pub mod headroom;
pub mod run;
pub mod world;

pub use compose::{family_labels, load_manifest, registry, resolve_fixture, schedule_from};
pub use headroom::{HeadroomOptions, HeadroomOutcome, run_headroom};
pub use run::{Backend, ReplayReport, RunOptions, RunStatus, answer, replay, resume, run};
pub use world::ComposedWorld;
