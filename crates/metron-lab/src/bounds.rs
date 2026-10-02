//! Query rulers for a finite class.
//!
//! * [`information_lower_bound`]: `⌈log2 |C|⌉` membership queries are
//!   necessary to identify a member of `C` in the worst case.
//! * [`greedy_depths`]: what the greedy "split the survivors" rule actually
//!   spends on each member, the near-optimal baseline of Dasgupta (2004).
//! * [`optimal_query_depth`]: the exact worst-case depth of the optimal
//!   adaptive query tree, by dynamic programming over survivor sets; only
//!   feasible for small explicit classes.

use crate::truth_table::TruthTable;
use std::collections::HashMap;

/// `⌈log2 n⌉`, the information-theoretic lower bound on worst-case queries.
#[must_use]
pub const fn information_lower_bound(class_size: usize) -> u32 {
    if class_size <= 1 {
        0
    } else {
        (class_size - 1).ilog2() + 1
    }
}

/// Precomputed row patterns over a class: `patterns[row]` has bit `i` set
/// when member `i` is true at `row`.
fn row_patterns(tables: &[TruthTable]) -> Vec<u64> {
    let rows = tables.first().map_or(0, TruthTable::rows);
    (0..rows)
        .map(|row| {
            tables
                .iter()
                .enumerate()
                .filter(|(_, t)| t.eval(row))
                .fold(0u64, |acc, (i, _)| acc | (1u64 << i))
        })
        .collect()
}

/// The row the greedy rule picks for survivor set `survivors`: the one that
/// minimises the larger side of the split, lowest index on ties. `None`
/// when no row splits the survivors.
#[must_use]
pub fn greedy_row(patterns: &[u64], survivors: u64) -> Option<usize> {
    let mut best: Option<(u32, usize)> = None;
    for (row, &p) in patterns.iter().enumerate() {
        let ones = (survivors & p).count_ones();
        let zeros = (survivors & !p).count_ones();
        if ones == 0 || zeros == 0 {
            continue;
        }
        let worst = ones.max(zeros);
        if best.is_none_or(|(w, _)| worst < w) {
            best = Some((worst, row));
        }
    }
    best.map(|(_, row)| row)
}

/// Queries the greedy rule spends to identify each member of the class
/// (at most 64 members). Members the class cannot distinguish stop when no
/// row splits the survivors.
#[must_use]
pub fn greedy_depths(tables: &[TruthTable]) -> Vec<u32> {
    assert!(
        tables.len() <= 64,
        "explicit rulers are limited to 64 members"
    );
    let patterns = row_patterns(tables);
    let all = if tables.len() == 64 {
        u64::MAX
    } else {
        (1u64 << tables.len()) - 1
    };
    (0..tables.len())
        .map(|target| {
            let mut survivors = all;
            let mut depth = 0;
            while survivors.count_ones() > 1 {
                let Some(row) = greedy_row(&patterns, survivors) else {
                    break;
                };
                depth += 1;
                survivors &= if tables[target].eval(row) {
                    patterns[row]
                } else {
                    !patterns[row]
                };
            }
            depth
        })
        .collect()
}

/// Exact worst-case depth of the optimal adaptive membership-query tree
/// for the class (at most 64 members). `None` if the class holds members
/// no row distinguishes.
#[must_use]
pub fn optimal_query_depth(tables: &[TruthTable]) -> Option<u32> {
    assert!(
        tables.len() <= 64,
        "explicit rulers are limited to 64 members"
    );
    if tables.len() <= 1 {
        return Some(0);
    }
    let patterns = row_patterns(tables);
    let all = if tables.len() == 64 {
        u64::MAX
    } else {
        (1u64 << tables.len()) - 1
    };
    let mut memo: HashMap<u64, Option<u32>> = HashMap::new();
    fn depth(
        survivors: u64,
        patterns: &[u64],
        memo: &mut HashMap<u64, Option<u32>>,
    ) -> Option<u32> {
        if survivors.count_ones() <= 1 {
            return Some(0);
        }
        if let Some(&d) = memo.get(&survivors) {
            return d;
        }
        let mut best: Option<u32> = None;
        for &p in patterns {
            let ones = survivors & p;
            let zeros = survivors & !p;
            if ones == 0 || zeros == 0 {
                continue;
            }
            let Some(a) = depth(ones, patterns, memo) else {
                continue;
            };
            let Some(b) = depth(zeros, patterns, memo) else {
                continue;
            };
            let d = 1 + a.max(b);
            if best.is_none_or(|bd| d < bd) {
                best = Some(d);
            }
        }
        memo.insert(survivors, best);
        best
    }
    depth(all, &patterns, &mut memo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::families::{Family, FamilyParams, sample};
    use metron_core::rng::Rng;

    #[test]
    fn lower_bound() {
        assert_eq!(information_lower_bound(0), 0);
        assert_eq!(information_lower_bound(1), 0);
        assert_eq!(information_lower_bound(2), 1);
        assert_eq!(information_lower_bound(3), 2);
        assert_eq!(information_lower_bound(8), 3);
        assert_eq!(information_lower_bound(9), 4);
    }

    #[test]
    fn rulers_bracket_the_greedy_rule() {
        let params = FamilyParams::default();
        let mut rng = Rng::seed_from_u64(2);
        let mut tables = Vec::new();
        while tables.len() < 16 {
            let t = sample(Family::KTermDnf, 4, &params, &mut rng).table;
            if !tables.contains(&t) {
                tables.push(t);
            }
        }
        let optimal = optimal_query_depth(&tables).unwrap();
        let greedy = greedy_depths(&tables);
        let worst_greedy = *greedy.iter().max().unwrap();
        assert!(optimal >= information_lower_bound(tables.len()));
        assert!(
            worst_greedy >= optimal,
            "greedy {worst_greedy} < optimal {optimal}"
        );
        assert!(worst_greedy < tables.len() as u32);
    }

    #[test]
    fn greedy_is_near_optimal_on_random_small_classes() {
        // Prior from Dasgupta (2004) and Golovin & Krause (2010): greedy
        // splitting is near-optimal. Measured here as the ratio of greedy
        // worst-case depth to the exact optimal depth over random explicit
        // classes drawn from mixed families.
        let params = FamilyParams::default();
        let mut rng = Rng::seed_from_u64(77);
        let mut ratios = Vec::new();
        let mut equal = 0;
        let trials = 60;
        for t in 0..trials {
            let arity = if t % 2 == 0 { 4 } else { 5 };
            let size = 8 + rng.below_usize(9);
            let mut tables = Vec::new();
            let mut guard = 0;
            while tables.len() < size && guard < 500 {
                guard += 1;
                let family = Family::ALL[rng.below_usize(Family::ALL.len())];
                let table = sample(family, arity, &params, &mut rng).table;
                if !tables.contains(&table) {
                    tables.push(table);
                }
            }
            let Some(optimal) = optimal_query_depth(&tables) else {
                continue;
            };
            let greedy = *greedy_depths(&tables).iter().max().unwrap();
            if greedy == optimal {
                equal += 1;
            }
            ratios.push(greedy as f64 / optimal.max(1) as f64);
        }
        let mean = ratios.iter().sum::<f64>() / ratios.len() as f64;
        let max = ratios.iter().cloned().fold(0.0, f64::max);
        println!(
            "greedy/optimal worst-case depth over {} classes: mean {mean:.3}, max {max:.3}, equal in {equal}",
            ratios.len()
        );
        assert!(mean <= 1.25, "mean ratio {mean}");
        assert!(max <= 2.0, "max ratio {max}");
    }

    #[test]
    fn affine_class_is_identified_in_about_log_queries() {
        let mut tables = Vec::new();
        // All 32 affine functions on 4 inputs.
        for mask in 0..16usize {
            for c in [false, true] {
                tables.push(
                    TruthTable::from_fn(4, |row| ((row & mask).count_ones() % 2 == 1) ^ c).unwrap(),
                );
            }
        }
        assert_eq!(optimal_query_depth(&tables), Some(5));
        let greedy = greedy_depths(&tables);
        assert!(greedy.iter().all(|&d| d == 5));
    }
}
