//! Task sets: targets drawn from a pool, split so that no NPN class spans
//! two splits.

use crate::families::{Family, Target};
use crate::npn::canonical;
use crate::pool::{HypothesisPool, PoolSpec};
use metron_core::rng::Rng;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Which split a task belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Split {
    /// For fitting anything that learns.
    Train,
    /// For choosing between learners.
    Validation,
    /// For the final report, touched once.
    Test,
}

impl Split {
    /// Label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Split::Train => "train",
            Split::Validation => "validation",
            Split::Test => "test",
        }
    }
}

/// Target fractions per split.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SplitFractions {
    /// Train fraction.
    pub train: f64,
    /// Validation fraction.
    pub validation: f64,
    /// Test fraction.
    pub test: f64,
}

impl Default for SplitFractions {
    fn default() -> Self {
        Self {
            train: 0.5,
            validation: 0.2,
            test: 0.3,
        }
    }
}

/// One task: a hidden target plus bookkeeping.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Task {
    /// Stable identifier within the task set.
    pub id: String,
    /// The target. Laboratory property.
    pub target: Target,
    /// Index of the target in the pool.
    pub pool_index: usize,
    /// Split.
    pub split: Split,
    /// NPN class representative, as rows.
    pub npn_class: String,
}

/// How to generate a task set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSetSpec {
    /// Number of inputs.
    pub arity: u8,
    /// The pool.
    pub pool: PoolSpec,
    /// Targets to draw per family, from the pool.
    pub targets_per_family: usize,
    /// Split fractions.
    #[serde(default)]
    pub split: SplitFractionsSpec,
}

/// Serialisable split fractions with a default.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SplitFractionsSpec {
    /// Train fraction.
    #[serde(default = "half")]
    pub train: f64,
    /// Validation fraction.
    #[serde(default = "fifth")]
    pub validation: f64,
    /// Test fraction.
    #[serde(default = "three_tenths")]
    pub test: f64,
}

const fn half() -> f64 {
    0.5
}
const fn fifth() -> f64 {
    0.2
}
const fn three_tenths() -> f64 {
    0.3
}

impl Default for SplitFractionsSpec {
    fn default() -> Self {
        Self {
            train: half(),
            validation: fifth(),
            test: three_tenths(),
        }
    }
}

impl PartialEq<SplitFractions> for SplitFractionsSpec {
    fn eq(&self, other: &SplitFractions) -> bool {
        self.train == other.train && self.validation == other.validation && self.test == other.test
    }
}

impl Eq for SplitFractionsSpec {}

/// A pool and the tasks drawn from it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TaskSet {
    /// Seed everything was generated from.
    pub seed: u64,
    /// Number of inputs.
    pub arity: u8,
    /// The pool.
    pub pool: HypothesisPool,
    /// The tasks.
    pub tasks: Vec<Task>,
}

impl TaskSet {
    /// Generates a task set.
    #[must_use]
    pub fn generate(spec: &TaskSetSpec, seed: u64) -> Self {
        let mut rng = Rng::seed_from_u64(seed);
        let pool = HypothesisPool::build(spec.arity, &spec.pool, &mut rng);
        let mut drawn: Vec<(usize, Target)> = Vec::new();
        for &family in &spec.pool.families {
            let mut indices = pool.indices_of(family);
            rng.shuffle(&mut indices);
            for &i in indices.iter().take(spec.targets_per_family) {
                let m = &pool.members()[i];
                drawn.push((
                    i,
                    Target {
                        table: m.table.clone(),
                        family: m.family,
                        description: m.description.clone(),
                    },
                ));
            }
        }
        // Group by NPN class, then deal whole classes into splits.
        let mut classes: BTreeMap<String, Vec<(usize, Target)>> = BTreeMap::new();
        for (i, t) in drawn {
            let key = canonical(&t.table).to_rows_string();
            classes.entry(key).or_default().push((i, t));
        }
        let total: usize = classes.values().map(Vec::len).sum();
        let mut class_list: Vec<(String, Vec<(usize, Target)>)> = classes.into_iter().collect();
        rng.shuffle(&mut class_list);
        let fractions = [
            (Split::Train, spec.split.train),
            (Split::Validation, spec.split.validation),
            (Split::Test, spec.split.test),
        ];
        let weight_sum: f64 = fractions
            .iter()
            .map(|(_, w)| w)
            .sum::<f64>()
            .max(f64::EPSILON);
        let mut quota: Vec<(Split, f64)> = fractions
            .iter()
            .map(|&(s, w)| (s, w / weight_sum * total as f64))
            .collect();
        let mut tasks = Vec::new();
        for (key, members) in class_list {
            // Give the class to the split with the largest remaining quota.
            let (slot, _) = quota
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.1.partial_cmp(&b.1.1).expect("finite"))
                .expect("three splits");
            let split = quota[slot].0;
            quota[slot].1 -= members.len() as f64;
            for (pool_index, target) in members {
                tasks.push(Task {
                    id: format!("{}-{}", target.family.label(), pool_index),
                    target,
                    pool_index,
                    split,
                    npn_class: key.clone(),
                });
            }
        }
        tasks.sort_by(|a, b| a.id.cmp(&b.id));
        Self {
            seed,
            arity: spec.arity,
            pool,
            tasks,
        }
    }

    /// Tasks in `split`.
    #[must_use]
    pub fn in_split(&self, split: Split) -> Vec<&Task> {
        self.tasks.iter().filter(|t| t.split == split).collect()
    }

    /// Tasks of `family`.
    #[must_use]
    pub fn of_family(&self, family: Family) -> Vec<&Task> {
        self.tasks
            .iter()
            .filter(|t| t.target.family == family)
            .collect()
    }

    /// Checks that no NPN class appears in two splits.
    pub fn verify_split_hygiene(&self) -> Result<(), String> {
        let mut seen: BTreeMap<&str, Split> = BTreeMap::new();
        for t in &self.tasks {
            match seen.get(t.npn_class.as_str()) {
                Some(&s) if s != t.split => {
                    return Err(format!(
                        "NPN class of task {} appears in both {} and {}",
                        t.id,
                        s.label(),
                        t.split.label()
                    ));
                }
                _ => {
                    seen.insert(&t.npn_class, t.split);
                }
            }
        }
        Ok(())
    }

    /// Shannon entropy, in bits, of the distribution the targets are drawn
    /// from: uniform over the families that have members, then uniform over
    /// that family's members of the pool.
    ///
    /// No strategy that is not told the target's family and always answers
    /// correctly averages fewer probes than this (Kraft's inequality;
    /// `bounds::optimal_expected_depth` checks the bound on small classes).
    /// A virtual best solver below this floor is spending information the
    /// probes do not carry. Laboratory property: it describes the target
    /// distribution.
    #[must_use]
    pub fn entropy_floor(&self) -> f64 {
        let counts = self.pool.family_counts();
        let k = counts.len() as f64;
        if counts.is_empty() {
            return 0.0;
        }
        counts.values().map(|&n| (k * n as f64).log2()).sum::<f64>() / k
    }

    /// Number of distinct NPN classes among the tasks.
    #[must_use]
    pub fn class_count(&self) -> usize {
        self.tasks
            .iter()
            .map(|t| t.npn_class.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::families::FamilyParams;

    fn spec(arity: u8) -> TaskSetSpec {
        TaskSetSpec {
            arity,
            pool: PoolSpec {
                families: Family::ALL.to_vec(),
                per_family: 12,
                params: FamilyParams::default(),
            },
            targets_per_family: 4,
            split: SplitFractionsSpec::default(),
        }
    }

    #[test]
    fn task_sets_are_deterministic_and_npn_clean() {
        let a = TaskSet::generate(&spec(5), 3);
        let b = TaskSet::generate(&spec(5), 3);
        assert_eq!(a, b);
        a.verify_split_hygiene().unwrap();
        assert!(a.tasks.len() <= 24 && a.tasks.len() >= 12);
        assert!(
            a.in_split(Split::Train).len()
                + a.in_split(Split::Validation).len()
                + a.in_split(Split::Test).len()
                == a.tasks.len()
        );
        for t in &a.tasks {
            assert_eq!(a.pool.members()[t.pool_index].table, t.target.table);
        }
        let c = TaskSet::generate(&spec(5), 4);
        assert_ne!(a.pool.hash(), c.pool.hash());
    }

    #[test]
    fn entropy_floor_is_the_entropy_of_family_then_member() {
        let set = TaskSet::generate(&spec(5), 3);
        let counts = set.pool.family_counts();
        let k = counts.len() as f64;
        let weights: Vec<f64> = counts
            .values()
            .flat_map(|&n| std::iter::repeat_n(1.0 / (k * n as f64), n))
            .collect();
        let direct = crate::bounds::entropy_bits(&weights);
        assert!((set.entropy_floor() - direct).abs() < 1e-9);
        if counts.values().all(|&n| n == 12) {
            assert!((set.entropy_floor() - (k * 12.0).log2()).abs() < 1e-9);
        }
    }

    #[test]
    fn split_hygiene_detects_a_shared_class() {
        let mut set = TaskSet::generate(&spec(4), 9);
        set.verify_split_hygiene().unwrap();
        if set.tasks.len() >= 2 {
            let class = set.tasks[0].npn_class.clone();
            let other = if set.tasks[0].split == Split::Train {
                Split::Test
            } else {
                Split::Train
            };
            set.tasks[1].npn_class = class;
            set.tasks[1].split = other;
            assert!(set.verify_split_hygiene().is_err());
        }
    }
}
