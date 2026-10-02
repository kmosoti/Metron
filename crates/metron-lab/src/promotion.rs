//! Capability promotion.
//!
//! The criteria are private. The system learns only whether a candidate was
//! promoted and, if not, the kind of shortfall, never the thresholds.

use crate::verdict::Verdict;
use metron_core::capability::{CapabilityCandidate, PromotionVerdict};
use metron_core::hash::ContentHash;
use serde::Serialize;
use std::fmt;

/// Thresholds a candidate must meet.
#[derive(Clone, PartialEq, Serialize)]
pub struct PromotionCriteria {
    min_independent_observations: usize,
    min_held_out_cases: usize,
    min_held_out_pass_rate: f64,
}

impl PromotionCriteria {
    /// The first, deliberately conservative, criteria set.
    #[must_use]
    pub const fn v0() -> Self {
        Self {
            min_independent_observations: 2,
            min_held_out_cases: 3,
            min_held_out_pass_rate: 1.0,
        }
    }

    /// Hash of the criteria, so results can state which criteria judged them
    /// without stating the values.
    #[must_use]
    pub fn fingerprint(&self) -> ContentHash {
        ContentHash::of_json(self).expect("criteria are always serialisable")
    }
}

impl fmt::Debug for PromotionCriteria {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PromotionCriteria {{ <redacted>, fingerprint: {} }}",
            self.fingerprint().short()
        )
    }
}

/// Judges capability candidates.
#[derive(Clone, Debug, PartialEq)]
pub struct PromotionGate {
    criteria: PromotionCriteria,
}

impl PromotionGate {
    /// A gate with the given criteria.
    #[must_use]
    pub const fn new(criteria: PromotionCriteria) -> Self {
        Self { criteria }
    }

    /// A gate with the v0 criteria.
    #[must_use]
    pub const fn v0() -> Self {
        Self::new(PromotionCriteria::v0())
    }

    /// Fingerprint of the criteria in force.
    #[must_use]
    pub fn fingerprint(&self) -> ContentHash {
        self.criteria.fingerprint()
    }

    /// Judges `candidate` given the verdicts of held-out episodes that used it.
    #[must_use]
    pub fn judge(&self, candidate: &CapabilityCandidate, held_out: &[Verdict]) -> PromotionVerdict {
        let mut reasons = Vec::new();
        if candidate.composition.is_empty() {
            reasons.push("composition is empty".to_owned());
        }
        if candidate.independent_support() < self.criteria.min_independent_observations {
            reasons.push("insufficient independent observations".to_owned());
        }
        if held_out.len() < self.criteria.min_held_out_cases {
            reasons.push("insufficient held-out cases".to_owned());
        } else {
            let passed = held_out.iter().filter(|v| v.correct).count();
            let rate = passed as f64 / held_out.len() as f64;
            if rate < self.criteria.min_held_out_pass_rate {
                reasons.push("held-out pass rate below threshold".to_owned());
            }
        }
        if reasons.is_empty() {
            PromotionVerdict::Promoted
        } else {
            PromotionVerdict::Rejected { reasons }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metron_core::cost::Cost;
    use metron_core::evidence::Evidence;
    use metron_core::id::{EpisodeId, ObservationId, OperatorId};

    fn verdict(correct: bool) -> Verdict {
        Verdict {
            answered: true,
            frame_accepted: true,
            correct,
            rows_total: 4,
            rows_correct: if correct { 4 } else { 3 },
            agreement: if correct { 1.0 } else { 0.75 },
            probes_answered: 4,
            cost: Cost::ZERO,
            target_fingerprint: ContentHash::GENESIS,
            reasons: Vec::new(),
        }
    }

    fn candidate(observations: &[u64]) -> CapabilityCandidate {
        CapabilityCandidate {
            name: "probe-then-commit".into(),
            description: "demo".into(),
            composition: vec![OperatorId::from("a"), OperatorId::from("b")],
            claimed_contract: None,
            evidence: observations
                .iter()
                .map(|&o| Evidence::direct(ObservationId(o)))
                .collect(),
            proposed_in: EpisodeId(1),
        }
    }

    #[test]
    fn gate_reports_shortfalls_without_thresholds() {
        let gate = PromotionGate::v0();
        let v = gate.judge(&candidate(&[1, 1]), &[verdict(true)]);
        let PromotionVerdict::Rejected { reasons } = v else {
            panic!("expected rejection");
        };
        assert_eq!(reasons.len(), 2);
        assert!(reasons.iter().all(|r| !r.contains('2') && !r.contains('3')));
        let ok = gate.judge(
            &candidate(&[1, 2]),
            &[verdict(true), verdict(true), verdict(true)],
        );
        assert!(ok.is_promoted());
        let flaky = gate.judge(
            &candidate(&[1, 2]),
            &[verdict(true), verdict(false), verdict(true)],
        );
        assert!(!flaky.is_promoted());
        assert!(format!("{:?}", gate).contains("<redacted>"));
    }
}
