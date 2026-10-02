//! Metron core: the pure domain of the cognitive kernel.
//!
//! This crate is the hexagonal *inside*. It defines the research objects and
//! the ports through which they touch the world, and nothing else. It does no
//! I/O, knows no file formats, talks to no model, and never sees a hidden
//! target. Its only dependencies are `serde`, `serde_json`, `sha2` and
//! `thiserror`; `tests/architecture` fails the build if that changes.
//!
//! The central objects, in the order one meets them during an episode:
//!
//! * [`inquiry::Inquiry`] — the system's working state about one
//!   [`inquiry::Question`]: its [`inquiry::View`]s, its observations, and
//!   eventually its [`inquiry::Answer`].
//! * [`evidence::Observation`] — a fact obtained from outside through the
//!   [`world::Oracle`] port, always paired with a [`receipt::ResourceReceipt`].
//! * [`evidence::Evidence`] — a link from a claim back to an observation,
//!   with the operators it passed through. Independence is counted in
//!   observations, never in evidence records.
//! * [`frame::Frame`] and [`frame::TransformContract`] — representation
//!   systems and the explicit, validated contracts that connect them.
//! * [`operator::Operator`] — a capability of kind observe, transform or
//!   commit, generic over the world `W` it needs.
//! * [`journal::Episode`] — the hash-chained, replayable record of a run.
//! * [`capability::CapabilityCandidate`] — a proposed reusable composition,
//!   judged by the laboratory and never by the system itself.

pub mod bits;
pub mod capability;
pub mod clock;
pub mod cost;
pub mod evidence;
pub mod frame;
pub mod hash;
pub mod id;
pub mod inquiry;
pub mod journal;
pub mod operator;
pub mod prelude;
pub mod receipt;
pub mod rng;
pub mod world;

/// Version string recorded in receipts and journals.
///
/// Set `METRON_GIT_SHA` at build time to include the commit hash.
#[must_use]
pub fn code_version() -> String {
    match option_env!("METRON_GIT_SHA") {
        Some(sha) => format!("metron-core/{}+{}", env!("CARGO_PKG_VERSION"), sha),
        None => format!("metron-core/{}", env!("CARGO_PKG_VERSION")),
    }
}
