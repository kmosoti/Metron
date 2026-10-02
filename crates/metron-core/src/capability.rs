//! Capability candidates.
//!
//! The system may propose that a composition of operators is worth keeping
//! as a reusable capability. It can only *propose*: the laboratory judges
//! candidates against held-out hidden targets under criteria the system
//! cannot read. This module holds the proposal and the verdict shape; the
//! criteria live in `metron-lab`.

use crate::evidence::{Evidence, independent_observations};
use crate::frame::TransformContract;
use crate::id::{EpisodeId, OperatorId};
use serde::{Deserialize, Serialize};

/// A proposed reusable composition of operators.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapabilityCandidate {
    /// Proposed name.
    pub name: String,
    /// What it is for.
    pub description: String,
    /// Operators, in application order.
    pub composition: Vec<OperatorId>,
    /// The contract the composition is claimed to satisfy end to end.
    pub claimed_contract: Option<TransformContract>,
    /// Evidence from the episodes in which it was used.
    #[serde(default)]
    pub evidence: Vec<Evidence>,
    /// Episode in which it was proposed.
    pub proposed_in: EpisodeId,
}

impl CapabilityCandidate {
    /// Independent observations cited by the candidate's evidence.
    #[must_use]
    pub fn independent_support(&self) -> usize {
        independent_observations(&self.evidence)
    }
}

/// The laboratory's decision on a candidate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum PromotionVerdict {
    /// Accepted as a capability.
    Promoted,
    /// Rejected, with reasons the system may read.
    Rejected {
        /// Why.
        reasons: Vec<String>,
    },
}

impl PromotionVerdict {
    /// Whether the candidate was promoted.
    #[must_use]
    pub fn is_promoted(&self) -> bool {
        matches!(self, PromotionVerdict::Promoted)
    }
}
