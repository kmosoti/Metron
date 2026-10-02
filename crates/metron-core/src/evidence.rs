//! Observations and evidence.

use crate::id::{ObservationId, OperatorId, ReceiptId};
use crate::inquiry::Representation;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A fact obtained from outside the system through the oracle port.
///
/// Observations are created only by oracles, never by operators, and every
/// observation names the receipt that paid for it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    /// Identifier, unique within an episode.
    pub id: ObservationId,
    /// The receipt for the probe that produced it.
    pub receipt: ReceiptId,
    /// What was asked.
    pub probe: Representation,
    /// What came back.
    pub result: Representation,
}

/// A link from a claim, view or answer back to an observation.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Evidence {
    /// The observation this evidence rests on.
    pub observation: ObservationId,
    /// Operators the observation passed through on the way, in order. Empty
    /// means the observation is cited directly.
    #[serde(default)]
    pub via: Vec<OperatorId>,
}

impl Evidence {
    /// Evidence citing an observation directly.
    #[must_use]
    pub fn direct(observation: ObservationId) -> Self {
        Self {
            observation,
            via: Vec::new(),
        }
    }

    /// Evidence derived from an observation through `via`.
    #[must_use]
    pub fn derived(observation: ObservationId, via: Vec<OperatorId>) -> Self {
        Self { observation, via }
    }
}

/// Number of distinct observations among `evidence`.
///
/// This is the provenance rule: redescriptions of one observation are one
/// piece of evidence.
#[must_use]
pub fn independent_observations(evidence: &[Evidence]) -> usize {
    evidence
        .iter()
        .map(|e| e.observation)
        .collect::<BTreeSet<_>>()
        .len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redescriptions_are_not_independent() {
        let e = vec![
            Evidence::direct(ObservationId(1)),
            Evidence::derived(ObservationId(1), vec![OperatorId::from("to-anf")]),
            Evidence::derived(ObservationId(1), vec![OperatorId::from("to-dnf")]),
            Evidence::direct(ObservationId(2)),
        ];
        assert_eq!(e.len(), 4);
        assert_eq!(independent_observations(&e), 2);
        assert_eq!(independent_observations(&[]), 0);
    }
}
