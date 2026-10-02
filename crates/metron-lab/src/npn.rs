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

/// Number of NPN classes of functions on `arity` inputs, by Burnside's
/// lemma: the mean, over the group of input permutations, input negations
/// and output negation, of the number of functions each element fixes.
///
/// An element acts on the `2^arity` input points as a bijection `σ`. With
/// the output kept, a fixed function is constant on each cycle of `σ`, so
/// `2^(cycles)` functions are fixed. With the output negated, a fixed
/// function alternates along each cycle, which is consistent only when
/// every cycle has even length. This is an independent derivation, not a
/// recalled value, and it is what [`class_count`] is checked against.
/// Feasible for `arity <= 7`.
#[must_use]
pub fn class_count_burnside(arity: u8) -> u128 {
    assert!(
        arity <= 7,
        "the group grows as 2^n n!; arity <= 7 is enough here"
    );
    let n = usize::from(arity);
    let points = 1usize << n;
    let mut perm: Vec<usize> = (0..n).collect();
    let mut total: u128 = 0;
    let mut elements: u128 = 0;
    loop {
        for negation in 0..points {
            // σ(x): permute the bits of x by `perm`, then negate by `negation`.
            let sigma = |x: usize| -> usize {
                let mut y = 0usize;
                for (j, &p) in perm.iter().enumerate() {
                    if (x >> p) & 1 == 1 {
                        y |= 1 << j;
                    }
                }
                y ^ negation
            };
            let mut seen = vec![false; points];
            let mut cycles = 0u32;
            let mut all_even = true;
            for start in 0..points {
                if seen[start] {
                    continue;
                }
                cycles += 1;
                let mut len = 0usize;
                let mut x = start;
                while !seen[x] {
                    seen[x] = true;
                    x = sigma(x);
                    len += 1;
                }
                if len % 2 == 1 {
                    all_even = false;
                }
            }
            let fixed = 1u128 << cycles;
            total += fixed; // output kept
            if all_even {
                total += fixed; // output negated
            }
            elements += 2;
        }
        if !next_permutation(&mut perm) {
            break;
        }
    }
    total / elements
}

/// Number of NPN classes of functions on `arity` inputs, by exhaustive
/// enumeration under [`canonical`]. Feasible for `arity <= 4`. Agreement
/// with [`class_count_burnside`] validates the canonicaliser, since an
/// invariant that merged too little or too much would miss the count.
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
    fn burnside_reproduces_the_small_counts_and_the_canonicaliser_agrees() {
        // Derived, not recalled: Burnside over the NPN group.
        assert_eq!(class_count_burnside(0), 1);
        assert_eq!(class_count_burnside(1), 2);
        assert_eq!(class_count_burnside(2), 4);
        assert_eq!(class_count_burnside(3), 14);
        assert_eq!(class_count_burnside(4), 222);
        // Exhaustive enumeration under the canonicaliser agrees with the
        // derivation, which is the check that the canonical form is exact.
        for arity in 1..=3 {
            assert_eq!(
                u128::try_from(class_count(arity)).unwrap(),
                class_count_burnside(arity)
            );
        }
    }

    #[test]
    fn class_count_at_arity_four_is_222() {
        assert_eq!(class_count(4), 222);
        assert_eq!(class_count_burnside(4), 222);
    }

    #[test]
    fn burnside_at_arity_five_and_six() {
        // Values this test was written to pin: 616,126 and
        // 200,253,952,527,184. They were recalled from OEIS A000370 before
        // being computed; the computation below is the independent check.
        assert_eq!(class_count_burnside(5), 616_126);
        assert_eq!(class_count_burnside(6), 200_253_952_527_184);
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
