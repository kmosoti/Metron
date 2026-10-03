//! Routing among fixed strategies by features the system computed.
//!
//! A [`RouterSelector`] spends a prefix of operators, reads the posterior
//! over the promised classes from the view one of them wrote (the structure
//! profile), and hands the rest of the episode to one candidate schedule
//! chosen by a [`RoutingPolicy`]. Policies are either hand-authored
//! ([`RoutingPolicy::PosteriorOrder`], the survivor rule of ADR 0011
//! generalised from counting a pool to counting a class) or fitted on the
//! train split ([`fit_tabular`], [`fit_ridge`]). Everything here is a pure
//! function of the inquiry and of training data; nothing reads a verdict.

use crate::schedule::{FixedSchedule, Scheduler};
use metron_core::id::{OperatorId, ViewId};
use metron_core::inquiry::Inquiry;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

/// The posterior over the promised classes, in published order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Posterior {
    /// Class names.
    pub classes: Vec<String>,
    /// Posterior probabilities, one per class.
    pub probabilities: Vec<f64>,
}

impl Posterior {
    /// Reads the posterior from a structure-profile view.
    #[must_use]
    pub fn from_view(inquiry: &Inquiry, view: &ViewId) -> Option<Self> {
        let json = inquiry.view(view)?.content.as_json()?;
        let mut classes = Vec::new();
        let mut probabilities = Vec::new();
        for c in json.get("classes")?.as_array()? {
            classes.push(c.get("name")?.as_str()?.to_owned());
            probabilities.push(c.get("posterior")?.as_f64()?);
        }
        Some(Self {
            classes,
            probabilities,
        })
    }

    /// Classes ranked by posterior, highest first (or lowest first); ties
    /// keep the published order.
    #[must_use]
    pub fn ranking(&self, highest_first: bool) -> Vec<String> {
        let mut idx: Vec<usize> = (0..self.classes.len()).collect();
        idx.sort_by(|&a, &b| {
            let by_probability = if highest_first {
                self.probabilities[b].total_cmp(&self.probabilities[a])
            } else {
                self.probabilities[a].total_cmp(&self.probabilities[b])
            };
            by_probability.then(a.cmp(&b))
        });
        idx.into_iter().map(|i| self.classes[i].clone()).collect()
    }

    /// The most probable class and its probability; ties go to the class
    /// published first.
    #[must_use]
    pub fn top(&self) -> Option<(&str, f64)> {
        let mut best: Option<usize> = None;
        for i in 0..self.classes.len() {
            if best.is_none_or(|b| self.probabilities[i] > self.probabilities[b]) {
                best = Some(i);
            }
        }
        best.map(|i| (self.classes[i].as_str(), self.probabilities[i]))
    }
}

/// A strategy a router can hand the episode to: its name and the order in
/// which it tries the classes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// Strategy name.
    pub name: String,
    /// Classes in the order the strategy tries them.
    pub order: Vec<String>,
}

/// How a router chooses a candidate from the posterior.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RoutingPolicy {
    /// The candidate whose class order is the posterior ranking: highest
    /// first for the control, lowest first for the negative control.
    PosteriorOrder {
        /// Highest first (the Bayes ranking) or lowest first.
        highest_first: bool,
    },
    /// A lookup by the most probable class and a confidence bin, fitted on
    /// the train split; `default` for buckets the train split never filled.
    Tabular {
        /// Upper edges of the confidence bins, increasing.
        bins: Vec<f64>,
        /// Bucket key to candidate index.
        table: BTreeMap<String, usize>,
        /// Candidate for buckets without training data.
        default: usize,
    },
    /// One ridge regression per candidate of its cost on the posterior; the
    /// lowest predicted cost wins.
    Linear {
        /// The ridge penalty it was fitted with.
        lambda: f64,
        /// Per candidate, one weight per class probability.
        weights: Vec<Vec<f64>>,
    },
}

impl RoutingPolicy {
    /// Short label for reports.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            RoutingPolicy::PosteriorOrder {
                highest_first: true,
            } => "posterior-order".to_owned(),
            RoutingPolicy::PosteriorOrder {
                highest_first: false,
            } => "posterior-reverse".to_owned(),
            RoutingPolicy::Tabular { .. } => "tabular".to_owned(),
            RoutingPolicy::Linear { lambda, .. } => format!("ridge(λ = {lambda})"),
        }
    }

    /// The candidate to run, by index into `candidates`.
    #[must_use]
    pub fn choose(&self, posterior: &Posterior, candidates: &[Candidate]) -> usize {
        match self {
            RoutingPolicy::PosteriorOrder { highest_first } => {
                let ranking = posterior.ranking(*highest_first);
                candidates
                    .iter()
                    .position(|c| c.order == ranking)
                    .unwrap_or(0)
            }
            RoutingPolicy::Tabular {
                bins,
                table,
                default,
            } => table
                .get(&bucket(posterior, bins))
                .copied()
                .unwrap_or(*default),
            RoutingPolicy::Linear { weights, .. } => {
                let mut best = 0;
                let mut best_cost = f64::INFINITY;
                for (i, w) in weights.iter().enumerate() {
                    let predicted: f64 = w
                        .iter()
                        .zip(&posterior.probabilities)
                        .map(|(a, b)| a * b)
                        .sum();
                    if predicted < best_cost {
                        best_cost = predicted;
                        best = i;
                    }
                }
                best
            }
        }
    }
}

/// The bucket key: the most probable class and the index of the first bin
/// edge its probability falls below (`bins.len()` when above all of them).
#[must_use]
pub fn bucket(posterior: &Posterior, bins: &[f64]) -> String {
    let Some((class, p)) = posterior.top() else {
        return "none".to_owned();
    };
    let bin = bins.iter().position(|&edge| p < edge).unwrap_or(bins.len());
    format!("{class}:{bin}")
}

/// One training example: the posterior after the prefix and each
/// candidate's cost on that task.
pub type Example = (Posterior, Vec<f64>);

/// Fits the tabular policy: in each bucket, the candidate with the lowest
/// mean cost over the training examples in it (lowest index on ties).
#[must_use]
pub fn fit_tabular(examples: &[Example], bins: &[f64], default: usize) -> RoutingPolicy {
    let mut sums: BTreeMap<String, (Vec<f64>, usize)> = BTreeMap::new();
    for (posterior, costs) in examples {
        let entry = sums
            .entry(bucket(posterior, bins))
            .or_insert_with(|| (vec![0.0; costs.len()], 0));
        for (s, c) in entry.0.iter_mut().zip(costs) {
            *s += c;
        }
        entry.1 += 1;
    }
    let table = sums
        .into_iter()
        .map(|(key, (totals, _))| {
            let best = totals
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.total_cmp(b.1).then(a.0.cmp(&b.0)))
                .map_or(default, |(i, _)| i);
            (key, best)
        })
        .collect();
    RoutingPolicy::Tabular {
        bins: bins.to_vec(),
        table,
        default,
    }
}

/// Solves `a x = b` for a small dense system by Gaussian elimination with
/// partial pivoting; `None` when singular.
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let pivot = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        for row in col + 1..n {
            let f = a[row][col] / a[col][col];
            let (upper, lower) = a.split_at_mut(row);
            for (target, pivot_value) in lower[0][col..].iter_mut().zip(&upper[col][col..]) {
                *target -= f * pivot_value;
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let s: f64 = (row + 1..n).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - s) / a[row][row];
    }
    Some(x)
}

/// Fits one ridge regression per candidate of its cost on the posterior
/// probabilities (which sum to one, so they carry the intercept).
#[must_use]
pub fn fit_ridge(examples: &[Example], lambda: f64) -> RoutingPolicy {
    let d = examples.first().map_or(0, |(p, _)| p.probabilities.len());
    let candidates = examples.first().map_or(0, |(_, c)| c.len());
    let mut gram = vec![vec![0.0; d]; d];
    for (p, _) in examples {
        for (row, &pi) in gram.iter_mut().zip(&p.probabilities) {
            for (cell, &pj) in row.iter_mut().zip(&p.probabilities) {
                *cell += pi * pj;
            }
        }
    }
    for (i, row) in gram.iter_mut().enumerate() {
        row[i] += lambda;
    }
    let weights = (0..candidates)
        .map(|c| {
            let rhs: Vec<f64> = (0..d)
                .map(|i| {
                    examples
                        .iter()
                        .map(|(p, costs)| p.probabilities[i] * costs[c])
                        .sum()
                })
                .collect();
            solve(gram.clone(), rhs).unwrap_or_else(|| vec![0.0; d])
        })
        .collect();
    RoutingPolicy::Linear { lambda, weights }
}

/// Spends a prefix, reads the posterior, and runs one candidate schedule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RouterSelector {
    prefix: VecDeque<OperatorId>,
    profile_view: ViewId,
    candidates: Vec<Candidate>,
    schedules: Vec<FixedSchedule>,
    policy: RoutingPolicy,
    chosen: Option<usize>,
    posterior: Option<Posterior>,
}

impl RouterSelector {
    /// A router that runs `prefix`, reads the posterior from
    /// `profile_view`, and hands over to the candidate `policy` picks.
    #[must_use]
    pub fn new(
        prefix: Vec<OperatorId>,
        profile_view: ViewId,
        candidates: Vec<(Candidate, FixedSchedule)>,
        policy: RoutingPolicy,
    ) -> Self {
        let (candidates, schedules) = candidates.into_iter().unzip();
        Self {
            prefix: prefix.into(),
            profile_view,
            candidates,
            schedules,
            policy,
            chosen: None,
            posterior: None,
        }
    }

    /// The candidate chosen, once the prefix is spent.
    #[must_use]
    pub fn chosen(&self) -> Option<&Candidate> {
        self.chosen.map(|i| &self.candidates[i])
    }

    /// The posterior the choice was made on.
    #[must_use]
    pub fn posterior(&self) -> Option<&Posterior> {
        self.posterior.as_ref()
    }
}

impl Scheduler for RouterSelector {
    fn name(&self) -> String {
        format!("router({})", self.policy.label())
    }

    fn next(&mut self, inquiry: &Inquiry) -> Option<OperatorId> {
        if let Some(op) = self.prefix.pop_front() {
            return Some(op);
        }
        if self.chosen.is_none() {
            let posterior = Posterior::from_view(inquiry, &self.profile_view);
            let index = posterior
                .as_ref()
                .map_or(0, |p| self.policy.choose(p, &self.candidates));
            self.posterior = posterior;
            self.chosen = Some(index);
        }
        let i = self.chosen?;
        self.schedules[i].next(inquiry)
    }

    fn state(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("routers are always serialisable")
    }

    fn restore(&mut self, state: &serde_json::Value) -> Result<(), String> {
        let restored: RouterSelector =
            serde_json::from_value(state.clone()).map_err(|e| e.to_string())?;
        if restored.candidates != self.candidates || restored.policy != self.policy {
            return Err("checkpointed router does not match this router".into());
        }
        *self = restored;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::ScheduleItem;
    use metron_core::id::{FrameId, InquiryId};
    use metron_core::inquiry::{Derivation, Question, Representation};

    fn posterior(p: [f64; 3]) -> Posterior {
        Posterior {
            classes: vec!["affine".into(), "symmetric".into(), "junta".into()],
            probabilities: p.to_vec(),
        }
    }

    fn candidates() -> Vec<Candidate> {
        let orders = [
            ["affine", "symmetric", "junta"],
            ["symmetric", "junta", "affine"],
            ["junta", "affine", "symmetric"],
            ["affine", "junta", "symmetric"],
        ];
        orders
            .iter()
            .map(|o| Candidate {
                name: o.join("-"),
                order: o.iter().map(|s| (*s).to_owned()).collect(),
            })
            .collect()
    }

    #[test]
    fn posterior_order_picks_the_ranking_and_its_reverse() {
        let p = posterior([0.2, 0.7, 0.1]);
        assert_eq!(p.ranking(true), vec!["symmetric", "affine", "junta"]);
        assert_eq!(p.ranking(false), vec!["junta", "affine", "symmetric"]);
        let c = candidates();
        let control = RoutingPolicy::PosteriorOrder {
            highest_first: true,
        };
        // No candidate has symmetric-affine-junta: the default (0) is used.
        assert_eq!(control.choose(&p, &c), 0);
        let q = posterior([0.1, 0.6, 0.3]);
        assert_eq!(control.choose(&q, &c), 1);
        let reverse = RoutingPolicy::PosteriorOrder {
            highest_first: false,
        };
        assert_eq!(reverse.choose(&p, &c), 2);
        assert_eq!(posterior([0.5, 0.5, 0.0]).top(), Some(("affine", 0.5)));
    }

    #[test]
    fn tabular_policy_learns_the_cheapest_candidate_per_bucket() {
        let examples: Vec<Example> = vec![
            (posterior([0.95, 0.03, 0.02]), vec![10.0, 30.0, 20.0, 12.0]),
            (posterior([0.97, 0.02, 0.01]), vec![12.0, 30.0, 20.0, 11.0]),
            (posterior([0.1, 0.1, 0.8]), vec![50.0, 40.0, 25.0, 30.0]),
        ];
        let policy = fit_tabular(&examples, &[0.6, 0.9], 0);
        assert_eq!(bucket(&examples[0].0, &[0.6, 0.9]), "affine:2");
        assert_eq!(bucket(&examples[2].0, &[0.6, 0.9]), "junta:1");
        let c = candidates();
        // Bucket affine:2 averages 11 vs 11.5: candidate 0.
        assert_eq!(policy.choose(&posterior([0.99, 0.0, 0.01]), &c), 0);
        assert_eq!(policy.choose(&posterior([0.2, 0.0, 0.8]), &c), 2);
        // An empty bucket falls back to the default.
        assert_eq!(policy.choose(&posterior([0.1, 0.85, 0.05]), &c), 0);
    }

    #[test]
    fn ridge_recovers_a_linear_cost_model() {
        // Cost of candidate 0 is 10·p_a + 50·p_s + 90·p_j; candidate 1 is
        // the reverse. Ridge with a tiny penalty recovers both and routes
        // by the posterior.
        let mut examples: Vec<Example> = Vec::new();
        for i in 0..=10 {
            for j in 0..=(10 - i) {
                let p = [i as f64 / 10.0, j as f64 / 10.0, (10 - i - j) as f64 / 10.0];
                let c0 = 10.0 * p[0] + 50.0 * p[1] + 90.0 * p[2];
                let c1 = 90.0 * p[0] + 50.0 * p[1] + 10.0 * p[2];
                examples.push((posterior(p), vec![c0, c1]));
            }
        }
        let RoutingPolicy::Linear { weights, .. } = fit_ridge(&examples, 1e-9) else {
            panic!("linear policy");
        };
        for (w, expected) in weights.iter().zip([[10.0, 50.0, 90.0], [90.0, 50.0, 10.0]]) {
            for (a, b) in w.iter().zip(expected) {
                assert!((a - b).abs() < 1e-6, "{w:?}");
            }
        }
        let policy = fit_ridge(&examples, 1e-9);
        let two = &candidates()[..2];
        assert_eq!(policy.choose(&posterior([0.9, 0.05, 0.05]), two), 0);
        assert_eq!(policy.choose(&posterior([0.05, 0.05, 0.9]), two), 1);
    }

    #[test]
    fn the_router_spends_its_prefix_reads_the_profile_and_hands_over() {
        let mut inquiry = Inquiry::new(
            InquiryId(1),
            Question {
                kind: "k".into(),
                statement: "s".into(),
                params: serde_json::Value::Null,
                answer_frame: FrameId::from("f"),
            },
        );
        let schedule = |op: &str| FixedSchedule::new([ScheduleItem::once(op)]);
        let c = candidates();
        let mut router = RouterSelector::new(
            vec![OperatorId::from("anchors"), OperatorId::from("profile")],
            ViewId::from("structure-profile"),
            vec![
                (c[0].clone(), schedule("a-first")),
                (c[1].clone(), schedule("s-first")),
            ],
            RoutingPolicy::PosteriorOrder {
                highest_first: true,
            },
        );
        assert_eq!(router.next(&inquiry).unwrap().as_str(), "anchors");
        assert_eq!(router.next(&inquiry).unwrap().as_str(), "profile");
        inquiry.write_view(
            "structure-profile",
            "hypotheses/structure-profile",
            Representation::Json(serde_json::json!({"classes": [
                {"name": "affine", "posterior": 0.1},
                {"name": "symmetric", "posterior": 0.6},
                {"name": "junta", "posterior": 0.3}
            ]})),
            Derivation::by(OperatorId::from("profile"), 1),
        );
        let mid = router.state();
        assert_eq!(router.next(&inquiry).unwrap().as_str(), "s-first");
        assert_eq!(router.chosen().unwrap().name, "symmetric-junta-affine");
        assert_eq!(router.next(&inquiry), None);
        let mut again = router.clone();
        again.restore(&mid).unwrap();
        assert_eq!(again.next(&inquiry).unwrap().as_str(), "s-first");
    }
}
