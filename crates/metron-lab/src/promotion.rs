//! Capability promotion.
//!
//! The criteria are private. The system learns only whether a candidate was
//! promoted and, if not, the kind of shortfall, never the thresholds.

use crate::verdict::Verdict;
use metron_core::capability::{CapabilityCandidate, PromotionVerdict};
use metron_core::hash::ContentHash;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fmt::Write as _;

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

/// Aggregate of a set of verdicts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HeldOutSummary {
    /// Episodes.
    pub episodes: usize,
    /// Fraction judged correct.
    pub solved: f64,
    /// Mean probes over solved episodes.
    pub mean_probes_solved: f64,
    /// Mean probes over all episodes.
    pub mean_probes: f64,
}

impl HeldOutSummary {
    /// Summarises verdicts.
    #[must_use]
    pub fn of(verdicts: &[Verdict]) -> Self {
        let n = verdicts.len();
        let solved: Vec<&Verdict> = verdicts.iter().filter(|v| v.correct).collect();
        let mean = |vs: &[&Verdict]| {
            if vs.is_empty() {
                0.0
            } else {
                vs.iter().map(|v| v.probes_answered as f64).sum::<f64>() / vs.len() as f64
            }
        };
        Self {
            episodes: n,
            solved: if n == 0 {
                0.0
            } else {
                solved.len() as f64 / n as f64
            },
            mean_probes_solved: mean(&solved),
            mean_probes: mean(&verdicts.iter().collect::<Vec<_>>()),
        }
    }
}

/// One candidate's judgement.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PromotionCase {
    /// The candidate.
    pub candidate: CapabilityCandidate,
    /// Train episodes that produced this composition and were judged correct.
    pub train_support: usize,
    /// Train episodes run.
    pub train_episodes: usize,
    /// Fraction of held-out episodes of the proposing strategy that executed
    /// this same composition.
    pub reuse_frequency: f64,
    /// The candidate run as a fixed strategy on the held-out split.
    pub held_out: HeldOutSummary,
    /// The proposing strategy on the same held-out tasks.
    pub reference: HeldOutSummary,
    /// The gate's decision.
    pub decision: PromotionVerdict,
}

/// A promotion report for one strategy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PromotionReport {
    /// The proposing strategy.
    pub strategy: String,
    /// Hash of the manifest.
    pub manifest_hash: ContentHash,
    /// Fingerprint of the criteria in force.
    pub gate_fingerprint: ContentHash,
    /// Candidates, most supported first.
    pub cases: Vec<PromotionCase>,
}

/// Renders promotion reports as Markdown.
#[must_use]
pub fn render_promotion_markdown(reports: &[PromotionReport], title: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# {title}
"
    );
    for r in reports {
        let _ = writeln!(
            out,
            "## Strategy `{}`

Gate fingerprint `{}`, manifest `{}`.
",
            r.strategy,
            r.gate_fingerprint.short(),
            r.manifest_hash.short()
        );
        let _ = writeln!(
            out,
            "| Candidate | Train support | Reuse (held-out) | Held-out solved | Probes (solved) | Reference solved | Reference probes (solved) | Decision |"
        );
        let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|---:|---|");
        for c in &r.cases {
            let decision = match &c.decision {
                PromotionVerdict::Promoted => "promoted".to_owned(),
                PromotionVerdict::Rejected { reasons } => {
                    format!("rejected: {}", reasons.join("; "))
                }
            };
            let _ = writeln!(
                out,
                "| `{}` | {}/{} | {:.0}% | {:.0}% | {:.2} | {:.0}% | {:.2} | {decision} |",
                c.candidate.name,
                c.train_support,
                c.train_episodes,
                c.reuse_frequency * 100.0,
                c.held_out.solved * 100.0,
                c.held_out.mean_probes_solved,
                c.reference.solved * 100.0,
                c.reference.mean_probes_solved
            );
        }
        for c in &r.cases {
            if let Some(contract) = &c.candidate.claimed_contract {
                let _ = writeln!(
                    out,
                    "
`{}` claims {contract}",
                    c.candidate.name
                );
                for a in &contract.assumes {
                    let _ = writeln!(out, "- assumes {a}");
                }
            }
        }
        out.push('\n');
    }
    out
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
            target_family: None,
            target_description: None,
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
            program: serde_json::Value::Null,
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
