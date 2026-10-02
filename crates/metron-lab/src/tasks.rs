//! Task sets: targets drawn from a pool, split so that no NPN class spans
//! two splits.

use crate::families::{Family, FamilyParams, Target};
use crate::npn::canonical;
use crate::pool::{HypothesisPool, PoolSpec};
use metron_core::rng::Rng;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The split a class goes to under class hashing: the first 64 bits of the
/// SHA-256 of its canonical rows, read as a point in `[0, 1)`, placed
/// against the cumulative fractions.
fn split_for_class(key: &str, fractions: &[(Split, f64); 3], weight_sum: f64) -> Split {
    let digest = metron_core::hash::ContentHash::of_bytes(key.as_bytes()).to_hex();
    let head = u64::from_str_radix(&digest[..16], 16).expect("hex digest");
    let point = head as f64 / 2f64.powi(64) * weight_sum;
    let mut cumulative = 0.0;
    for &(split, w) in fractions {
        cumulative += w;
        if point < cumulative {
            return split;
        }
    }
    fractions[2].0
}

/// Checks that no NPN class appears in two splits across several task
/// sets: the condition for pooling their splits.
pub fn verify_pooled_split_hygiene(sets: &[TaskSet]) -> Result<(), String> {
    let mut seen: BTreeMap<&str, (Split, u64)> = BTreeMap::new();
    for set in sets {
        for t in &set.tasks {
            match seen.get(t.npn_class.as_str()) {
                Some(&(s, seed)) if s != t.split => {
                    return Err(format!(
                        "NPN class of task {} (seed {}) is in {}, but in {} under seed {seed}",
                        t.id,
                        set.seed,
                        t.split.label(),
                        s.label()
                    ));
                }
                _ => {
                    seen.insert(&t.npn_class, (t.split, set.seed));
                }
            }
        }
    }
    Ok(())
}

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
    /// Whether the pool is published to the system. When it is not, the
    /// question publishes the structure promise instead (ADR 0015): the
    /// classes, their definitions and sizes, and the prior; every family
    /// must then be a structure-keyed class. Serialised only when false, so
    /// manifests written before it existed keep their hashes.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub publish_pool: bool,
    /// Assign each NPN class to a split by hashing its canonical form,
    /// instead of dealing the classes of one task set into splits. With
    /// class hashing a class lands in the same split in every task set, so
    /// splits stay NPN-clean when several seeds are pooled for learning.
    /// Fractions then hold over classes, in expectation. Serialised only
    /// when set.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub class_hash_splits: bool,
}

const fn yes() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_true(b: &bool) -> bool {
    *b
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
    /// Whether the pool is published to the system.
    pub publish_pool: bool,
    /// The families targets were drawn from, in specification order.
    pub families: Vec<Family>,
    /// Generator knobs, for the structure promise.
    pub params: FamilyParams,
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
            let split = if spec.class_hash_splits {
                split_for_class(&key, &fractions, weight_sum)
            } else {
                // Give the class to the split with the largest remaining quota.
                let (slot, _) = quota
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.1.partial_cmp(&b.1.1).expect("finite"))
                    .expect("three splits");
                quota[slot].1 -= members.len() as f64;
                quota[slot].0
            };
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
            publish_pool: spec.publish_pool,
            families: spec.pool.families.clone(),
            params: spec.pool.params.clone(),
        }
    }

    /// The structure promise a pool-free question publishes: each class with
    /// its definition and exact size, and the prior over them. Public
    /// knowledge about the classes, nothing about the target.
    #[must_use]
    pub fn structure_promise(&self) -> serde_json::Value {
        let classes: Vec<serde_json::Value> = self
            .families
            .iter()
            .map(|f| {
                serde_json::json!({
                    "name": f.label(),
                    "definition": f.definition(&self.params),
                    "size": f.class_size(self.arity, &self.params),
                })
            })
            .collect();
        serde_json::json!({
            "classes": classes,
            "prior": "uniform over the classes, then uniform within the class",
            "junta_size": self.params.junta_size,
            "non_constant": true,
        })
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
    ///
    /// When the pool is published, the support is the pool. When it is not,
    /// the support is the union of the structure-keyed classes, each of its
    /// exact size.
    #[must_use]
    pub fn entropy_floor(&self) -> f64 {
        let sizes: Vec<f64> = if self.publish_pool {
            self.pool
                .family_counts()
                .values()
                .map(|&n| n as f64)
                .collect()
        } else {
            self.families
                .iter()
                .filter_map(|f| f.class_size(self.arity, &self.params))
                .filter(|&n| n > 0.0)
                .collect()
        };
        let k = sizes.len() as f64;
        if sizes.is_empty() {
            return 0.0;
        }
        sizes.iter().map(|&n| (k * n).log2()).sum::<f64>() / k
    }

    /// The size of the support targets are drawn from, as seen by the
    /// system: the pool when it is published, the union of the classes
    /// otherwise.
    #[must_use]
    pub fn support_size(&self) -> f64 {
        if self.publish_pool {
            self.pool.len() as f64
        } else {
            self.families
                .iter()
                .filter_map(|f| f.class_size(self.arity, &self.params))
                .sum()
        }
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
            publish_pool: true,
            class_hash_splits: false,
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
    fn pool_free_sets_publish_the_promise_and_take_the_floor_from_class_sizes() {
        let spec = TaskSetSpec {
            arity: 6,
            pool: PoolSpec {
                families: Family::STRUCTURE_KEYED.to_vec(),
                per_family: 4,
                params: FamilyParams::default(),
            },
            targets_per_family: 4,
            split: SplitFractionsSpec::default(),
            publish_pool: false,
            class_hash_splits: false,
        };
        let set = TaskSet::generate(&spec, 5);
        set.verify_split_hygiene().unwrap();
        let promise = set.structure_promise();
        let classes = promise["classes"].as_array().unwrap();
        assert_eq!(classes.len(), 3);
        assert_eq!(classes[0]["name"], "affine");
        assert_eq!(classes[0]["size"], 126.0);
        let sizes = [126.0f64, 124.0, 20.0 * 216.0];
        let expected = sizes.iter().map(|n| (3.0 * n).log2()).sum::<f64>() / 3.0;
        assert!((set.entropy_floor() - expected).abs() < 1e-9);
        assert_eq!(set.support_size(), sizes.iter().sum::<f64>());
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("publish_pool"));
        let published = TaskSetSpec {
            publish_pool: true,
            ..spec
        };
        assert!(
            !serde_json::to_string(&published)
                .unwrap()
                .contains("publish_pool")
        );
    }

    #[test]
    fn class_hash_splits_stay_clean_when_seeds_are_pooled() {
        let mut spec = spec(4);
        spec.pool.per_family = 6;
        spec.targets_per_family = 6;
        let dealt: Vec<TaskSet> = (0..12).map(|s| TaskSet::generate(&spec, s)).collect();
        for set in &dealt {
            set.verify_split_hygiene().unwrap();
        }
        // At arity 4 classes recur across seeds, so dealing per set leaks.
        assert!(verify_pooled_split_hygiene(&dealt).is_err());
        spec.class_hash_splits = true;
        let hashed: Vec<TaskSet> = (0..12).map(|s| TaskSet::generate(&spec, s)).collect();
        verify_pooled_split_hygiene(&hashed).unwrap();
        let in_test: usize = hashed.iter().map(|s| s.in_split(Split::Test).len()).sum();
        assert!(in_test > 0, "some classes hash to the test split");
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
