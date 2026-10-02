//! Verdicts.

use metron_core::cost::Cost;
use metron_core::hash::ContentHash;
use serde::{Deserialize, Serialize};

/// The laboratory's judgement of one episode.
///
/// A verdict is produced after the episode ends and is never shown to the
/// system during it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    /// Whether an answer was committed.
    pub answered: bool,
    /// Whether the answer was in the required frame and shape.
    pub frame_accepted: bool,
    /// Whether the answer equals the hidden target on every row.
    pub correct: bool,
    /// Rows in the target.
    pub rows_total: usize,
    /// Rows the answer got right (0 if no acceptable answer).
    pub rows_correct: usize,
    /// `rows_correct / rows_total`.
    pub agreement: f64,
    /// Probes the oracle answered during the episode.
    pub probes_answered: u64,
    /// Resources the inquiry spent.
    pub cost: Cost,
    /// Fingerprint of the hidden target.
    pub target_fingerprint: ContentHash,
    /// Family of the target, revealed after the episode.
    #[serde(default)]
    pub target_family: Option<String>,
    /// Description of the target, revealed after the episode.
    #[serde(default)]
    pub target_description: Option<String>,
    /// Why the verdict is what it is.
    #[serde(default)]
    pub reasons: Vec<String>,
}
