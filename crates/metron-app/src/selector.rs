//! Hand-authored strategy selection: the first control for routing.
//!
//! [`SurvivorCountSelector`] is a [`Scheduler`] that routes between the
//! family-restricted identification strategies the laboratory compares. It
//! spends a short probing prefix, then filters the version space of each
//! family in turn and reads how many members survive, then commits to one
//! family by a [`SurvivorRule`] and runs its restricted pipeline. If every
//! family is empty it falls back to the unrestricted pipeline.
//!
//! Under a uniform prior over the pool, the posterior probability that the
//! target belongs to a family is proportional to that family's surviving
//! members, so [`SurvivorRule::Most`] is the Bayes rule. [`SurvivorRule::Fewest`]
//! is kept as the negative control: it prefers a family that a few probes
//! have almost eliminated, which is almost always the wrong one.
//!
//! The selector pays for its features: the per-family filters cost work
//! units, and a wrong family (a family other than the target's that still
//! has a survivor) ends in a confidently wrong answer, which the cost model
//! penalises. That is the trade-off ADR 0010 says a router must face.
//!
//! The operator and view names are a [`Vocabulary`]; the composition root
//! supplies the identifiers of the operators it registered.

use crate::schedule::Scheduler;
use metron_core::id::{OperatorId, ViewId};
use metron_core::inquiry::Inquiry;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

/// Operator and view identifiers the selector schedules and reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vocabulary {
    /// The unrestricted version-space filter.
    pub filter: OperatorId,
    /// Prefix of the family-restricted filters; the label follows a colon.
    pub restricted_filter_prefix: String,
    /// The probe that splits the version space.
    pub probe: OperatorId,
    /// The transform that turns a single survivor into the answer table.
    pub survivor_to_table: OperatorId,
    /// The commit operator.
    pub commit: OperatorId,
    /// The view holding the version-space mask.
    pub version_space_view: ViewId,
}

impl Vocabulary {
    /// The restricted filter for `family`.
    #[must_use]
    pub fn restricted_filter(&self, family: &str) -> OperatorId {
        OperatorId::new(format!("{}{family}", self.restricted_filter_prefix))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
enum Phase {
    /// Spending the probing prefix.
    Prefix,
    /// About to filter family `next`.
    Measure { next: usize },
    /// Filtered family `family`; its survivor count is read on the next call.
    Measuring { family: usize },
    /// Running the chosen pipeline.
    Committed,
    /// Nothing left.
    Done,
}

/// Which family to commit to, given survivor counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SurvivorRule {
    /// The family with the most survivors: the maximum-a-posteriori family
    /// under a uniform prior over the pool.
    Most,
    /// The family with the fewest survivors (at least one): a negative
    /// control.
    Fewest,
}

impl SurvivorRule {
    /// Label for names and reports.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            SurvivorRule::Most => "most",
            SurvivorRule::Fewest => "fewest",
        }
    }
}

/// Routes to a family by its surviving hypotheses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurvivorCountSelector {
    vocabulary: Vocabulary,
    families: Vec<String>,
    rule: SurvivorRule,
    prefix_probes: u32,
    rounds: u32,
    phase: Phase,
    queue: VecDeque<OperatorId>,
    measured: BTreeMap<String, usize>,
    chosen: Option<String>,
}

impl SurvivorCountSelector {
    /// Creates a selector over `families` using `rule`, spending
    /// `prefix_probes` greedy probes before measuring and up to `rounds`
    /// probes after choosing.
    #[must_use]
    pub fn new(
        vocabulary: Vocabulary,
        families: Vec<String>,
        rule: SurvivorRule,
        prefix_probes: u32,
        rounds: u32,
    ) -> Self {
        let mut queue = VecDeque::new();
        queue.push_back(vocabulary.filter.clone());
        for _ in 0..prefix_probes {
            queue.push_back(vocabulary.probe.clone());
            queue.push_back(vocabulary.filter.clone());
        }
        Self {
            vocabulary,
            families,
            rule,
            prefix_probes,
            rounds,
            phase: Phase::Prefix,
            queue,
            measured: BTreeMap::new(),
            chosen: None,
        }
    }

    /// Survivors per family, once measured.
    #[must_use]
    pub fn measured(&self) -> &BTreeMap<String, usize> {
        &self.measured
    }

    /// The family committed to, once chosen; `None` before the choice or
    /// when the selector fell back to the unrestricted pipeline.
    #[must_use]
    pub fn chosen(&self) -> Option<&str> {
        self.chosen.as_deref()
    }

    /// The prefix length.
    #[must_use]
    pub const fn prefix_probes(&self) -> u32 {
        self.prefix_probes
    }

    /// The rule.
    #[must_use]
    pub const fn rule(&self) -> SurvivorRule {
        self.rule
    }

    fn survivors(&self, inquiry: &Inquiry) -> usize {
        inquiry
            .view(&self.vocabulary.version_space_view)
            .and_then(|v| v.content.as_bits())
            .map_or(0, |b| b.count_ones())
    }

    fn choose(&mut self) {
        let candidates = self
            .families
            .iter()
            .filter(|f| self.measured.get(*f).is_some_and(|&n| n >= 1));
        let best = match self.rule {
            SurvivorRule::Most => candidates.max_by_key(|f| self.measured[*f]),
            SurvivorRule::Fewest => candidates.min_by_key(|f| self.measured[*f]),
        }
        .cloned();
        let filter = match &best {
            Some(f) => self.vocabulary.restricted_filter(f),
            None => self.vocabulary.filter.clone(),
        };
        self.chosen = best;
        self.queue.clear();
        self.queue.push_back(filter.clone());
        for _ in 0..self.rounds {
            self.queue.push_back(self.vocabulary.probe.clone());
            self.queue.push_back(filter.clone());
        }
        self.queue
            .push_back(self.vocabulary.survivor_to_table.clone());
        self.queue.push_back(self.vocabulary.commit.clone());
        self.phase = Phase::Committed;
    }
}

impl Scheduler for SurvivorCountSelector {
    fn name(&self) -> String {
        format!(
            "survivor-count({}, prefix={})",
            self.rule.label(),
            self.prefix_probes
        )
    }

    fn next(&mut self, inquiry: &Inquiry) -> Option<OperatorId> {
        loop {
            match self.phase.clone() {
                Phase::Prefix => {
                    if let Some(op) = self.queue.pop_front() {
                        return Some(op);
                    }
                    self.phase = Phase::Measure { next: 0 };
                }
                Phase::Measure { next } => {
                    if next >= self.families.len() {
                        self.choose();
                        continue;
                    }
                    self.phase = Phase::Measuring { family: next };
                    return Some(self.vocabulary.restricted_filter(&self.families[next]));
                }
                Phase::Measuring { family } => {
                    let count = self.survivors(inquiry);
                    self.measured.insert(self.families[family].clone(), count);
                    self.phase = Phase::Measure { next: family + 1 };
                }
                Phase::Committed => {
                    if let Some(op) = self.queue.pop_front() {
                        return Some(op);
                    }
                    self.phase = Phase::Done;
                }
                Phase::Done => return None,
            }
        }
    }

    fn state(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("selectors are always serialisable")
    }

    fn restore(&mut self, state: &serde_json::Value) -> Result<(), String> {
        let restored: SurvivorCountSelector =
            serde_json::from_value(state.clone()).map_err(|e| e.to_string())?;
        if restored.vocabulary != self.vocabulary
            || restored.families != self.families
            || restored.rule != self.rule
        {
            return Err("checkpointed selector does not match this selector".into());
        }
        *self = restored;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metron_core::bits::BitVector;
    use metron_core::id::{FrameId, InquiryId};
    use metron_core::inquiry::{Derivation, Question, Representation};

    fn vocab() -> Vocabulary {
        Vocabulary {
            filter: OperatorId::from("filter"),
            restricted_filter_prefix: "filter:".into(),
            probe: OperatorId::from("probe"),
            survivor_to_table: OperatorId::from("survivor"),
            commit: OperatorId::from("commit"),
            version_space_view: ViewId::from("vs"),
        }
    }

    fn inquiry() -> Inquiry {
        Inquiry::new(
            InquiryId(1),
            Question {
                kind: "k".into(),
                statement: "s".into(),
                params: serde_json::Value::Null,
                answer_frame: FrameId::from("f"),
            },
        )
    }

    fn set_survivors(inquiry: &mut Inquiry, n: usize) {
        inquiry.write_view(
            "vs",
            "frame",
            Representation::Bits(BitVector::from_fn(10, |i| i < n)),
            Derivation::by(OperatorId::from("filter"), 0),
        );
    }

    #[test]
    fn measures_each_family_and_commits_by_rule() {
        let families = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        let mut most =
            SurvivorCountSelector::new(vocab(), families.clone(), SurvivorRule::Most, 1, 2);
        let mut q = inquiry();
        let ids = |s: &mut SurvivorCountSelector, q: &Inquiry, n: usize| -> Vec<String> {
            (0..n).map(|_| s.next(q).unwrap().to_string()).collect()
        };
        assert_eq!(ids(&mut most, &q, 3), vec!["filter", "probe", "filter"]);
        assert_eq!(most.next(&q).unwrap().as_str(), "filter:a");
        set_survivors(&mut q, 0);
        assert_eq!(most.next(&q).unwrap().as_str(), "filter:b");
        set_survivors(&mut q, 4);
        assert_eq!(most.next(&q).unwrap().as_str(), "filter:c");
        set_survivors(&mut q, 2);
        // b has the most survivors.
        assert_eq!(
            ids(&mut most, &q, 7),
            vec![
                "filter:b", "probe", "filter:b", "probe", "filter:b", "survivor", "commit"
            ]
        );
        assert_eq!(most.chosen(), Some("b"));
        assert_eq!(most.measured()["a"], 0);
        assert_eq!(most.next(&q), None);

        // The negative control picks c, the fewest non-zero.
        let mut fewest = SurvivorCountSelector::new(vocab(), families, SurvivorRule::Fewest, 0, 1);
        let mut q = inquiry();
        assert_eq!(fewest.next(&q).unwrap().as_str(), "filter");
        assert_eq!(fewest.next(&q).unwrap().as_str(), "filter:a");
        set_survivors(&mut q, 0);
        assert_eq!(fewest.next(&q).unwrap().as_str(), "filter:b");
        set_survivors(&mut q, 4);
        assert_eq!(fewest.next(&q).unwrap().as_str(), "filter:c");
        set_survivors(&mut q, 2);
        assert_eq!(fewest.next(&q).unwrap().as_str(), "filter:c");
        assert_eq!(fewest.chosen(), Some("c"));
    }

    #[test]
    fn falls_back_when_every_family_is_empty_and_state_round_trips() {
        let mut s =
            SurvivorCountSelector::new(vocab(), vec!["a".to_owned()], SurvivorRule::Most, 0, 1);
        let mut q = inquiry();
        assert_eq!(s.next(&q).unwrap().as_str(), "filter");
        assert_eq!(s.next(&q).unwrap().as_str(), "filter:a");
        set_survivors(&mut q, 0);
        let state_mid = s.state();
        assert_eq!(s.next(&q).unwrap().as_str(), "filter");
        assert_eq!(s.chosen(), None);
        let mut t =
            SurvivorCountSelector::new(vocab(), vec!["a".to_owned()], SurvivorRule::Most, 0, 1);
        t.restore(&state_mid).unwrap();
        assert_eq!(t.next(&q).unwrap().as_str(), "filter");
        let mut other =
            SurvivorCountSelector::new(vocab(), vec!["z".to_owned()], SurvivorRule::Most, 0, 1);
        assert!(other.restore(&state_mid).is_err());
    }
}
