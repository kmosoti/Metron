//! The retrieval workload for milestone M5.
//!
//! Given `k` observed rows of a target, retrieve the previously solved
//! functions in a store that agree with them. Four methods are compared on
//! recall, bytes per stored entry and nanoseconds per query:
//!
//! * [`ExactScan`] — test every stored table against the observations;
//! * [`BitmaskIndex`] — one bitset per (row, value) over the store, ANDed
//!   across the observations;
//! * [`BloomPerTable`] — a Bloom filter of (row, value) pairs per table,
//!   queried pair by pair (no false negatives, some false positives);
//! * [`HdcBundle`] — binary spatter codes: each table is the majority
//!   bundle of `bind(row_code, value_code)` over its rows, a query is the
//!   bundle over the observed pairs, and results are ranked by Hamming
//!   similarity.
//!
//! The exact methods return exactly the consistent set; the sketches are
//! scored by how much of that set appears in their top `|answer|` and top
//! `2 |answer|` results. Wall-clock time is the measured quantity here, by
//! design; it never enters a decision the system makes.

use crate::families::{Family, FamilyParams, sample};
use crate::truth_table::TruthTable;
use metron_core::bits::BitVector;
use metron_core::hash::ContentHash;
use metron_core::rng::Rng;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::time::Instant;

/// How to run the workload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetrievalSpec {
    /// Number of inputs.
    pub arity: u8,
    /// Families to fill the store from.
    pub families: Vec<Family>,
    /// Tables sampled per family (fewer after de-duplication).
    pub store_per_family: usize,
    /// Queries per setting.
    #[serde(default = "default_queries")]
    pub queries: usize,
    /// Numbers of observed rows to test.
    #[serde(default = "default_observed")]
    pub observed_rows: Vec<usize>,
    /// Hypervector dimensions to test.
    #[serde(default = "default_dimensions")]
    pub dimensions: Vec<usize>,
    /// Bloom filter bits per stored pair.
    #[serde(default = "default_bloom_bits")]
    pub bloom_bits_per_pair: usize,
    /// Generator knobs.
    #[serde(default)]
    pub params: FamilyParams,
}

const fn default_queries() -> usize {
    200
}
fn default_observed() -> Vec<usize> {
    vec![4, 8, 16]
}
fn default_dimensions() -> Vec<usize> {
    vec![1024, 4096, 8192]
}
const fn default_bloom_bits() -> usize {
    8
}

/// A store of solved functions.
#[derive(Clone, Debug)]
pub struct Store {
    arity: u8,
    tables: Vec<BitVector>,
}

impl Store {
    /// Samples a de-duplicated store.
    #[must_use]
    pub fn build(spec: &RetrievalSpec, rng: &mut Rng) -> Self {
        let mut seen = BTreeSet::new();
        let mut tables = Vec::new();
        for &family in &spec.families {
            let mut attempts = 0;
            let mut added = 0;
            while added < spec.store_per_family && attempts < spec.store_per_family * 10 {
                attempts += 1;
                let t = sample(family, spec.arity, &spec.params, rng).table;
                if seen.insert(t.to_rows_string()) {
                    tables.push(t.bits().clone());
                    added += 1;
                }
            }
        }
        Self {
            arity: spec.arity,
            tables,
        }
    }

    /// Number of inputs.
    #[must_use]
    pub const fn arity(&self) -> u8 {
        self.arity
    }

    /// Rows per table.
    #[must_use]
    pub const fn rows(&self) -> usize {
        TruthTable::rows_for(self.arity)
    }

    /// Number of tables.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tables.len()
    }

    /// Whether the store is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }

    /// The tables.
    #[must_use]
    pub fn tables(&self) -> &[BitVector] {
        &self.tables
    }
}

/// A query: observed rows of some stored target, and the exact answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    /// Observed `(row, value)` pairs.
    pub observed: Vec<(usize, bool)>,
    /// Indices of every stored table consistent with the observations.
    pub answer: BTreeSet<usize>,
}

impl Query {
    /// Samples a query of `k` rows of a random stored target.
    #[must_use]
    pub fn sample(store: &Store, k: usize, rng: &mut Rng) -> Self {
        let target = rng.below_usize(store.len());
        let mut rows: Vec<usize> = (0..store.rows()).collect();
        rng.shuffle(&mut rows);
        let observed: Vec<(usize, bool)> = rows[..k.min(rows.len())]
            .iter()
            .map(|&r| (r, store.tables[target].bit(r)))
            .collect();
        let answer = store
            .tables
            .iter()
            .enumerate()
            .filter(|(_, t)| observed.iter().all(|&(r, v)| t.bit(r) == v))
            .map(|(i, _)| i)
            .collect();
        Self { observed, answer }
    }
}

/// A retrieval method.
pub trait Method {
    /// Name for reports.
    fn name(&self) -> String;
    /// Bytes of index per stored entry.
    fn bytes_per_entry(&self) -> f64;
    /// Candidate indices, best first; may be a superset or a ranking.
    fn retrieve(&self, query: &Query, limit: usize) -> Vec<usize>;
}

/// Tests every table against the observations.
pub struct ExactScan<'a> {
    store: &'a Store,
}

impl<'a> ExactScan<'a> {
    /// Builds the method.
    #[must_use]
    pub const fn new(store: &'a Store) -> Self {
        Self { store }
    }
}

impl Method for ExactScan<'_> {
    fn name(&self) -> String {
        "exact-scan".into()
    }

    fn bytes_per_entry(&self) -> f64 {
        (self.store.rows() as f64 / 8.0).max(1.0)
    }

    fn retrieve(&self, query: &Query, limit: usize) -> Vec<usize> {
        self.store
            .tables
            .iter()
            .enumerate()
            .filter(|(_, t)| query.observed.iter().all(|&(r, v)| t.bit(r) == v))
            .map(|(i, _)| i)
            .take(limit)
            .collect()
    }
}

/// One bitset over the store per (row, value).
pub struct BitmaskIndex {
    rows: usize,
    len: usize,
    /// `masks[row * 2 + value]`.
    masks: Vec<BitVector>,
}

impl BitmaskIndex {
    /// Builds the index.
    #[must_use]
    pub fn build(store: &Store) -> Self {
        let rows = store.rows();
        let len = store.len();
        let masks = (0..rows * 2)
            .map(|key| {
                let (row, value) = (key / 2, key % 2 == 1);
                BitVector::from_fn(len, |i| store.tables[i].bit(row) == value)
            })
            .collect();
        Self { rows, len, masks }
    }
}

impl Method for BitmaskIndex {
    fn name(&self) -> String {
        "bitmask-index".into()
    }

    fn bytes_per_entry(&self) -> f64 {
        (self.rows * 2) as f64 / 8.0
    }

    fn retrieve(&self, query: &Query, limit: usize) -> Vec<usize> {
        let mut acc = BitVector::ones(self.len);
        for &(row, value) in &query.observed {
            acc = acc.and(&self.masks[row * 2 + usize::from(value)]);
        }
        acc.ones_iter().take(limit).collect()
    }
}

/// A Bloom filter of (row, value) pairs per table.
pub struct BloomPerTable {
    bits: usize,
    hashes: usize,
    filters: Vec<BitVector>,
}

impl BloomPerTable {
    fn positions(bits: usize, hashes: usize, row: usize, value: bool) -> Vec<usize> {
        let key = ((row as u64) << 1) | u64::from(value);
        let h = ContentHash::of_bytes(&key.to_le_bytes());
        let a = u64::from_le_bytes(h.0[..8].try_into().expect("8 bytes"));
        let b = u64::from_le_bytes(h.0[8..16].try_into().expect("8 bytes")) | 1;
        (0..hashes as u64)
            .map(|i| (a.wrapping_add(i.wrapping_mul(b)) % bits as u64) as usize)
            .collect()
    }

    /// Builds filters with `bits_per_pair * rows` bits each.
    #[must_use]
    pub fn build(store: &Store, bits_per_pair: usize) -> Self {
        let rows = store.rows();
        let bits = (bits_per_pair * rows).max(8);
        let hashes = ((bits as f64 / rows as f64) * std::f64::consts::LN_2)
            .round()
            .max(1.0) as usize;
        let filters = store
            .tables
            .iter()
            .map(|t| {
                let mut f = BitVector::zeros(bits);
                for row in 0..rows {
                    for p in Self::positions(bits, hashes, row, t.bit(row)) {
                        f.set(p, true).expect("in range");
                    }
                }
                f
            })
            .collect();
        Self {
            bits,
            hashes,
            filters,
        }
    }
}

impl Method for BloomPerTable {
    fn name(&self) -> String {
        format!(
            "bloom-per-table({} bits, {} hashes)",
            self.bits, self.hashes
        )
    }

    fn bytes_per_entry(&self) -> f64 {
        self.bits as f64 / 8.0
    }

    fn retrieve(&self, query: &Query, limit: usize) -> Vec<usize> {
        let positions: Vec<Vec<usize>> = query
            .observed
            .iter()
            .map(|&(r, v)| Self::positions(self.bits, self.hashes, r, v))
            .collect();
        self.filters
            .iter()
            .enumerate()
            .filter(|(_, f)| positions.iter().all(|ps| ps.iter().all(|&p| f.bit(p))))
            .map(|(i, _)| i)
            .take(limit)
            .collect()
    }
}

/// Binary spatter codes over (row, value) pairs.
pub struct HdcBundle {
    dim: usize,
    row_codes: Vec<BitVector>,
    value_codes: [BitVector; 2],
    codes: Vec<BitVector>,
}

impl HdcBundle {
    fn random(dim: usize, rng: &mut Rng) -> BitVector {
        let words = (0..BitVector::words_for(dim))
            .map(|_| rng.next_u64())
            .collect();
        BitVector::from_words(dim, words).expect("word count matches")
    }

    /// Majority bundle of `items`; ties resolved by a seeded random vector.
    fn bundle(dim: usize, items: &[BitVector], tie: &BitVector) -> BitVector {
        let mut counts = vec![0u32; dim];
        for v in items {
            for i in v.ones_iter() {
                counts[i] += 1;
            }
        }
        let n = items.len() as u32;
        BitVector::from_fn(dim, |i| match (counts[i] * 2).cmp(&n) {
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Less => false,
            std::cmp::Ordering::Equal => tie.bit(i),
        })
    }

    /// Encodes the store with `dim`-bit codes.
    #[must_use]
    pub fn build(store: &Store, dim: usize, seed: u64) -> Self {
        let mut rng = Rng::seed_from_u64(seed);
        let row_codes: Vec<BitVector> = (0..store.rows())
            .map(|_| Self::random(dim, &mut rng))
            .collect();
        let value_codes = [Self::random(dim, &mut rng), Self::random(dim, &mut rng)];
        let tie = Self::random(dim, &mut rng);
        let codes = store
            .tables
            .iter()
            .map(|t| {
                let bound: Vec<BitVector> = (0..store.rows())
                    .map(|r| row_codes[r].xor(&value_codes[usize::from(t.bit(r))]))
                    .collect();
                Self::bundle(dim, &bound, &tie)
            })
            .collect();
        Self {
            dim,
            row_codes,
            value_codes,
            codes,
        }
    }

    /// Encodes a query.
    #[must_use]
    pub fn encode_query(&self, query: &Query) -> BitVector {
        let bound: Vec<BitVector> = query
            .observed
            .iter()
            .map(|&(r, v)| self.row_codes[r].xor(&self.value_codes[usize::from(v)]))
            .collect();
        let tie = BitVector::zeros(self.dim);
        Self::bundle(self.dim, &bound, &tie)
    }
}

impl Method for HdcBundle {
    fn name(&self) -> String {
        format!("hdc-bundle({} bits)", self.dim)
    }

    fn bytes_per_entry(&self) -> f64 {
        self.dim as f64 / 8.0
    }

    fn retrieve(&self, query: &Query, limit: usize) -> Vec<usize> {
        let q = self.encode_query(query);
        let mut scored: Vec<(usize, usize)> = self
            .codes
            .iter()
            .enumerate()
            .map(|(i, c)| (c.hamming(&q), i))
            .collect();
        scored.sort_unstable();
        scored.into_iter().take(limit).map(|(_, i)| i).collect()
    }
}

/// One measured setting.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetrievalRow {
    /// Method name.
    pub method: String,
    /// Observed rows per query.
    pub observed_rows: usize,
    /// Mean answer-set size.
    pub mean_answer_size: f64,
    /// Mean recall of the answer set within the top `|answer|` results.
    pub recall_at_answer: f64,
    /// Mean recall within the top `2 |answer|` results.
    pub recall_at_double: f64,
    /// Bytes of index per stored entry.
    pub bytes_per_entry: f64,
    /// Mean nanoseconds per query.
    pub mean_query_nanos: f64,
    /// Nanoseconds to build the index.
    pub build_nanos: u64,
}

/// The report.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetrievalReport {
    /// Stored entries.
    pub store_size: usize,
    /// Rows per entry.
    pub rows: usize,
    /// Bytes of the raw table per entry.
    pub table_bytes: f64,
    /// Settings.
    pub rows_measured: Vec<RetrievalRow>,
}

fn recall(answer: &BTreeSet<usize>, ranked: &[usize], limit: usize) -> f64 {
    if answer.is_empty() {
        return 1.0;
    }
    let hits = ranked
        .iter()
        .take(limit)
        .filter(|i| answer.contains(i))
        .count();
    hits as f64 / answer.len() as f64
}

fn measure(
    method: &dyn Method,
    queries: &[Query],
    build_nanos: u64,
    observed_rows: usize,
) -> RetrievalRow {
    let mut r1 = 0.0;
    let mut r2 = 0.0;
    let mut sizes = 0.0;
    let mut nanos = 0u128;
    for q in queries {
        let a = q.answer.len().max(1);
        let started = Instant::now();
        let ranked = method.retrieve(q, 2 * a);
        nanos += started.elapsed().as_nanos();
        r1 += recall(&q.answer, &ranked, a);
        r2 += recall(&q.answer, &ranked, 2 * a);
        sizes += q.answer.len() as f64;
    }
    let n = queries.len().max(1) as f64;
    RetrievalRow {
        method: method.name(),
        observed_rows,
        mean_answer_size: sizes / n,
        recall_at_answer: r1 / n,
        recall_at_double: r2 / n,
        bytes_per_entry: method.bytes_per_entry(),
        mean_query_nanos: nanos as f64 / n,
        build_nanos,
    }
}

/// Runs the workload.
#[must_use]
pub fn run(spec: &RetrievalSpec, seed: u64) -> RetrievalReport {
    let mut rng = Rng::seed_from_u64(seed);
    let store = Store::build(spec, &mut rng);
    let mut rows_measured = Vec::new();
    let timed = |f: &dyn Fn() -> Box<dyn Method>| -> (Box<dyn Method>, u64) {
        let started = Instant::now();
        let m = f();
        (
            m,
            u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
        )
    };
    for &k in &spec.observed_rows {
        let queries: Vec<Query> = (0..spec.queries)
            .map(|_| Query::sample(&store, k, &mut rng))
            .collect();
        let exact = ExactScan::new(&store);
        rows_measured.push(measure(&exact, &queries, 0, k));
        let (bitmask, t) = timed(&|| Box::new(BitmaskIndex::build(&store)));
        rows_measured.push(measure(bitmask.as_ref(), &queries, t, k));
        let (bloom, t) =
            timed(&|| Box::new(BloomPerTable::build(&store, spec.bloom_bits_per_pair)));
        rows_measured.push(measure(bloom.as_ref(), &queries, t, k));
        for &dim in &spec.dimensions {
            let (hdc, t) = timed(&|| Box::new(HdcBundle::build(&store, dim, seed ^ dim as u64)));
            rows_measured.push(measure(hdc.as_ref(), &queries, t, k));
        }
    }
    RetrievalReport {
        store_size: store.len(),
        rows: store.rows(),
        table_bytes: store.rows() as f64 / 8.0,
        rows_measured,
    }
}

/// Renders a report as Markdown.
#[must_use]
pub fn render_markdown(report: &RetrievalReport, title: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# {title}\n");
    let _ = writeln!(
        out,
        "Store: {} entries of {} rows ({:.1} bytes per raw table).\n",
        report.store_size, report.rows, report.table_bytes
    );
    let _ = writeln!(
        out,
        "| Method | Observed rows | Mean answer size | Recall@answer | Recall@2x | Bytes/entry | ns/query | Build ms |"
    );
    let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|---:|---:|");
    for r in &report.rows_measured {
        let _ = writeln!(
            out,
            "| `{}` | {} | {:.1} | {:.3} | {:.3} | {:.1} | {:.0} | {:.1} |",
            r.method,
            r.observed_rows,
            r.mean_answer_size,
            r.recall_at_answer,
            r.recall_at_double,
            r.bytes_per_entry,
            r.mean_query_nanos,
            r.build_nanos as f64 / 1e6
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> RetrievalSpec {
        RetrievalSpec {
            arity: 5,
            families: vec![Family::Affine, Family::Monotone, Family::KTermDnf],
            store_per_family: 30,
            queries: 20,
            observed_rows: vec![4, 8],
            dimensions: vec![512, 2048],
            bloom_bits_per_pair: 8,
            params: FamilyParams::default(),
        }
    }

    #[test]
    fn exact_methods_return_exactly_the_consistent_set() {
        let mut rng = Rng::seed_from_u64(4);
        let store = Store::build(&spec(), &mut rng);
        assert!(store.len() > 30);
        let bitmask = BitmaskIndex::build(&store);
        let bloom = BloomPerTable::build(&store, 8);
        for _ in 0..20 {
            let q = Query::sample(&store, 6, &mut rng);
            let exact: BTreeSet<usize> = ExactScan::new(&store)
                .retrieve(&q, usize::MAX)
                .into_iter()
                .collect();
            assert_eq!(exact, q.answer);
            let masked: BTreeSet<usize> = bitmask.retrieve(&q, usize::MAX).into_iter().collect();
            assert_eq!(masked, q.answer);
            let bloomed: BTreeSet<usize> = bloom.retrieve(&q, usize::MAX).into_iter().collect();
            assert!(
                q.answer.is_subset(&bloomed),
                "a Bloom filter has no false negatives"
            );
        }
    }

    #[test]
    fn hdc_ranks_the_target_well_but_not_perfectly_and_the_report_renders() {
        let report = run(&spec(), 9);
        let exact = report
            .rows_measured
            .iter()
            .find(|r| r.method == "exact-scan")
            .unwrap();
        assert_eq!(exact.recall_at_answer, 1.0);
        let hdc = report
            .rows_measured
            .iter()
            .find(|r| r.method.starts_with("hdc-bundle(2048"))
            .unwrap();
        assert!(hdc.recall_at_answer > 0.3, "{hdc:?}");
        assert!(hdc.bytes_per_entry > report.table_bytes);
        let md = render_markdown(&report, "test");
        assert!(md.contains("bitmask-index"));
        assert_eq!(
            run(&spec(), 9).into_deterministic(),
            report.clone().into_deterministic(),
            "deterministic apart from timing"
        );
    }

    impl RetrievalReport {
        fn into_deterministic(mut self) -> Self {
            for r in &mut self.rows_measured {
                r.mean_query_nanos = 0.0;
                r.build_nanos = 0;
            }
            self
        }
    }
}
