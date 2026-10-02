//! Diagnostics that test the laboratory's own assumptions.
//!
//! These are not experiments on the system under study; they check the
//! priors the laboratory's design rests on, so that a claim like "the
//! family label is not identifiable from probes" is measured rather than
//! asserted. Each diagnostic is pure and reproducible from a seed.

use crate::families::{Family, LabelCrosstab, label_crosstab};
use crate::tasks::TaskSet;
use crate::truth_table::TruthTable;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Labels against predicates, over a task set's pool.
#[must_use]
pub fn pool_crosstab(set: &TaskSet) -> Vec<LabelCrosstab> {
    label_crosstab(
        set.pool
            .members()
            .iter()
            .map(|m| (m.family.label(), &m.table)),
    )
}

/// One bucket of posterior mass.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CalibrationBucket {
    /// Lower bound of the top family's posterior mass.
    pub mass_lo: f64,
    /// Upper bound.
    pub mass_hi: f64,
    /// Tasks in the bucket.
    pub tasks: usize,
    /// Mean posterior mass of the top family.
    pub mean_mass: f64,
    /// How often the top family was the target's family.
    pub accuracy: f64,
}

/// Simulates `k` greedy probes against the whole pool for every task, then
/// reads the family posterior under a uniform prior over pool members
/// (survivors per family over all survivors) and checks whether the
/// family with the most survivors is the target's. Buckets the tasks by
/// the top family's posterior mass.
///
/// A calibrated posterior has accuracy close to mean mass in every bucket.
#[must_use]
pub fn family_posterior_calibration(
    set: &TaskSet,
    k: usize,
    buckets: usize,
) -> Vec<CalibrationBucket> {
    let pool = set.pool.members();
    let rows = set.pool.rows();
    let mut samples: Vec<(f64, bool)> = Vec::new();
    for task in &set.tasks {
        let target: &TruthTable = &task.target.table;
        let mut alive: Vec<bool> = vec![true; pool.len()];
        let mut observed: Vec<bool> = vec![false; rows];
        for _ in 0..k {
            // Greedy: the row that minimises the larger side of the split.
            let mut best: Option<(usize, usize)> = None;
            for (row, &seen) in observed.iter().enumerate() {
                if seen {
                    continue;
                }
                let ones = (0..pool.len())
                    .filter(|&i| alive[i] && pool[i].table.eval(row))
                    .count();
                let total = alive.iter().filter(|&&a| a).count();
                let zeros = total - ones;
                if ones == 0 || zeros == 0 {
                    continue;
                }
                let worst = ones.max(zeros);
                if best.is_none_or(|(w, _)| worst < w) {
                    best = Some((worst, row));
                }
            }
            let Some((_, row)) = best else {
                break;
            };
            observed[row] = true;
            let value = target.eval(row);
            for (i, m) in pool.iter().enumerate() {
                if alive[i] && m.table.eval(row) != value {
                    alive[i] = false;
                }
            }
        }
        let mut counts: BTreeMap<Family, usize> = BTreeMap::new();
        for (i, m) in pool.iter().enumerate() {
            if alive[i] {
                *counts.entry(m.family).or_insert(0) += 1;
            }
        }
        let total: usize = counts.values().sum();
        if total == 0 {
            continue;
        }
        let (top, &top_count) = counts
            .iter()
            .max_by_key(|(f, c)| (**c, std::cmp::Reverse(**f)))
            .expect("non-empty");
        samples.push((top_count as f64 / total as f64, *top == task.target.family));
    }
    let buckets = buckets.max(1);
    (0..buckets)
        .map(|b| {
            let lo = b as f64 / buckets as f64;
            let hi = (b + 1) as f64 / buckets as f64;
            let members: Vec<&(f64, bool)> = samples
                .iter()
                .filter(|(m, _)| *m >= lo && (*m < hi || (b + 1 == buckets && *m <= hi)))
                .collect();
            let n = members.len();
            CalibrationBucket {
                mass_lo: lo,
                mass_hi: hi,
                tasks: n,
                mean_mass: if n == 0 {
                    0.0
                } else {
                    members.iter().map(|(m, _)| m).sum::<f64>() / n as f64
                },
                accuracy: if n == 0 {
                    0.0
                } else {
                    members.iter().filter(|(_, c)| *c).count() as f64 / n as f64
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::families::FamilyParams;
    use crate::pool::PoolSpec;
    use crate::tasks::{SplitFractionsSpec, TaskSetSpec};

    fn headroom_like_set(arity: u8, seed: u64) -> TaskSet {
        TaskSet::generate(
            &TaskSetSpec {
                arity,
                pool: PoolSpec {
                    families: Family::ALL.to_vec(),
                    per_family: 40,
                    params: FamilyParams::default(),
                },
                targets_per_family: 5,
                split: SplitFractionsSpec::default(),
            },
            seed,
        )
    }

    #[test]
    fn labels_are_not_properties_of_the_functions() {
        // The pools the headroom reports used.
        for (arity, seed) in [(5u8, 101u64), (6, 201)] {
            let set = headroom_like_set(arity, seed);
            let rows = pool_crosstab(&set);
            println!("arity {arity} seed {seed} pool {}:", set.pool.len());
            for r in &rows {
                println!(
                    "  {:<14} count {:>3}  affine {:>3}  monotone {:>3}",
                    r.label, r.count, r.affine, r.monotone
                );
            }
            let monotone_not_labelled: usize = rows
                .iter()
                .filter(|r| r.label != "monotone")
                .map(|r| r.monotone)
                .sum();
            let affine_not_labelled: usize = rows
                .iter()
                .filter(|r| r.label != "affine")
                .map(|r| r.affine)
                .sum();
            assert!(
                monotone_not_labelled > 0,
                "monotone functions should appear under other labels (arity {arity})"
            );
            println!(
                "  monotone under other labels: {monotone_not_labelled}; affine under other labels: {affine_not_labelled}"
            );
        }
    }

    #[test]
    fn most_survivors_posterior_is_informative_but_the_prior_is_over_labels() {
        let set = headroom_like_set(5, 101);
        for k in [2usize, 4, 6] {
            let buckets = family_posterior_calibration(&set, k, 4);
            println!("k = {k}:");
            for b in &buckets {
                println!(
                    "  mass [{:.2}, {:.2}) tasks {:>3} mean mass {:.3} accuracy {:.3}",
                    b.mass_lo, b.mass_hi, b.tasks, b.mean_mass, b.accuracy
                );
            }
            let populated: Vec<&CalibrationBucket> =
                buckets.iter().filter(|b| b.tasks > 0).collect();
            if populated.len() >= 2 {
                let first = populated.first().unwrap();
                let last = populated.last().unwrap();
                assert!(
                    last.accuracy >= first.accuracy,
                    "higher posterior mass should not be less accurate at k = {k}"
                );
            }
        }
    }
}
