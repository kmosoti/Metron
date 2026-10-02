//! NPN canonicalisation.
//!
//! Two functions are NPN-equivalent when one becomes the other by negating
//! inputs, permuting inputs and negating the output. A learner that has seen
//! a function has, for most purposes, seen its whole class, so train and
//! test splits must not share a class. [`canonical`] computes an exact class
//! representative; [`class_count`] verifies it against OEIS A000370 for
//! small arities.
//!
//! The algorithm enumerates a transform-invariant subset of the orbit:
//! output polarity with at most half the rows true, input polarities with
//! the positive cofactor no larger than the negative one, and inputs ordered
//! by positive-cofactor weight. Ties are enumerated; variables that are
//! interchangeable in the function are collapsed so symmetric functions do
//! not blow up. The minimum over that subset is the representative.

use crate::truth_table::TruthTable;
use metron_core::rng::Rng;
use std::collections::BTreeSet;

/// The NPN class representative of `table`.
#[must_use]
pub fn canonical(table: &TruthTable) -> TruthTable {
    let n = usize::from(table.arity());
    let rows = table.rows();
    let half = rows / 2;
    let ones = table.ones();
    let mut best: Option<TruthTable> = None;

    let mut outputs = Vec::with_capacity(2);
    if ones <= half {
        outputs.push(table.clone());
    }
    if rows - ones <= half {
        outputs.push(table.negate_output());
    }

    for g in outputs {
        // 0: keep, 1: flip, 2: tie (enumerate both).
        let choice: Vec<u8> = (0..n)
            .map(|i| {
                let c1 = g.cofactor_ones(i, true);
                let c0 = g.cofactor_ones(i, false);
                match c1.cmp(&c0) {
                    std::cmp::Ordering::Less => 0,
                    std::cmp::Ordering::Greater => 1,
                    std::cmp::Ordering::Equal => 2,
                }
            })
            .collect();
        let ties: Vec<usize> = (0..n).filter(|&i| choice[i] == 2).collect();
        for mask in 0..(1u64 << ties.len()) {
            let mut h = g.clone();
            for (i, &c) in choice.iter().enumerate() {
                let flip = match c {
                    1 => true,
                    2 => {
                        let k = ties.iter().position(|&t| t == i).expect("tie listed");
                        (mask >> k) & 1 == 1
                    }
                    _ => false,
                };
                if flip {
                    h = h.negate_input(i);
                }
            }
            for perm in orderings(&h) {
                let candidate = h.permute_inputs(&perm);
                if best.as_ref().is_none_or(|b| candidate < *b) {
                    best = Some(candidate);
                }
            }
        }
    }
    best.expect("at least one output polarity qualifies")
}

/// All input orderings to try for `h`: variables sorted by positive
/// cofactor weight, ties enumerated modulo interchangeable variables.
fn orderings(h: &TruthTable) -> Vec<Vec<usize>> {
    let n = usize::from(h.arity());
    let sig: Vec<usize> = (0..n).map(|i| h.cofactor_ones(i, true)).collect();
    let mut vars: Vec<usize> = (0..n).collect();
    vars.sort_by_key(|&i| (sig[i], i));
    // Consecutive runs with equal signature form tie groups.
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for &v in &vars {
        match groups.last_mut() {
            Some(g) if sig[g[0]] == sig[v] => g.push(v),
            _ => groups.push(vec![v]),
        }
    }
    let per_group: Vec<Vec<Vec<usize>>> = groups.iter().map(|g| group_orderings(h, g)).collect();
    let mut result: Vec<Vec<usize>> = vec![Vec::new()];
    for options in per_group {
        let mut next = Vec::with_capacity(result.len() * options.len());
        for prefix in &result {
            for option in &options {
                let mut p = prefix.clone();
                p.extend_from_slice(option);
                next.push(p);
            }
        }
        result = next;
    }
    result
}

/// Distinct orderings of one tie group, collapsing variables that `h` does
/// not distinguish (swapping them leaves `h` unchanged).
fn group_orderings(h: &TruthTable, group: &[usize]) -> Vec<Vec<usize>> {
    if group.len() == 1 {
        return vec![group.to_vec()];
    }
    // Union-find over interchangeable variables.
    let mut class: Vec<usize> = (0..group.len()).collect();
    fn find(class: &mut [usize], i: usize) -> usize {
        let mut r = i;
        while class[r] != r {
            r = class[r];
        }
        let mut c = i;
        while class[c] != r {
            let next = class[c];
            class[c] = r;
            c = next;
        }
        r
    }
    for a in 0..group.len() {
        for b in (a + 1)..group.len() {
            if find(&mut class, a) != find(&mut class, b) && h.swap_inputs(group[a], group[b]) == *h
            {
                let ra = find(&mut class, a);
                let rb = find(&mut class, b);
                class[ra] = rb;
            }
        }
    }
    let labels: Vec<usize> = (0..group.len()).map(|i| find(&mut class, i)).collect();
    // Enumerate distinct permutations of the label multiset.
    let mut sorted = labels.clone();
    sorted.sort_unstable();
    let mut out = Vec::new();
    loop {
        // Assign actual variables to label positions, each class in index order.
        let mut queues: Vec<Vec<usize>> = vec![Vec::new(); group.len()];
        for (i, &l) in labels.iter().enumerate() {
            queues[l].push(group[i]);
        }
        let mut cursors = vec![0usize; group.len()];
        let ordering: Vec<usize> = sorted
            .iter()
            .map(|&l| {
                let v = queues[l][cursors[l]];
                cursors[l] += 1;
                v
            })
            .collect();
        out.push(ordering);
        if !next_permutation(&mut sorted) {
            break;
        }
    }
    out
}

fn next_permutation(a: &mut [usize]) -> bool {
    if a.len() < 2 {
        return false;
    }
    let mut i = a.len() - 1;
    while i > 0 && a[i - 1] >= a[i] {
        i -= 1;
    }
    if i == 0 {
        return false;
    }
    let mut j = a.len() - 1;
    while a[j] <= a[i - 1] {
        j -= 1;
    }
    a.swap(i - 1, j);
    a[i..].reverse();
    true
}

/// Applies a uniformly random NPN transform.
#[must_use]
pub fn random_transform(table: &TruthTable, rng: &mut Rng) -> TruthTable {
    let n = usize::from(table.arity());
    let mut perm: Vec<usize> = (0..n).collect();
    rng.shuffle(&mut perm);
    let mut t = table.permute_inputs(&perm);
    for i in 0..n {
        if rng.bool() {
            t = t.negate_input(i);
        }
    }
    if rng.bool() { t.negate_output() } else { t }
}

/// Number of NPN classes of functions on `arity` inputs, by exhaustive
/// enumeration. Feasible for `arity <= 4` (OEIS A000370: 2, 4, 14, 222).
#[must_use]
pub fn class_count(arity: u8) -> usize {
    assert!(
        arity <= 4,
        "exhaustive enumeration is only feasible for arity <= 4"
    );
    let rows = TruthTable::rows_for(arity);
    let mut classes = BTreeSet::new();
    for bits in 0..(1u64 << rows) {
        let table = TruthTable::from_fn(arity, |row| (bits >> row) & 1 == 1).expect("small arity");
        classes.insert(canonical(&table).to_rows_string());
    }
    classes.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_counts_match_oeis_a000370_for_small_arity() {
        assert_eq!(class_count(1), 2);
        assert_eq!(class_count(2), 4);
        assert_eq!(class_count(3), 14);
    }

    #[test]
    fn class_count_at_arity_four_is_222() {
        assert_eq!(class_count(4), 222);
    }

    #[test]
    fn canonical_form_is_invariant_under_random_transforms() {
        use crate::families::{Family, FamilyParams, sample};
        let params = FamilyParams::default();
        let mut rng = Rng::seed_from_u64(5);
        for arity in [5u8, 6, 7, 8] {
            for family in Family::ALL {
                let t = sample(family, arity, &params, &mut rng).table;
                let c = canonical(&t);
                for _ in 0..3 {
                    let u = random_transform(&t, &mut rng);
                    assert_eq!(canonical(&u), c, "{family} at arity {arity}");
                }
            }
        }
    }

    #[test]
    fn symmetric_functions_canonicalise_quickly() {
        let parity8 = TruthTable::from_fn(8, |row| row.count_ones() % 2 == 1).unwrap();
        let c = canonical(&parity8);
        assert_eq!(canonical(&parity8.negate_output()), c);
        assert_eq!(canonical(&parity8.negate_input(3)), c);
    }
}
