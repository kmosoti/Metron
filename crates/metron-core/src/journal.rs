//! The episode journal: a hash-chained, replayable record of a run.

use crate::cost::{BudgetDimension, Cost};
use crate::frame::TransformContract;
use crate::hash::ContentHash;
use crate::id::{EpisodeId, FrameId, InquiryId, ObservationId, OperatorId, ViewId};
use crate::inquiry::Question;
use crate::operator::{OperatorKind, StepStatus};
use crate::receipt::ResourceReceipt;
use crate::world::ServiceAnswer;
use serde::{Deserialize, Serialize};

/// Why an episode ended.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum StopReason {
    /// An answer was committed.
    Answered,
    /// The schedule ran out of operators.
    ScheduleExhausted,
    /// The step limit was reached.
    MaxSteps,
    /// Too many consecutive steps changed nothing.
    Stalled,
    /// The budget was exceeded.
    BudgetExceeded {
        /// Which dimension.
        dimension: BudgetDimension,
    },
    /// An operator returned an error.
    OperatorFailed {
        /// Which operator.
        operator: OperatorId,
        /// Its message.
        message: String,
    },
    /// An operator wrote outside what its kind and contract allow.
    ContractViolation {
        /// Which operator.
        operator: OperatorId,
        /// Which view.
        view: ViewId,
        /// What went wrong.
        message: String,
    },
}

impl StopReason {
    /// Whether the episode ended with an answer.
    #[must_use]
    pub fn is_answered(&self) -> bool {
        matches!(self, StopReason::Answered)
    }
}

/// Something that happened during an episode.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum JournalEvent {
    /// The episode began.
    EpisodeStarted {
        /// The episode.
        episode: EpisodeId,
        /// The inquiry.
        inquiry: InquiryId,
        /// The question as the system sees it.
        question: Question,
        /// Hash of the manifest that configured the run.
        manifest_hash: ContentHash,
        /// Seed of the episode's random stream.
        seed: u64,
        /// Code version.
        code_version: String,
    },
    /// An operator was applied.
    OperatorApplied {
        /// Step index.
        step: u32,
        /// The operator.
        operator: OperatorId,
        /// Its kind.
        kind: OperatorKind,
        /// Reported status.
        status: StepStatus,
        /// Cost charged for the step (call, work and receipts).
        cost: Cost,
        /// State hash before.
        input_hash: ContentHash,
        /// State hash after.
        output_hash: ContentHash,
        /// Operator note.
        note: String,
    },
    /// A scheduled operator was not applicable and was skipped.
    OperatorSkipped {
        /// Step index.
        step: u32,
        /// The operator.
        operator: OperatorId,
    },
    /// A receipt was issued by a port.
    Receipt {
        /// Step during which it was issued.
        step: u32,
        /// The receipt.
        receipt: ResourceReceipt,
    },
    /// An external service answered. The full content is kept so the
    /// episode can be replayed without the service.
    ServiceAnswered {
        /// Step during which it was answered.
        step: u32,
        /// The answer.
        answer: ServiceAnswer,
    },
    /// The episode suspended waiting for an external service.
    Suspended {
        /// Step at which it suspended.
        step: u32,
        /// The operator that will be retried.
        operator: OperatorId,
        /// The service.
        service: String,
        /// The pending request.
        request_id: String,
    },
    /// The episode resumed.
    Resumed {
        /// Step at which it resumed.
        step: u32,
        /// The request that was answered.
        request_id: String,
    },
    /// A view was written.
    ViewWritten {
        /// Step index.
        step: u32,
        /// The view.
        view: ViewId,
        /// Its frame.
        frame: FrameId,
        /// Its new version.
        version: u32,
        /// The contract it was written under, if any.
        contract: Option<TransformContract>,
        /// Observations it rests on, transitively.
        observations: Vec<ObservationId>,
        /// Hash of its content.
        content_hash: ContentHash,
    },
    /// An answer was committed.
    Answered {
        /// Step index.
        step: u32,
        /// The committing operator.
        operator: OperatorId,
        /// Frame of the answer.
        frame: FrameId,
        /// Hash of the answer content.
        content_hash: ContentHash,
        /// Stated confidence.
        confidence: f64,
        /// Independent observations cited.
        independent_observations: usize,
    },
    /// The episode ended.
    EpisodeEnded {
        /// Why.
        stop: StopReason,
        /// Steps taken.
        steps: u32,
        /// Total cost.
        cost: Cost,
        /// Hash of the answer, if any.
        answer_hash: Option<ContentHash>,
    },
}

/// A sealed journal entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Zero-based sequence number.
    pub seq: u64,
    /// Wall-clock nanoseconds when recorded. Informational; not hashed.
    #[serde(default)]
    pub wall_nanos: u64,
    /// The event.
    pub event: JournalEvent,
    /// Hash of the previous entry, or genesis.
    pub prev_hash: ContentHash,
    /// Hash of `seq`, `event` and `prev_hash`.
    pub hash: ContentHash,
}

/// Journal errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JournalError {
    /// The chain does not verify.
    #[error("journal corrupt at seq {seq}: {reason}")]
    Corrupt {
        /// Offending entry.
        seq: u64,
        /// What is wrong.
        reason: String,
    },
}

impl JournalEvent {
    /// The event with every wall-clock reading removed.
    ///
    /// Receipts carry an informational `wall_nanos`; two replays of the same
    /// episode must hash identically, so the chain covers this projection
    /// rather than the raw event.
    #[must_use]
    pub fn replay_projection(&self) -> JournalEvent {
        match self {
            JournalEvent::Receipt { step, receipt } => {
                let mut receipt = receipt.clone();
                receipt.wall_nanos = 0;
                JournalEvent::Receipt {
                    step: *step,
                    receipt,
                }
            }
            other => other.clone(),
        }
    }
}

impl JournalEntry {
    fn digest(seq: u64, event: &JournalEvent, prev_hash: &ContentHash) -> ContentHash {
        #[derive(Serialize)]
        struct Body {
            seq: u64,
            event: JournalEvent,
        }
        let bytes = serde_json::to_vec(&Body {
            seq,
            event: event.replay_projection(),
        })
        .expect("journal events are always serialisable");
        ContentHash::chain(prev_hash, &bytes)
    }

    /// Seals an event onto the chain ending in `prev_hash`.
    #[must_use]
    pub fn seal(seq: u64, wall_nanos: u64, event: JournalEvent, prev_hash: ContentHash) -> Self {
        let hash = Self::digest(seq, &event, &prev_hash);
        Self {
            seq,
            wall_nanos,
            event,
            prev_hash,
            hash,
        }
    }

    /// Whether the hash matches the contents.
    #[must_use]
    pub fn verify(&self) -> bool {
        self.hash == Self::digest(self.seq, &self.event, &self.prev_hash)
    }

    /// Verifies a whole chain.
    pub fn verify_chain(entries: &[JournalEntry]) -> Result<(), JournalError> {
        let mut prev = ContentHash::GENESIS;
        for (i, e) in entries.iter().enumerate() {
            if e.seq != i as u64 {
                return Err(JournalError::Corrupt {
                    seq: e.seq,
                    reason: format!("expected seq {i}"),
                });
            }
            if e.prev_hash != prev {
                return Err(JournalError::Corrupt {
                    seq: e.seq,
                    reason: "broken link to previous entry".into(),
                });
            }
            if !e.verify() {
                return Err(JournalError::Corrupt {
                    seq: e.seq,
                    reason: "hash mismatch".into(),
                });
            }
            if let JournalEvent::Receipt { receipt, .. } = &e.event
                && !receipt.verify()
            {
                return Err(JournalError::Corrupt {
                    seq: e.seq,
                    reason: format!("receipt {} does not verify", receipt.id),
                });
            }
            prev = e.hash;
        }
        Ok(())
    }
}

/// How an episode ended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EpisodeOutcome {
    /// Why.
    pub stop: StopReason,
    /// Steps taken.
    pub steps: u32,
    /// Cost of the episode.
    pub cost: Cost,
    /// Whether an answer was committed.
    pub answered: bool,
}

/// One bounded run of the system on an inquiry, with its full journal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Episode {
    /// Identifier.
    pub id: EpisodeId,
    /// The inquiry worked on.
    pub inquiry: InquiryId,
    /// Seed of the episode's random stream.
    pub seed: u64,
    /// Hash of the manifest that configured the run.
    pub manifest_hash: ContentHash,
    /// The journal, oldest first.
    pub entries: Vec<JournalEntry>,
    /// Outcome, once ended.
    pub outcome: Option<EpisodeOutcome>,
}

impl Episode {
    /// Creates an empty episode.
    #[must_use]
    pub fn new(id: EpisodeId, inquiry: InquiryId, seed: u64, manifest_hash: ContentHash) -> Self {
        Self {
            id,
            inquiry,
            seed,
            manifest_hash,
            entries: Vec::new(),
            outcome: None,
        }
    }

    /// Hash of the last entry, or genesis.
    #[must_use]
    pub fn head_hash(&self) -> ContentHash {
        self.entries.last().map_or(ContentHash::GENESIS, |e| e.hash)
    }

    /// Seals and appends an event.
    pub fn append(&mut self, wall_nanos: u64, event: JournalEvent) -> &JournalEntry {
        let entry = JournalEntry::seal(
            self.entries.len() as u64,
            wall_nanos,
            event,
            self.head_hash(),
        );
        self.entries.push(entry);
        self.entries.last().expect("just pushed")
    }

    /// Verifies the chain and every receipt in it.
    pub fn verify(&self) -> Result<(), JournalError> {
        JournalEntry::verify_chain(&self.entries)
    }

    /// All receipts in the journal, in order.
    pub fn receipts(&self) -> impl Iterator<Item = &ResourceReceipt> {
        self.entries.iter().filter_map(|e| match &e.event {
            JournalEvent::Receipt { receipt, .. } => Some(receipt),
            _ => None,
        })
    }

    /// All service answers in the journal, in order.
    pub fn service_answers(&self) -> impl Iterator<Item = &ServiceAnswer> {
        self.entries.iter().filter_map(|e| match &e.event {
            JournalEvent::ServiceAnswered { answer, .. } => Some(answer),
            _ => None,
        })
    }

    /// The events that describe what the system did, in order, leaving out
    /// suspension bookkeeping and wall-clock readings. Two runs of the same
    /// episode, one of which waited for a service and one of which replayed
    /// the recorded answers, have equal effective events.
    #[must_use]
    pub fn effective_events(&self) -> Vec<JournalEvent> {
        self.entries
            .iter()
            .filter(|e| {
                !matches!(
                    e.event,
                    JournalEvent::Suspended { .. } | JournalEvent::Resumed { .. }
                )
            })
            .map(|e| e.event.replay_projection())
            .collect()
    }

    /// Total cost charged across receipts.
    #[must_use]
    pub fn receipt_cost(&self) -> Cost {
        self.receipts().fold(Cost::ZERO, |acc, r| acc + r.cost)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_verifies_and_detects_tampering() {
        let mut ep = Episode::new(EpisodeId(1), InquiryId(1), 7, ContentHash::GENESIS);
        ep.append(
            1,
            JournalEvent::OperatorSkipped {
                step: 0,
                operator: OperatorId::from("a"),
            },
        );
        ep.append(
            2,
            JournalEvent::OperatorSkipped {
                step: 1,
                operator: OperatorId::from("b"),
            },
        );
        ep.verify().unwrap();
        let head = ep.head_hash();
        let mut tampered = ep.clone();
        tampered.entries[0].event = JournalEvent::OperatorSkipped {
            step: 0,
            operator: OperatorId::from("z"),
        };
        assert!(tampered.verify().is_err());
        // Wall time is not part of the chain.
        let mut retimed = ep.clone();
        retimed.entries[1].wall_nanos = 999;
        retimed.verify().unwrap();
        assert_eq!(retimed.head_hash(), head);
    }

    #[test]
    fn receipt_wall_time_does_not_enter_the_chain() {
        use crate::cost::Cost;
        use crate::id::ReceiptId;
        use crate::receipt::{OperationKind, ResourceReceipt};
        let receipt = |wall| {
            ResourceReceipt::seal(
                ReceiptId(1),
                OperationKind::OracleProbe,
                OperatorId::from("probe"),
                InquiryId(1),
                ContentHash::GENESIS,
                ContentHash::GENESIS,
                Cost::probes(1),
                vec![],
                wall,
            )
        };
        let mut a = Episode::new(EpisodeId(1), InquiryId(1), 0, ContentHash::GENESIS);
        a.append(
            0,
            JournalEvent::Receipt {
                step: 0,
                receipt: receipt(5),
            },
        );
        let mut b = Episode::new(EpisodeId(1), InquiryId(1), 0, ContentHash::GENESIS);
        b.append(
            0,
            JournalEvent::Receipt {
                step: 0,
                receipt: receipt(500),
            },
        );
        assert_eq!(a.head_hash(), b.head_hash());
        a.verify().unwrap();
        let mut tampered = a.clone();
        if let JournalEvent::Receipt { receipt, .. } = &mut tampered.entries[0].event {
            receipt.cost = Cost::probes(2);
        }
        assert!(
            tampered.verify().is_err(),
            "a receipt edit must break the chain"
        );
    }
}
