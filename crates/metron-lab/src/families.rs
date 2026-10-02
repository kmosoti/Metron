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
    /// Functions of the number of ones only: not constant, not affine.
    Symmetric,
    /// Functions with exactly `junta_size` relevant variables, not affine.
    Junta,
}

impl Family {
    /// The six generator families of the mixed laboratory (M1 to M3 on the
    /// family-labelled task sets), in canonical order. Their labels name a
    /// generator, not a property: a monotone function can also be a
    /// threshold function.
    pub const ALL: [Family; 6] = [
        Family::Affine,
        Family::Monotone,
        Family::ReadOnce,
        Family::Threshold,
        Family::KTermDnf,
        Family::DecisionTree,
    ];

    /// The structure-keyed classes of ADR 0015: each a property of the
    /// function, disjoint by construction, each with an exact size.
    pub const STRUCTURE_KEYED: [Family; 3] = [Family::Affine, Family::Symmetric, Family::Junta];

    /// Every family, mixed and structure-keyed.
    pub const EVERY: [Family; 8] = [
        Family::Affine,
        Family::Monotone,
        Family::ReadOnce,
        Family::Threshold,
        Family::KTermDnf,
        Family::DecisionTree,
        Family::Symmetric,
        Family::Junta,
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
            Family::Symmetric => "symmetric",
            Family::Junta => "junta",
        }
    }

    /// Parses a label.
    #[must_use]
    pub fn parse(label: &str) -> Option<Family> {
        Family::EVERY.into_iter().find(|f| f.label() == label)
    }

    /// The class's definition as the question publishes it, for the
    /// structure-keyed classes.
    #[must_use]
    pub fn definition(self, params: &FamilyParams) -> Option<String> {
        match self {
            Family::Affine => {
                Some("a·x ⊕ b over GF(2) with a ≠ 0 (parity and its complement included)".into())
            }
            Family::Symmetric => {
                Some("depends only on the number of ones in x; not constant and not affine".into())
            }
            Family::Junta => Some(format!(
                "exactly {} relevant variables; not affine",
                params.junta_size
            )),
            _ => None,
        }
    }

    /// Exact number of functions in the class on `arity` inputs, for the
    /// structure-keyed classes; `None` for generator families, whose
    /// sampling distribution is not uniform over a defined class.
    #[must_use]
    pub fn class_size(self, arity: u8, params: &FamilyParams) -> Option<f64> {
        let n = i32::from(arity);
        match self {
            Family::Affine => Some(2.0 * (2f64.powi(n) - 1.0)),
            Family::Symmetric => Some((2f64.powi(n + 1) - 4.0).max(0.0)),
            Family::Junta => {
                let k = params.junta_size;
                if k == 0 || k > usize::from(arity) {
                    return Some(0.0);
                }
                let affine_on_all = 2.0;
                Some(binomial(usize::from(arity), k) * (depends_on_all(k) - affine_on_all))
            }
            _ => None,
        }
    }
}

/// `C(n, k)` as a float.
fn binomial(n: usize, k: usize) -> f64 {
    if k > n {
        return 0.0;
    }
    (0..k).fold(1.0, |acc, i| acc * (n - i) as f64 / (i + 1) as f64)
}

/// Number of Boolean functions of `k` variables that depend on all `k`, by
/// inclusion and exclusion over the variables they ignore.
fn depends_on_all(k: usize) -> f64 {
    (0..=k)
        .map(|j| {
            let sign = if (k - j).is_multiple_of(2) { 1.0 } else { -1.0 };
            sign * binomial(k, j) * 2f64.powf(2f64.powi(j as i32))
        })
        .sum()
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
    /// Relevant variables of a `junta` function. Serialised only when it
    /// differs from the default, so manifests written before it existed
    /// keep their hashes.
    #[serde(
        default = "default_junta_size",
        skip_serializing_if = "is_default_junta_size"
    )]
    pub junta_size: usize,
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
const fn default_junta_size() -> usize {
    3
}
#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_default_junta_size(k: &usize) -> bool {
    *k == default_junta_size()
}

impl Default for FamilyParams {
    fn default() -> Self {
        Self {
            k_terms: default_k_terms(),
            term_width: default_term_width(),
            tree_depth: default_tree_depth(),
            minimal_points: default_minimal_points(),
            max_weight: default_max_weight(),
            junta_size: default_junta_size(),
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
            Family::Symmetric => symmetric(arity, rng),
            Family::Junta => junta(arity, params, rng),
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

/// Uniform over symmetric functions that are neither constant nor affine:
/// the weight profile `g` is drawn uniformly and redrawn when it is
/// constant or alternating (parity and its complement).
fn symmetric(arity: u8, rng: &mut Rng) -> Target {
    let n = usize::from(arity);
    if n < 2 {
        // No symmetric function on one input is both non-constant and
        // non-affine; fall back to the affine generator.
        return affine(arity, rng);
    }
    let profile: Vec<bool> = loop {
        let g: Vec<bool> = (0..=n).map(|_| rng.bool()).collect();
        let constant = g.iter().all(|&b| b == g[0]);
        let alternating = g.windows(2).all(|w| w[0] != w[1]);
        if !constant && !alternating {
            break g;
        }
    };
    let table = TruthTable::from_fn(arity, |row| profile[row.count_ones() as usize])
        .expect("arity validated by caller");
    let ones: Vec<String> = (0..=n)
        .filter(|&w| profile[w])
        .map(|w| w.to_string())
        .collect();
    Target {
        table,
        family: Family::Symmetric,
        description: format!(
            "true exactly when the number of ones is in {{{}}}",
            ones.join(", ")
        ),
    }
}

/// Uniform over functions with exactly `junta_size` relevant variables that
/// are not affine: a uniformly random set of variables and a uniformly
/// random table on them, redrawn when the table ignores a variable or is
/// affine.
fn junta(arity: u8, params: &FamilyParams, rng: &mut Rng) -> Target {
    let n = usize::from(arity);
    let k = params.junta_size.clamp(1, n);
    let mut vars: Vec<usize> = (0..n).collect();
    rng.shuffle(&mut vars);
    let mut chosen: Vec<usize> = vars[..k].to_vec();
    chosen.sort_unstable();
    let cells = 1usize << k;
    let inner: Vec<bool> = loop {
        let t: Vec<bool> = (0..cells).map(|_| rng.bool()).collect();
        let depends_on_all = (0..k).all(|j| (0..cells).any(|c| t[c] != t[c ^ (1 << j)]));
        let f0 = t[0];
        let affine = (0..cells).all(|x| (0..cells).all(|y| t[x ^ y] ^ f0 == t[x] ^ t[y]));
        if depends_on_all && !affine {
            break t;
        }
    };
    let project = |row: usize| -> usize {
        chosen
            .iter()
            .enumerate()
            .fold(0, |acc, (j, &v)| acc | (((row >> v) & 1) << j))
    };
    let table =
        TruthTable::from_fn(arity, |row| inner[project(row)]).expect("arity validated by caller");
    let names: Vec<String> = chosen.iter().map(|&v| var(v)).collect();
    let bits: String = inner.iter().map(|&b| if b { '1' } else { '0' }).collect();
    Target {
        table,
        family: Family::Junta,
        description: format!("on {{{}}} with table {bits}", names.join(", ")),
    }
}

/// Whether `table` depends only on the number of ones in its input.
#[must_use]
pub fn is_symmetric(table: &TruthTable) -> bool {
    let n = usize::from(table.arity());
    let mut seen: Vec<Option<bool>> = vec![None; n + 1];
    (0..table.rows()).all(|row| {
        let w = row.count_ones() as usize;
        let v = table.eval(row);
        match seen[w] {
            Some(prev) => prev == v,
            None => {
                seen[w] = Some(v);
                true
            }
        }
    })
}

/// The variables `table` depends on, in increasing order.
#[must_use]
pub fn relevant_variables(table: &TruthTable) -> Vec<usize> {
    let n = usize::from(table.arity());
    (0..n)
        .filter(|&i| (0..table.rows()).any(|row| table.eval(row) != table.eval(row ^ (1 << i))))
        .collect()
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
    fn structure_keyed_generators_produce_disjoint_class_members() {
        let params = FamilyParams::default();
        let mut rng = Rng::seed_from_u64(5);
        for arity in [4u8, 6, 8] {
            for _ in 0..40 {
                let a = sample(Family::Affine, arity, &params, &mut rng);
                assert!(is_affine(&a.table) && !a.table.is_constant());
                let s = sample(Family::Symmetric, arity, &params, &mut rng);
                assert_eq!(s.family, Family::Symmetric);
                assert!(is_symmetric(&s.table), "{}", s.description);
                assert!(!is_affine(&s.table) && !s.table.is_constant());
                let j = sample(Family::Junta, arity, &params, &mut rng);
                assert_eq!(j.family, Family::Junta);
                assert_eq!(relevant_variables(&j.table).len(), 3, "{}", j.description);
                assert!(!is_affine(&j.table));
            }
        }
    }

    #[test]
    fn class_sizes_match_exhaustive_enumeration() {
        // Prior: the closed forms for the class sizes. Checked by
        // classifying every function on four inputs.
        let params = FamilyParams::default();
        let arity = 4u8;
        let (mut affine, mut symmetric, mut junta) = (0usize, 0usize, 0usize);
        for bits in 0..(1u32 << 16) {
            let t = TruthTable::from_fn(arity, |row| (bits >> row) & 1 == 1).unwrap();
            if t.is_constant() {
                continue;
            }
            let aff = is_affine(&t);
            if aff {
                affine += 1;
            }
            if is_symmetric(&t) && !aff {
                symmetric += 1;
            }
            if relevant_variables(&t).len() == 3 && !aff {
                junta += 1;
            }
        }
        assert_eq!(
            Family::Affine.class_size(arity, &params),
            Some(affine as f64)
        );
        assert_eq!(
            Family::Symmetric.class_size(arity, &params),
            Some(symmetric as f64)
        );
        assert_eq!(Family::Junta.class_size(arity, &params), Some(junta as f64));
        assert_eq!((affine, symmetric, junta), (30, 28, 864));
        assert_eq!(Family::Junta.class_size(8, &params), Some(12_096.0));
        assert_eq!(Family::Affine.class_size(8, &params), Some(510.0));
        assert_eq!(Family::Symmetric.class_size(8, &params), Some(508.0));
        assert_eq!(depends_on_all(4), 64_594.0);
        assert_eq!(Family::Monotone.class_size(8, &params), None);
    }

    #[test]
    fn structure_keyed_generators_are_uniform_over_their_classes() {
        // Prior: rejection sampling from a uniform proposal is uniform over
        // the accepted set. Chi-square over the 28 symmetric functions on
        // four inputs and over the 30 non-constant affine functions.
        let params = FamilyParams::default();
        let mut rng = Rng::seed_from_u64(77);
        for (family, classes) in [(Family::Symmetric, 28usize), (Family::Affine, 30)] {
            let draws = 28_000usize;
            let mut counts: std::collections::BTreeMap<String, usize> =
                std::collections::BTreeMap::new();
            for _ in 0..draws {
                let t = sample(family, 4, &params, &mut rng).table.to_rows_string();
                *counts.entry(t).or_insert(0) += 1;
            }
            assert_eq!(counts.len(), classes, "{family}: every member is drawn");
            let expected = draws as f64 / classes as f64;
            let chi2: f64 = counts
                .values()
                .map(|&c| (c as f64 - expected).powi(2) / expected)
                .sum();
            // 0.1% critical value for 27 or 29 degrees of freedom is about 56.
            assert!(chi2 < 56.0, "{family}: chi-square {chi2:.1}");
        }
    }

    #[test]
    fn labels_round_trip() {
        for f in Family::EVERY {
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
