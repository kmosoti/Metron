//! Structural families of Boolean functions.
//!
//! Mixed families are what make representation choice matter: an affine
//! function is identified with `n + 1` probes in the algebraic normal form
//! and needs `2^(n-1)` terms in DNF, a monotone function is cheap as a set
//! of minimal true points, and so on. Each generator returns the table and a
//! human-readable description; the description is laboratory property and
//! is revealed only in verdicts.

use crate::truth_table::TruthTable;
use metron_core::rng::Rng;
use serde::{Deserialize, Serialize};
use std::fmt;

/// A structural family.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Family {
    /// `f(x) = a·x ⊕ b` over GF(2); parity is the case `a = 1…1`.
    Affine,
    /// Monotone (positive) functions, given by minimal true points.
    Monotone,
    /// Read-once formulas over AND, OR and negated leaves.
    ReadOnce,
    /// Linear threshold functions with small integer weights.
    Threshold,
    /// Disjunctions of `k` conjunctive terms.
    KTermDnf,
    /// Shallow decision trees.
    DecisionTree,
}

impl Family {
    /// Every family, in canonical order.
    pub const ALL: [Family; 6] = [
        Family::Affine,
        Family::Monotone,
        Family::ReadOnce,
        Family::Threshold,
        Family::KTermDnf,
        Family::DecisionTree,
    ];

    /// The label used in manifests, pool documents and reports.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Family::Affine => "affine",
            Family::Monotone => "monotone",
            Family::ReadOnce => "read-once",
            Family::Threshold => "threshold",
            Family::KTermDnf => "k-term-dnf",
            Family::DecisionTree => "decision-tree",
        }
    }

    /// Parses a label.
    #[must_use]
    pub fn parse(label: &str) -> Option<Family> {
        Family::ALL.into_iter().find(|f| f.label() == label)
    }
}

impl fmt::Display for Family {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Knobs for the generators.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FamilyParams {
    /// Terms in a `k-term-dnf` function.
    #[serde(default = "default_k_terms")]
    pub k_terms: usize,
    /// Maximum literals per DNF term.
    #[serde(default = "default_term_width")]
    pub term_width: usize,
    /// Depth of a decision tree.
    #[serde(default = "default_tree_depth")]
    pub tree_depth: usize,
    /// Minimal true points of a monotone function.
    #[serde(default = "default_minimal_points")]
    pub minimal_points: usize,
    /// Largest absolute weight of a threshold function.
    #[serde(default = "default_max_weight")]
    pub max_weight: i32,
}

const fn default_k_terms() -> usize {
    3
}
const fn default_term_width() -> usize {
    3
}
const fn default_tree_depth() -> usize {
    3
}
const fn default_minimal_points() -> usize {
    3
}
const fn default_max_weight() -> i32 {
    3
}

impl Default for FamilyParams {
    fn default() -> Self {
        Self {
            k_terms: default_k_terms(),
            term_width: default_term_width(),
            tree_depth: default_tree_depth(),
            minimal_points: default_minimal_points(),
            max_weight: default_max_weight(),
        }
    }
}

/// A generated function with its provenance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Target {
    /// The table.
    pub table: TruthTable,
    /// Which generator produced it.
    pub family: Family,
    /// A formula or structure description, for verdicts and reports.
    pub description: String,
}

/// Samples a non-constant function from `family` on `arity` inputs.
///
/// Generators that can produce constants resample up to 64 times; the
/// affine and read-once generators never produce constants.
#[must_use]
pub fn sample(family: Family, arity: u8, params: &FamilyParams, rng: &mut Rng) -> Target {
    for _ in 0..64 {
        let target = match family {
            Family::Affine => affine(arity, rng),
            Family::Monotone => monotone(arity, params, rng),
            Family::ReadOnce => read_once(arity, rng),
            Family::Threshold => threshold(arity, params, rng),
            Family::KTermDnf => k_term_dnf(arity, params, rng),
            Family::DecisionTree => decision_tree(arity, params, rng),
        };
        if !target.table.is_constant() {
            return target;
        }
    }
    // Fall back to a function that is never constant.
    affine(arity, rng)
}

fn var(i: usize) -> String {
    format!("x{i}")
}

fn affine(arity: u8, rng: &mut Rng) -> Target {
    let n = usize::from(arity);
    let mut mask = 0usize;
    while mask == 0 {
        mask = rng.below(1u64 << n) as usize;
    }
    let constant = rng.bool();
    let table = TruthTable::from_fn(arity, |row| ((row & mask).count_ones() % 2 == 1) ^ constant)
        .expect("arity validated by caller");
    let mut terms: Vec<String> = (0..n).filter(|i| (mask >> i) & 1 == 1).map(var).collect();
    if constant {
        terms.push("1".into());
    }
    Target {
        table,
        family: Family::Affine,
        description: terms.join(" ⊕ "),
    }
}

fn monotone(arity: u8, params: &FamilyParams, rng: &mut Rng) -> Target {
    let n = usize::from(arity);
    let mut points: Vec<usize> = Vec::new();
    for _ in 0..params.minimal_points.max(1) {
        let mut p = 0usize;
        while p == 0 {
            p = rng.below(1u64 << n) as usize;
        }
        if !points.contains(&p) {
            points.push(p);
        }
    }
    // Keep only minimal points so the description is honest.
    let minimal: Vec<usize> = points
        .iter()
        .copied()
        .filter(|&p| !points.iter().any(|&q| q != p && q & p == q))
        .collect();
    let table = TruthTable::from_fn(arity, |row| minimal.iter().any(|&p| row | p == row))
        .expect("arity validated by caller");
    let description = minimal
        .iter()
        .map(|&p| {
            (0..n)
                .filter(|i| (p >> i) & 1 == 1)
                .map(var)
                .collect::<Vec<_>>()
                .join("")
        })
        .collect::<Vec<_>>()
        .join(" ∨ ");
    Target {
        table,
        family: Family::Monotone,
        description,
    }
}

#[derive(Clone)]
enum Formula {
    Leaf { var: usize, negated: bool },
    And(Box<Formula>, Box<Formula>),
    Or(Box<Formula>, Box<Formula>),
}

impl Formula {
    fn eval(&self, row: usize) -> bool {
        match self {
            Formula::Leaf { var, negated } => ((row >> var) & 1 == 1) ^ negated,
            Formula::And(a, b) => a.eval(row) && b.eval(row),
            Formula::Or(a, b) => a.eval(row) || b.eval(row),
        }
    }

    fn render(&self) -> String {
        match self {
            Formula::Leaf { var: v, negated } => {
                if *negated {
                    format!("¬{}", var(*v))
                } else {
                    var(*v)
                }
            }
            Formula::And(a, b) => format!("({} ∧ {})", a.render(), b.render()),
            Formula::Or(a, b) => format!("({} ∨ {})", a.render(), b.render()),
        }
    }
}

fn build_read_once(vars: &[usize], rng: &mut Rng) -> Formula {
    if vars.len() == 1 {
        return Formula::Leaf {
            var: vars[0],
            negated: rng.bool(),
        };
    }
    let split = 1 + rng.below_usize(vars.len() - 1);
    let left = build_read_once(&vars[..split], rng);
    let right = build_read_once(&vars[split..], rng);
    if rng.bool() {
        Formula::And(Box::new(left), Box::new(right))
    } else {
        Formula::Or(Box::new(left), Box::new(right))
    }
}

fn read_once(arity: u8, rng: &mut Rng) -> Target {
    let n = usize::from(arity);
    let mut vars: Vec<usize> = (0..n).collect();
    rng.shuffle(&mut vars);
    let used = if n >= 2 {
        2 + rng.below_usize(n - 1)
    } else {
        1
    };
    let formula = build_read_once(&vars[..used], rng);
    let table =
        TruthTable::from_fn(arity, |row| formula.eval(row)).expect("arity validated by caller");
    Target {
        table,
        family: Family::ReadOnce,
        description: formula.render(),
    }
}

fn threshold(arity: u8, params: &FamilyParams, rng: &mut Rng) -> Target {
    let n = usize::from(arity);
    let w = params.max_weight.max(1);
    let mut weights: Vec<i32> = Vec::with_capacity(n);
    loop {
        weights.clear();
        for _ in 0..n {
            weights.push(rng.below(u64::try_from(2 * w + 1).expect("small")) as i32 - w);
        }
        if weights.iter().any(|&x| x != 0) {
            break;
        }
    }
    let min: i32 = weights.iter().filter(|&&x| x < 0).sum();
    let max: i32 = weights.iter().filter(|&&x| x > 0).sum();
    // θ in [min + 1, max] keeps the function non-constant.
    let theta = min + 1 + rng.below(u64::try_from(max - min).expect("max >= min + 1")) as i32;
    let table = TruthTable::from_fn(arity, |row| {
        let sum: i32 = (0..n)
            .filter(|i| (row >> i) & 1 == 1)
            .map(|i| weights[i])
            .sum();
        sum >= theta
    })
    .expect("arity validated by caller");
    let terms: Vec<String> = (0..n)
        .filter(|&i| weights[i] != 0)
        .map(|i| format!("{:+}·{}", weights[i], var(i)))
        .collect();
    Target {
        table,
        family: Family::Threshold,
        description: format!("[{} ≥ {theta}]", terms.join(" ")),
    }
}

fn random_term(n: usize, width: usize, rng: &mut Rng) -> (usize, usize) {
    let size = 1 + rng.below_usize(width.clamp(1, n));
    let mut vars: Vec<usize> = (0..n).collect();
    rng.shuffle(&mut vars);
    let mut care = 0usize;
    let mut polarity = 0usize;
    for &v in &vars[..size] {
        care |= 1 << v;
        if rng.bool() {
            polarity |= 1 << v;
        }
    }
    (care, polarity)
}

fn render_term(n: usize, care: usize, polarity: usize) -> String {
    let lits: Vec<String> = (0..n)
        .filter(|i| (care >> i) & 1 == 1)
        .map(|i| {
            if (polarity >> i) & 1 == 1 {
                var(i)
            } else {
                format!("¬{}", var(i))
            }
        })
        .collect();
    format!("({})", lits.join(" ∧ "))
}

fn k_term_dnf(arity: u8, params: &FamilyParams, rng: &mut Rng) -> Target {
    let n = usize::from(arity);
    let terms: Vec<(usize, usize)> = (0..params.k_terms.max(1))
        .map(|_| random_term(n, params.term_width, rng))
        .collect();
    let table = TruthTable::from_fn(arity, |row| {
        terms.iter().any(|&(care, pol)| row & care == pol & care)
    })
    .expect("arity validated by caller");
    Target {
        table,
        family: Family::KTermDnf,
        description: terms
            .iter()
            .map(|&(c, p)| render_term(n, c, p))
            .collect::<Vec<_>>()
            .join(" ∨ "),
    }
}

enum Tree {
    Leaf(bool),
    Node {
        var: usize,
        zero: Box<Tree>,
        one: Box<Tree>,
    },
}

impl Tree {
    fn eval(&self, row: usize) -> bool {
        match self {
            Tree::Leaf(b) => *b,
            Tree::Node { var, zero, one } => {
                if (row >> var) & 1 == 1 {
                    one.eval(row)
                } else {
                    zero.eval(row)
                }
            }
        }
    }

    fn render(&self) -> String {
        match self {
            Tree::Leaf(b) => u8::from(*b).to_string(),
            Tree::Node { var: v, zero, one } => {
                format!("({} ? {} : {})", var(*v), one.render(), zero.render())
            }
        }
    }
}

fn build_tree(depth: usize, available: &[usize], rng: &mut Rng) -> Tree {
    if depth == 0 || available.is_empty() {
        return Tree::Leaf(rng.bool());
    }
    let pick = rng.below_usize(available.len());
    let var = available[pick];
    let rest: Vec<usize> = available.iter().copied().filter(|&v| v != var).collect();
    Tree::Node {
        var,
        zero: Box::new(build_tree(depth - 1, &rest, rng)),
        one: Box::new(build_tree(depth - 1, &rest, rng)),
    }
}

fn decision_tree(arity: u8, params: &FamilyParams, rng: &mut Rng) -> Target {
    let n = usize::from(arity);
    let available: Vec<usize> = (0..n).collect();
    let tree = build_tree(params.tree_depth.max(1), &available, rng);
    let table =
        TruthTable::from_fn(arity, |row| tree.eval(row)).expect("arity validated by caller");
    Target {
        table,
        family: Family::DecisionTree,
        description: tree.render(),
    }
}

/// Whether `table` is affine over GF(2).
#[must_use]
pub fn is_affine(table: &TruthTable) -> bool {
    let f0 = table.eval(0);
    let rows = table.rows();
    // f is affine iff f(x) ⊕ f(0) is linear: f(x ⊕ y) ⊕ f(0) = (f(x) ⊕ f(0)) ⊕ (f(y) ⊕ f(0)).
    (0..rows).all(|x| (0..rows).all(|y| table.eval(x ^ y) ^ f0 == table.eval(x) ^ table.eval(y)))
}

/// Whether `table` is monotone (non-decreasing in every input).
#[must_use]
pub fn is_monotone(table: &TruthTable) -> bool {
    let n = usize::from(table.arity());
    (0..table.rows()).all(|row| {
        (0..n).all(|i| {
            let bit = 1 << i;
            row & bit != 0 || !table.eval(row) || table.eval(row | bit)
        })
    })
}

/// How many functions carrying each label satisfy the testable structural
/// predicates. A label is the generator a function came from; whether the
/// function is affine or monotone is a property of the function. When the
/// two disagree, the label is not identifiable from probes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelCrosstab {
    /// The label.
    pub label: String,
    /// Functions carrying it.
    pub count: usize,
    /// Of those, how many are affine.
    pub affine: usize,
    /// Of those, how many are monotone.
    pub monotone: usize,
}

/// Cross-tabulates labels against the testable predicates.
#[must_use]
pub fn label_crosstab<'a>(
    members: impl IntoIterator<Item = (&'a str, &'a TruthTable)>,
) -> Vec<LabelCrosstab> {
    let mut rows: Vec<LabelCrosstab> = Vec::new();
    for (label, table) in members {
        let row = match rows.iter_mut().find(|r| r.label == label) {
            Some(r) => r,
            None => {
                rows.push(LabelCrosstab {
                    label: label.to_owned(),
                    count: 0,
                    affine: 0,
                    monotone: 0,
                });
                rows.last_mut().expect("just pushed")
            }
        };
        row.count += 1;
        if is_affine(table) {
            row.affine += 1;
        }
        if is_monotone(table) {
            row.monotone += 1;
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generators_produce_members_of_their_families() {
        let params = FamilyParams::default();
        let mut rng = Rng::seed_from_u64(11);
        for arity in [3u8, 5, 6] {
            for _ in 0..20 {
                let a = sample(Family::Affine, arity, &params, &mut rng);
                assert!(is_affine(&a.table), "{} is not affine", a.description);
                assert!(!a.table.is_constant());
                let m = sample(Family::Monotone, arity, &params, &mut rng);
                assert!(is_monotone(&m.table), "{} is not monotone", m.description);
                for family in [
                    Family::ReadOnce,
                    Family::Threshold,
                    Family::KTermDnf,
                    Family::DecisionTree,
                ] {
                    let t = sample(family, arity, &params, &mut rng);
                    assert_eq!(t.family, family);
                    assert!(!t.table.is_constant(), "{family}: {}", t.description);
                    assert!(!t.description.is_empty());
                }
            }
        }
    }

    #[test]
    fn labels_round_trip() {
        for f in Family::ALL {
            assert_eq!(Family::parse(f.label()), Some(f));
            let json = serde_json::to_string(&f).unwrap();
            assert_eq!(json, format!("\"{}\"", f.label()));
        }
        assert_eq!(Family::parse("nope"), None);
    }

    #[test]
    fn parity_is_affine_and_dnf_is_not_always() {
        let parity = TruthTable::parse_rows(3, "01101001").unwrap();
        assert!(is_affine(&parity));
        let maj = TruthTable::parse_rows(3, "00010111").unwrap();
        assert!(!is_affine(&maj));
        assert!(is_monotone(&maj));
        assert!(!is_monotone(&parity));
    }
}
