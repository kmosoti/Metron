//! Checkpoints for suspended episodes.
//!
//! When a consult operator finds its request unanswered, the runner stops
//! without ending the episode. Everything the runner owns is captured here;
//! the composition root adds the scheduler's and the world's own public
//! state, persists the checkpoint, and later resumes from it.

use crate::id::{InquiryId, OperatorId};
use crate::inquiry::Inquiry;
use crate::journal::Episode;
use crate::rng::Rng;
use serde::{Deserialize, Serialize};

/// The request an episode is waiting on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingRequest {
    /// Identifier the service gave the request.
    pub request_id: String,
    /// The service.
    pub service: String,
    /// The operator to retry once the request is answered.
    pub operator: OperatorId,
}

/// A suspended episode.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EpisodeCheckpoint {
    /// The journal so far.
    pub episode: Episode,
    /// The inquiry as it was before the pending operator ran.
    pub inquiry: Inquiry,
    /// What the episode is waiting on.
    pub pending: PendingRequest,
    /// Scheduler ticks so far.
    pub ticks: u32,
    /// Consecutive unchanged applications so far.
    pub unchanged_streak: u32,
    /// The episode's random stream, positioned after the last completed
    /// application.
    pub rng: Rng,
    /// Scheduler state, as the scheduler serialised it.
    #[serde(default)]
    pub scheduler_state: serde_json::Value,
    /// Public world state, as the composition root serialised it.
    #[serde(default)]
    pub world_state: serde_json::Value,
}

impl EpisodeCheckpoint {
    /// The inquiry this checkpoint belongs to.
    #[must_use]
    pub fn inquiry_id(&self) -> InquiryId {
        self.inquiry.id
    }
}
