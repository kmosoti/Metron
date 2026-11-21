//! Error types for the Metron two-lane sandbox.

use thiserror::Error;

/// Failure returned by sandbox setup, policy checks, or execution cleanup.
#[derive(Debug, Error)]
pub enum SandboxError {
    /// Deterministic denial before or during sandbox setup (effect did not run or was refused).
    #[error("sandbox denied: {0}")]
    Denied(String),

    /// Host kernel cannot strictly enforce the required Landlock ABI or filesystem rules.
    #[error("landlock insufficient: {0}")]
    LandlockInsufficient(String),

    /// Scratch filesystem lacks the required free space reserve (never disguised as timeout).
    #[error(
        "insufficient scratch space: available {available_bytes}B < required {required_bytes}B"
    )]
    InsufficientScratchSpace {
        /// Currently available bytes on the scratch filesystem.
        available_bytes: u64,
        /// Minimum reserve bytes demanded by the sandbox policy.
        required_bytes: u64,
    },

    /// Post-dispatch failure where process-tree cleanup could not be established.
    #[error("sandbox outcome uncertain: {0}")]
    Uncertain(String),
}

impl SandboxError {
    /// Returns `true` when post-dispatch cleanup/state could not be proven.
    #[must_use]
    pub fn is_uncertain(&self) -> bool {
        matches!(self, Self::Uncertain(_))
    }
}
