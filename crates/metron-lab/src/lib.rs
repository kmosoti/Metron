//! The Metron laboratory: the immutable evaluator.
//!
//! The laboratory owns everything the system under study must not see:
//!
//! * the **hidden target** ([`HiddenFunction`]): sealed, unreadable from
//!   outside this crate, with a redacted `Debug` so it cannot leak into a log;
//! * the **protocol** ([`Protocol`]): how many probes an episode may spend;
//! * the **verdict** ([`Verdict`]): computed by [`LabWorld::judge`] after an
//!   episode, from the committed answer and the ground truth;
//! * the **promotion criteria** ([`PromotionGate`]): private thresholds a
//!   [`metron_core::capability::CapabilityCandidate`] must meet.
//!
//! The system reaches the laboratory only through the
//! [`metron_core::world::Oracle`] port that [`LabWorld`] implements. Every
//! probe answered through that port issues a resource receipt.
//!
//! The laboratory depends on `metron-core` only. It does no I/O; fixtures
//! and manifests are parsed from strings that the composition root reads.

pub mod fixture;
pub mod hidden;
pub mod manifest;
pub mod promotion;
pub mod truth_table;
pub mod verdict;
pub mod world;

pub use fixture::{BooleanFixture, FixtureError};
pub use hidden::HiddenFunction;
pub use manifest::{LabConfig, Manifest, ManifestError, ScheduleStep, SystemPlan};
pub use promotion::{PromotionCriteria, PromotionGate};
pub use truth_table::{TruthTable, TruthTableError};
pub use verdict::Verdict;
pub use world::{LabWorld, Protocol};

/// The only task family in this slice.
pub const FAMILY_HIDDEN_BOOLEAN: &str = "hidden-boolean-function";

/// The frame an answer must be given in.
pub const ANSWER_FRAME: &str = "truth-table/complete";
