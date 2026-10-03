//! Structural representations for the structure-keyed laboratory (ADR 0015).
//!
//! Each class the question's structure promise names has a learner that
//! knows only its own class: the affine shortcut ([`crate::AffineProbe`],
//! [`crate::AffineSolve`]), the weight profile ([`SymmetricProbe`],
//! [`SymmetricSolve`]) and the relevant-variable search ([`JuntaProbe`],
//! [`JuntaSolve`]). A solve writes the complete table only when its
//! hypothesis agrees with every observation, so a schedule can verify a
//! hypothesis with [`ProbeRandomUnobserved`] before solving, and fall
//! through to the next learner when the hypothesis is refuted.
//!
//! [`StructureProfile`] is the representation routers read: for each class,
//! the exact number of its members consistent with every observation, and
//! the posterior over the classes under the published prior.

use crate::support::{
    arity, observed_rows, observed_set, probe_for_row, result_output, rows, write_observations_view,
};
use crate::{frames, views};
use metron_core::bits::BitVector;
use metron_core::cost::Cost;
use metron_core::frame::TransformContract;
use metron_core::id::OperatorId;
use metron_core::inquiry::{Derivation, Inquiry, Representation};
use metron_core::operator::{Operator, OperatorError, OperatorKind, OperatorOutcome, OperatorSpec};
use metron_core::rng::Rng;
use metron_core::world::Oracle;
use std::collections::{BTreeMap, BTreeSet};

/// The structure promise a question publishes in its `structure` parameter.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Promise {
    /// Class names with their sizes, in published order.
    pub classes: Vec<(String, f64)>,
    /// Relevant variables of a junta.
    pub junta_size: usize,
    /// Whether constant functions are excluded.
    pub non_constant: bool,
}

impl Promise {
    /// Reads the promise from the question, if it publishes one.
    pub(crate) fn of(inquiry: &Inquiry) -> Option<Self> {
        let s = inquiry.question.params.get("structure")?;
        let classes = s
            .get("classes")?
            .as_array()?
            .iter()
            .filter_map(|c| {
                Some((
                    c.get("name")?.as_str()?.to_owned(),
                    c.get("size")?.as_f64()?,
                ))
            })
            .collect();
        Some(Self {
            classes,
            junta_size: s
                .get("junta_size")
                .and_then(serde_json::Value::as_u64)
                .and_then(|k| usize::try_from(k).ok())
                .unwrap_or(3),
            non_constant: s
                .get("non_constant")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        })
    }
}

/// Whether the question promises a non-constant target.
pub(crate) fn promises_non_constant(inquiry: &Inquiry) -> bool {
    Promise::of(inquiry).is_some_and(|p| p.non_constant)
}

fn junta_size(inquiry: &Inquiry) -> usize {
    Promise::of(inquiry).map_or(3, |p| p.junta_size)
}

fn note_output(output: Option<bool>) -> String {
    output.map_or("?".to_owned(), |b| u8::from(b).to_string())
}

// ---------------------------------------------------------------------------
// Verification.

/// Probes a uniformly random row not yet observed, drawn from the episode's
/// seeded random stream: the verification probe a learner spends before it
/// solves. One unit of work per row scanned.
#[derive(Debug, Default, Clone, Copy)]
pub struct ProbeRandomUnobserved;

impl ProbeRandomUnobserved {
    /// The operator identifier.
    pub const ID: &'static str = "probe-random-unobserved";
}

impl<W: Oracle> Operator<W> for ProbeRandomUnobserved {
    fn spec(&self) -> OperatorSpec {
        OperatorSpec::new(
            Self::ID,
            OperatorKind::Observe,
            "Probe a uniformly random row that has not been observed.",
        )
        .writes(views::OBSERVATIONS)
    }

    fn apply(
        &self,
        inquiry: &mut Inquiry,
        world: &mut W,
        rng: &mut Rng,
    ) -> Result<OperatorOutcome, OperatorError> {
        let arity = arity(inquiry)?;
        let observed = observed_set(inquiry, arity);
        let total = rows(arity);
        let work = Cost::work(total as u64);
        let open: Vec<usize> = (0..total).filter(|r| !observed.contains(r)).collect();
        if open.is_empty() {
            return Ok(OperatorOutcome::no_change(work).with_note("every row is already observed"));
        }
        if world.probes_remaining() == Some(0) {
            return Ok(
                OperatorOutcome::no_change(work).with_note("the protocol's probe cap is spent")
            );
        }
        let row = open[rng.below_usize(open.len())];
        let observation = world.probe(
            &OperatorId::from(Self::ID),
            inquiry.id,
            &probe_for_row(row, arity),
        )?;
        let output = result_output(&observation.result);
        inquiry.record_observation(observation);
        write_observations_view(inquiry, Self::ID, arity);
        Ok(OperatorOutcome::progressed(work)
            .with_note(format!("row {row} -> {}", note_output(output))))
    }
}

// ---------------------------------------------------------------------------
// The weight profile.

/// Probes one row of every Hamming weight not yet represented among the
/// observations: the row whose lowest `w` bits are set.
#[derive(Debug, Default, Clone, Copy)]
pub struct SymmetricProbe;

impl SymmetricProbe {
    /// The operator identifier.
    pub const ID: &'static str = "symmetric-probe";
}

impl<W: Oracle> Operator<W> for SymmetricProbe {
    fn spec(&self) -> OperatorSpec {
        OperatorSpec::new(
            Self::ID,
            OperatorKind::Observe,
            "Probe one row of every Hamming weight not yet observed.",
        )
        .writes(views::OBSERVATIONS)
    }

    fn apply(
        &self,
        inquiry: &mut Inquiry,
        world: &mut W,
        _rng: &mut Rng,
    ) -> Result<OperatorOutcome, OperatorError> {
        let arity = arity(inquiry)?;
        let weights: BTreeSet<u32> = observed_set(inquiry, arity)
            .into_iter()
            .map(usize::count_ones)
            .collect();
        let work = Cost::work(arity as u64 + 1);
        let mut probed = 0u64;
        let mut capped = false;
        for w in 0..=arity {
            if weights.contains(&(w as u32)) {
                continue;
            }
            if world.probes_remaining() == Some(0) {
                capped = true;
                break;
            }
            let row = (1usize << w) - 1;
            let observation = world.probe(
                &OperatorId::from(Self::ID),
                inquiry.id,
                &probe_for_row(row, arity),
            )?;
            inquiry.record_observation(observation);
            probed += 1;
        }
        if probed == 0 {
            return Ok(OperatorOutcome::no_change(work).with_note(if capped {
                "the protocol's probe cap is spent"
            } else {
                "every weight is already observed"
            }));
        }
        write_observations_view(inquiry, Self::ID, arity);
        Ok(OperatorOutcome::progressed(work).with_note(format!(
            "probed {probed} weights{}",
            if capped { " (cap reached)" } else { "" }
        )))
    }
}

/// Reads the weight profile off the observations and extrapolates it to
/// every row, unless two observations of the same weight disagree, a weight
/// is unobserved, or the profile is constant and the promise excludes
/// constants.
#[derive(Debug, Default, Clone, Copy)]
pub struct SymmetricSolve;

impl SymmetricSolve {
    /// The operator identifier.
    pub const ID: &'static str = "symmetric-solve";

    /// The contract this transform runs under.
    #[must_use]
    pub fn contract() -> TransformContract {
        TransformContract::new([frames::OBSERVATIONS], frames::TRUTH_TABLE_COMPLETE)
            .preserves("every observed row")
            .assumes("the hidden function depends only on the number of ones in its input")
            .approximate(
                "rows other than the observed ones are extrapolated from the weight profile",
            )
    }
}

impl<W> Operator<W> for SymmetricSolve {
    fn spec(&self) -> OperatorSpec {
        OperatorSpec::new(
            Self::ID,
            OperatorKind::Transform {
                contract: Self::contract(),
            },
            "Read the weight profile from the observations and extrapolate it, unless refuted.",
        )
        .reads(views::OBSERVATIONS)
        .writes(views::COMPLETE_TABLE)
    }

    fn apply(
        &self,
        inquiry: &mut Inquiry,
        _world: &mut W,
        _rng: &mut Rng,
    ) -> Result<OperatorOutcome, OperatorError> {
        let arity = arity(inquiry)?;
        let observed = observed_rows(inquiry, arity);
        let work = Cost::work(observed.len() as u64 + arity as u64 + 1);
        let mut profile: Vec<Option<bool>> = vec![None; arity + 1];
        for &(row, out) in &observed {
            let w = row.count_ones() as usize;
            match profile[w] {
                Some(prev) if prev != out => {
                    return Ok(OperatorOutcome::no_change(work).with_note(format!(
                        "symmetric hypothesis refuted: two rows of weight {w} disagree"
                    )));
                }
                _ => profile[w] = Some(out),
            }
        }
        let missing: Vec<String> = (0..=arity)
            .filter(|&w| profile[w].is_none())
            .map(|w| w.to_string())
            .collect();
        if !missing.is_empty() {
            return Ok(OperatorOutcome::no_change(work)
                .with_note(format!("weights {} unobserved", missing.join(", "))));
        }
        let g: Vec<bool> = profile.into_iter().map(|v| v.unwrap_or(false)).collect();
        if promises_non_constant(inquiry) && g.iter().all(|&b| b == g[0]) {
            return Ok(OperatorOutcome::no_change(work)
                .with_note("constant profile excluded by the promise"));
        }
        let total = rows(arity);
        let table = BitVector::from_fn(total, |row| g[row.count_ones() as usize]);
        inquiry.write_view(
            views::COMPLETE_TABLE,
            frames::TRUTH_TABLE_COMPLETE,
            Representation::Bits(table),
            Derivation::by(OperatorId::from(Self::ID), inquiry.steps)
                .from_view(views::OBSERVATIONS),
        );
        let ones: Vec<String> = (0..=arity)
            .filter(|&w| g[w])
            .map(|w| w.to_string())
            .collect();
        Ok(OperatorOutcome::progressed(work).with_note(format!(
            "symmetric hypothesis: true at weights {{{}}}, consistent with {} observations",
            ones.join(", "),
            observed.len()
        )))
    }
}

// ---------------------------------------------------------------------------
// The relevant-variable search.

/// Variables evidenced as relevant: two observed rows that differ in that
/// variable alone and disagree.
fn evidenced_relevant(known: &BTreeMap<usize, bool>, arity: usize) -> BTreeSet<usize> {
    let mut relevant = BTreeSet::new();
    for (&row, &v) in known {
        for i in 0..arity {
            if let Some(&w) = known.get(&(row ^ (1 << i)))
                && w != v
            {
                relevant.insert(i);
            }
        }
    }
    relevant
}

/// The row's bits on `vars`, packed in order.
fn project(row: usize, vars: &[usize]) -> usize {
    vars.iter()
        .enumerate()
        .fold(0, |acc, (j, &v)| acc | (((row >> v) & 1) << j))
}

/// The row that sets `vars` from `cell` and every other variable to zero.
fn embed(cell: usize, vars: &[usize]) -> usize {
    vars.iter()
        .enumerate()
        .fold(0, |acc, (j, &v)| acc | (((cell >> j) & 1) << v))
}

/// Searches for the relevant variables of a junta and probes its table.
///
/// Variables already evidenced as relevant are taken as found. For each new
/// variable it draws pairs of rows that agree on the variables found so far,
/// cycling through their assignments, until a pair disagrees; a binary
/// search between the two rows then isolates one relevant variable in
/// `log2` of their distance. It stops at `k` variables (the promise's junta
/// size, three without a promise), at more than `k` (the target is not a
/// `k`-junta, and the table is not probed), or after a bounded number of
/// fruitless pairs. Then it probes every cell of the table on the variables
/// found that no observation already covers.
#[derive(Debug, Default, Clone, Copy)]
pub struct JuntaProbe;

impl JuntaProbe {
    /// The operator identifier.
    pub const ID: &'static str = "junta-probe";
    /// Fruitless pairs drawn before the search gives up on a new variable.
    pub const PAIRS_PER_VARIABLE: usize = 12;
}

struct Prober<'a, W> {
    inquiry: &'a mut Inquiry,
    world: &'a mut W,
    known: BTreeMap<usize, bool>,
    arity: usize,
    probed: u64,
}

impl<W: Oracle> Prober<'_, W> {
    /// The value at `row`, probing it if unobserved; `None` once the cap is
    /// spent.
    fn value(&mut self, row: usize) -> Result<Option<bool>, OperatorError> {
        if let Some(&v) = self.known.get(&row) {
            return Ok(Some(v));
        }
        if self.world.probes_remaining() == Some(0) {
            return Ok(None);
        }
        let observation = self.world.probe(
            &OperatorId::from(JuntaProbe::ID),
            self.inquiry.id,
            &probe_for_row(row, self.arity),
        )?;
        let output = result_output(&observation.result);
        self.inquiry.record_observation(observation);
        self.probed += 1;
        if let Some(v) = output {
            self.known.insert(row, v);
        }
        Ok(output)
    }
}

impl<W: Oracle> Operator<W> for JuntaProbe {
    fn spec(&self) -> OperatorSpec {
        OperatorSpec::new(
            Self::ID,
            OperatorKind::Observe,
            "Find the relevant variables by binary search between disagreeing rows, then probe their table.",
        )
        .writes(views::OBSERVATIONS)
    }

    fn apply(
        &self,
        inquiry: &mut Inquiry,
        world: &mut W,
        rng: &mut Rng,
    ) -> Result<OperatorOutcome, OperatorError> {
        let arity = arity(inquiry)?;
        let k = junta_size(inquiry);
        let known: BTreeMap<usize, bool> = observed_rows(inquiry, arity).into_iter().collect();
        let mut p = Prober {
            inquiry,
            world,
            known,
            arity,
            probed: 0,
        };
        let mut work = 0u64;
        let mut capped = false;
        let mut relevant = evidenced_relevant(&p.known, arity);
        'search: while relevant.len() < k {
            let found: Vec<usize> = relevant.iter().copied().collect();
            let free: Vec<usize> = (0..arity).filter(|i| !relevant.contains(i)).collect();
            if free.is_empty() {
                break;
            }
            let mut new_variable = None;
            for attempt in 0..Self::PAIRS_PER_VARIABLE {
                let cell = attempt % (1usize << found.len());
                let base = embed(cell, &found);
                let mut x = base;
                let mut y = base;
                for &v in &free {
                    if rng.bool() {
                        x |= 1 << v;
                    }
                    if rng.bool() {
                        y |= 1 << v;
                    }
                }
                if x == y {
                    y ^= 1 << free[rng.below_usize(free.len())];
                }
                work += free.len() as u64;
                let (Some(fx), Some(fy)) = (p.value(x)?, p.value(y)?) else {
                    capped = true;
                    break 'search;
                };
                if fx == fy {
                    continue;
                }
                // f(a) = fx and f(b) != fx throughout; a and b differ only
                // in variables outside the found set.
                let (mut a, mut b) = (x, y);
                loop {
                    let differ: Vec<usize> =
                        (0..arity).filter(|&i| (a ^ b) >> i & 1 == 1).collect();
                    work += arity as u64;
                    if differ.len() == 1 {
                        new_variable = Some(differ[0]);
                        break;
                    }
                    let z = differ[..differ.len() / 2]
                        .iter()
                        .fold(a, |acc, &i| acc ^ (1 << i));
                    let Some(fz) = p.value(z)? else {
                        capped = true;
                        break 'search;
                    };
                    if fz == fx {
                        a = z;
                    } else {
                        b = z;
                    }
                }
                break;
            }
            match new_variable {
                Some(v) => {
                    relevant.insert(v);
                    relevant.extend(evidenced_relevant(&p.known, arity));
                    work += (p.known.len() * arity) as u64;
                }
                None => break,
            }
        }
        let mut cells_probed = 0u64;
        if relevant.len() <= k && !capped {
            let vars: Vec<usize> = relevant.iter().copied().collect();
            let covered: BTreeSet<usize> = p.known.keys().map(|&row| project(row, &vars)).collect();
            for cell in 0..(1usize << vars.len()) {
                if covered.contains(&cell) {
                    continue;
                }
                let before = p.probed;
                if p.value(embed(cell, &vars))?.is_none() {
                    capped = true;
                    break;
                }
                cells_probed += p.probed - before;
            }
        }
        let probed = p.probed;
        let names: Vec<String> = relevant.iter().map(|v| format!("x{v}")).collect();
        let note = format!(
            "relevant {{{}}} (k = {k}){}; probed {probed} rows, {cells_probed} of them table cells{}",
            names.join(", "),
            if relevant.len() > k {
                ": more than k, not a junta"
            } else {
                ""
            },
            if capped { " (cap reached)" } else { "" }
        );
        let cost = Cost::work(work.max(1) + probed * arity as u64);
        if probed == 0 {
            return Ok(OperatorOutcome::no_change(cost).with_note(note));
        }
        write_observations_view(p.inquiry, Self::ID, arity);
        Ok(OperatorOutcome::progressed(cost).with_note(note))
    }
}

/// Builds the table on the evidenced relevant variables and extrapolates it
/// to every row, unless more than `k` variables are evidenced, two
/// observations that agree on those variables disagree, a cell is
/// unobserved, or the table is constant and the promise excludes
/// constants.
#[derive(Debug, Default, Clone, Copy)]
pub struct JuntaSolve;

impl JuntaSolve {
    /// The operator identifier.
    pub const ID: &'static str = "junta-solve";

    /// The contract this transform runs under.
    #[must_use]
    pub fn contract() -> TransformContract {
        TransformContract::new([frames::OBSERVATIONS], frames::TRUTH_TABLE_COMPLETE)
            .preserves("every observed row")
            .assumes("the hidden function depends only on the relevant variables evidenced among the observations, at most the promised number")
            .approximate("rows other than the observed ones are extrapolated from the table on the relevant variables")
    }
}

impl<W> Operator<W> for JuntaSolve {
    fn spec(&self) -> OperatorSpec {
        OperatorSpec::new(
            Self::ID,
            OperatorKind::Transform {
                contract: Self::contract(),
            },
            "Build the table on the evidenced relevant variables and extrapolate it, unless refuted.",
        )
        .reads(views::OBSERVATIONS)
        .writes(views::COMPLETE_TABLE)
    }

    fn apply(
        &self,
        inquiry: &mut Inquiry,
        _world: &mut W,
        _rng: &mut Rng,
    ) -> Result<OperatorOutcome, OperatorError> {
        let arity = arity(inquiry)?;
        let k = junta_size(inquiry);
        let known: BTreeMap<usize, bool> = observed_rows(inquiry, arity).into_iter().collect();
        let work = Cost::work((known.len() * (arity + 1)) as u64);
        let vars: Vec<usize> = evidenced_relevant(&known, arity).into_iter().collect();
        if vars.len() > k {
            return Ok(OperatorOutcome::no_change(work).with_note(format!(
                "junta hypothesis refuted: {} relevant variables evidenced, more than {k}",
                vars.len()
            )));
        }
        let mut cells: Vec<Option<bool>> = vec![None; 1 << vars.len()];
        for (&row, &v) in &known {
            let c = project(row, &vars);
            match cells[c] {
                Some(prev) if prev != v => {
                    return Ok(OperatorOutcome::no_change(work).with_note(
                        "junta hypothesis refuted: two rows that agree on the relevant variables disagree",
                    ));
                }
                _ => cells[c] = Some(v),
            }
        }
        if cells.iter().any(Option::is_none) {
            return Ok(OperatorOutcome::no_change(work).with_note("table cells unobserved"));
        }
        let table_cells: Vec<bool> = cells.into_iter().map(|c| c.unwrap_or(false)).collect();
        if promises_non_constant(inquiry) && table_cells.iter().all(|&b| b == table_cells[0]) {
            return Ok(OperatorOutcome::no_change(work)
                .with_note("constant table excluded by the promise"));
        }
        let total = rows(arity);
        let table = BitVector::from_fn(total, |row| table_cells[project(row, &vars)]);
        inquiry.write_view(
            views::COMPLETE_TABLE,
            frames::TRUTH_TABLE_COMPLETE,
            Representation::Bits(table),
            Derivation::by(OperatorId::from(Self::ID), inquiry.steps)
                .from_view(views::OBSERVATIONS),
        );
        let names: Vec<String> = vars.iter().map(|v| format!("x{v}")).collect();
        Ok(OperatorOutcome::progressed(work).with_note(format!(
            "junta hypothesis on {{{}}}, consistent with {} observations",
            names.join(", "),
            known.len()
        )))
    }
}

// ---------------------------------------------------------------------------
// The structure profile.

/// Non-constant affine functions consistent with the observations, by
/// Gaussian elimination over GF(2) on `(x, 1) · (a, b) = y`.
pub(crate) fn count_affine(arity: usize, observed: &[(usize, bool)]) -> f64 {
    // Each basis entry: (pivot bit, equation mask over n + 1 unknowns, rhs).
    let mut basis: Vec<(usize, u64, bool)> = Vec::new();
    for &(row, y) in observed {
        let mut mask = (row as u64) | (1u64 << arity);
        let mut rhs = y;
        for &(pivot, m, r) in &basis {
            if (mask >> pivot) & 1 == 1 {
                mask ^= m;
                rhs ^= r;
            }
        }
        if mask == 0 {
            if rhs {
                return 0.0;
            }
            continue;
        }
        let pivot = mask.trailing_zeros() as usize;
        basis.push((pivot, mask, rhs));
    }
    let solutions = 2f64.powi((arity + 1 - basis.len()) as i32);
    let constants = [false, true]
        .into_iter()
        .filter(|&b| observed.iter().all(|&(_, y)| y == b))
        .count() as f64;
    solutions - constants
}

/// Symmetric functions that are neither constant nor affine consistent with
/// the observations.
pub(crate) fn count_symmetric(arity: usize, observed: &[(usize, bool)]) -> f64 {
    let mut profile: Vec<Option<bool>> = vec![None; arity + 1];
    for &(row, y) in observed {
        let w = row.count_ones() as usize;
        match profile[w] {
            Some(prev) if prev != y => return 0.0,
            _ => profile[w] = Some(y),
        }
    }
    let free = profile.iter().filter(|p| p.is_none()).count();
    let total = 2f64.powi(free as i32);
    let excluded = [
        |_: usize| false,
        |_: usize| true,
        |w: usize| w % 2 == 1,
        |w: usize| w.is_multiple_of(2),
    ]
    .iter()
    .filter(|g| (0..=arity).all(|w| profile[w].is_none_or(|v| v == g(w))))
    .count() as f64;
    total - excluded
}

/// Tables on `k` variables that depend on all `k` and are not affine, as
/// bit masks over the `2^k` cells (`k <= 6`).
fn valid_junta_tables(k: usize) -> Vec<u64> {
    let cells = 1usize << k;
    let space: u64 = if cells == 64 {
        u64::MAX
    } else {
        (1u64 << cells) - 1
    };
    let bit = |t: u64, c: usize| (t >> c) & 1 == 1;
    let mut out = Vec::new();
    let mut t: u64 = 0;
    loop {
        let depends_on_all = (0..k).all(|j| (0..cells).any(|c| bit(t, c) != bit(t, c ^ (1 << j))));
        if depends_on_all {
            let f0 = bit(t, 0);
            let affine =
                (0..cells).all(|x| (0..cells).all(|y| bit(t, x ^ y) ^ f0 == bit(t, x) ^ bit(t, y)));
            if !affine {
                out.push(t);
            }
        }
        if t == space {
            break;
        }
        t += 1;
    }
    out
}

/// Functions with exactly `k` relevant variables that are not affine,
/// consistent with the observations: summed over the variable sets, since
/// such a function has exactly one. `None` for `k > 4`.
pub(crate) fn count_junta(arity: usize, k: usize, observed: &[(usize, bool)]) -> Option<f64> {
    if k == 0 || k > 4 || k > arity {
        return if k > 4 { None } else { Some(0.0) };
    }
    let tables = valid_junta_tables(k);
    let mut total = 0f64;
    let mut set: u32 = (1 << k) - 1;
    let limit: u32 = 1 << arity;
    while set < limit {
        let vars: Vec<usize> = (0..arity).filter(|&i| (set >> i) & 1 == 1).collect();
        let mut fixed_mask = 0u64;
        let mut fixed_vals = 0u64;
        let mut consistent = true;
        for &(row, y) in observed {
            let c = project(row, &vars);
            let b = 1u64 << c;
            if fixed_mask & b != 0 {
                if (fixed_vals & b != 0) != y {
                    consistent = false;
                    break;
                }
            } else {
                fixed_mask |= b;
                if y {
                    fixed_vals |= b;
                }
            }
        }
        if consistent {
            total += tables
                .iter()
                .filter(|&&t| (t ^ fixed_vals) & fixed_mask == 0)
                .count() as f64;
        }
        // Next subset with the same number of bits (Gosper's hack).
        let c = set & set.wrapping_neg();
        let r = set + c;
        set = (((r ^ set) >> 2) / c) | r;
    }
    Some(total)
}

/// For each class of the structure promise, the number of its members
/// consistent with every observation, and the posterior over the classes
/// under the published prior (uniform over classes, then over members).
#[derive(Debug, Default, Clone, Copy)]
pub struct StructureProfile;

impl StructureProfile {
    /// The operator identifier.
    pub const ID: &'static str = "structure-profile";

    /// The contract this transform runs under.
    #[must_use]
    pub fn contract() -> TransformContract {
        TransformContract::new([frames::OBSERVATIONS], frames::STRUCTURE_PROFILE)
            .preserves(
                "the number of members of each promised class consistent with every observation",
            )
            .loses("which members they are, and the order of the observations")
            .assumes("the classes, sizes and prior of the question's structure promise")
    }
}

impl<W> Operator<W> for StructureProfile {
    fn spec(&self) -> OperatorSpec {
        OperatorSpec::new(
            Self::ID,
            OperatorKind::Transform {
                contract: Self::contract(),
            },
            "Count each promised class's members consistent with the observations; the posterior over classes.",
        )
        .reads(views::OBSERVATIONS)
        .writes(views::STRUCTURE_PROFILE)
    }

    fn applicable(&self, inquiry: &Inquiry, _world: &W) -> bool {
        Promise::of(inquiry).is_some() && inquiry.has_view(&views::OBSERVATIONS.into())
    }

    fn apply(
        &self,
        inquiry: &mut Inquiry,
        _world: &mut W,
        _rng: &mut Rng,
    ) -> Result<OperatorOutcome, OperatorError> {
        let arity = arity(inquiry)?;
        let promise = Promise::of(inquiry).ok_or(OperatorError::NotApplicable)?;
        let observed = observed_rows(inquiry, arity);
        let mut entries = Vec::new();
        for (name, size) in &promise.classes {
            let consistent = match name.as_str() {
                "affine" => Some(count_affine(arity, &observed)),
                "symmetric" => Some(count_symmetric(arity, &observed)),
                "junta" => count_junta(arity, promise.junta_size, &observed),
                _ => None,
            };
            let Some(consistent) = consistent else {
                return Ok(OperatorOutcome::failed(
                    Cost::work(1),
                    format!("no counting rule for the class `{name}`"),
                ));
            };
            entries.push((name.clone(), consistent, *size));
        }
        let likelihoods: Vec<f64> = entries
            .iter()
            .map(|&(_, c, n)| if n > 0.0 { c / n } else { 0.0 })
            .collect();
        let total: f64 = likelihoods.iter().sum();
        let classes: Vec<serde_json::Value> = entries
            .iter()
            .zip(&likelihoods)
            .map(|((name, consistent, size), &l)| {
                serde_json::json!({
                    "name": name,
                    "consistent": consistent,
                    "size": size,
                    "posterior": if total > 0.0 { l / total } else { 0.0 },
                })
            })
            .collect();
        let summary: Vec<String> = entries
            .iter()
            .zip(&likelihoods)
            .map(|((name, c, _), &l)| {
                format!(
                    "{name} {c} ({:.3})",
                    if total > 0.0 { l / total } else { 0.0 }
                )
            })
            .collect();
        inquiry.write_view(
            views::STRUCTURE_PROFILE,
            frames::STRUCTURE_PROFILE,
            Representation::Json(serde_json::json!({
                "observations": observed.len(),
                "classes": classes,
            })),
            Derivation::by(OperatorId::from(Self::ID), inquiry.steps)
                .from_view(views::OBSERVATIONS),
        );
        let subsets = (1..=promise.junta_size.min(arity))
            .fold(1u64, |acc, i| acc * (arity + 1 - i) as u64 / i as u64);
        let work = (observed.len() as u64 + 1) * (arity as u64 + 2) * (subsets + 2);
        Ok(OperatorOutcome::progressed(Cost::work(work))
            .with_note(format!("consistent members: {}", summary.join(", "))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_affine(f: &[bool]) -> bool {
        let f0 = f[0];
        (0..f.len()).all(|x| (0..f.len()).all(|y| f[x ^ y] ^ f0 == f[x] ^ f[y]))
    }

    fn is_symmetric(f: &[bool]) -> bool {
        (0..f.len()).all(|x| (0..f.len()).all(|y| x.count_ones() != y.count_ones() || f[x] == f[y]))
    }

    fn relevant(f: &[bool], arity: usize) -> usize {
        (0..arity)
            .filter(|&i| (0..f.len()).any(|x| f[x] != f[x ^ (1 << i)]))
            .count()
    }

    #[test]
    fn class_counts_match_brute_force_enumeration() {
        // Prior: the three counting rules are exact. Checked against every
        // function on four inputs for random observation sets, including
        // sets that refute a class.
        let arity = 4;
        let cells = 16;
        let functions: Vec<Vec<bool>> = (0..(1u32 << cells))
            .map(|bits| (0..cells).map(|r| (bits >> r) & 1 == 1).collect())
            .collect();
        let mut rng = Rng::seed_from_u64(91);
        for trial in 0..60 {
            let m = trial % 9;
            let target = &functions[rng.below_usize(functions.len())];
            let mut rows_seen: Vec<usize> = (0..cells).collect();
            rng.shuffle(&mut rows_seen);
            let observed: Vec<(usize, bool)> = rows_seen[..m]
                .iter()
                .map(|&r| {
                    (
                        r,
                        if trial % 7 == 0 {
                            rng.bool()
                        } else {
                            target[r]
                        },
                    )
                })
                .collect();
            let consistent = |f: &Vec<bool>| observed.iter().all(|&(r, y)| f[r] == y);
            let non_constant = |f: &Vec<bool>| f.iter().any(|&b| b != f[0]);
            let affine = functions
                .iter()
                .filter(|f| consistent(f) && non_constant(f) && is_affine(f))
                .count() as f64;
            let symmetric = functions
                .iter()
                .filter(|f| consistent(f) && non_constant(f) && is_symmetric(f) && !is_affine(f))
                .count() as f64;
            let junta3 = functions
                .iter()
                .filter(|f| consistent(f) && relevant(f, arity) == 3 && !is_affine(f))
                .count() as f64;
            let junta2 = functions
                .iter()
                .filter(|f| consistent(f) && relevant(f, arity) == 2 && !is_affine(f))
                .count() as f64;
            assert_eq!(
                count_affine(arity, &observed),
                affine,
                "affine, {observed:?}"
            );
            assert_eq!(
                count_symmetric(arity, &observed),
                symmetric,
                "symmetric, {observed:?}"
            );
            assert_eq!(
                count_junta(arity, 3, &observed),
                Some(junta3),
                "junta 3, {observed:?}"
            );
            assert_eq!(
                count_junta(arity, 2, &observed),
                Some(junta2),
                "junta 2, {observed:?}"
            );
        }
        assert_eq!(count_affine(8, &[]), 510.0);
        assert_eq!(count_symmetric(8, &[]), 508.0);
        assert_eq!(count_junta(8, 3, &[]), Some(12_096.0));
    }

    #[test]
    fn projections_round_trip() {
        let vars = [1usize, 4, 6];
        for cell in 0..8 {
            assert_eq!(project(embed(cell, &vars), &vars), cell);
        }
        let mut known = BTreeMap::new();
        known.insert(0b000, false);
        known.insert(0b010, true);
        known.insert(0b011, true);
        assert_eq!(evidenced_relevant(&known, 3), BTreeSet::from([1]));
    }
}
