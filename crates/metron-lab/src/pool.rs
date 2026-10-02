//! Hypothesis pools: the public, finite hypothesis class a question comes
//! with.
//!
//! The prior-art review's central point about the laboratory is that with
//! an unrestricted class every strategy must probe every row; all the
//! interesting structure lives in the *restricted* class. The pool is that
//! class, sampled from mixed families and published to the system as a
//! document it may read without a receipt. Which member is the target stays
//! hidden.

use crate::families::{Family, FamilyParams, Target, sample};
use crate::truth_table::TruthTable;
use metron_core::bits::BitVector;
use metron_core::hash::ContentHash;
use metron_core::inquiry::Representation;
use metron_core::rng::Rng;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Name of the document holding every member's rows, concatenated.
pub const POOL_DOCUMENT: &str = "hypothesis-pool";
/// Name of the document holding each member's family label.
pub const POOL_FAMILIES_DOCUMENT: &str = "hypothesis-pool-families";

/// One member of the pool.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PoolMember {
    /// The table.
    pub table: TruthTable,
    /// Family it was sampled from.
    pub family: Family,
    /// Description, laboratory-side only.
    pub description: String,
}

/// A finite hypothesis class.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HypothesisPool {
    arity: u8,
    members: Vec<PoolMember>,
}

/// How to build a pool.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolSpec {
    /// Families to sample from.
    pub families: Vec<Family>,
    /// Members to sample per family (after de-duplication the count may be
    /// lower when a family is small).
    pub per_family: usize,
    /// Generator knobs.
    #[serde(default)]
    pub params: FamilyParams,
}

impl HypothesisPool {
    /// Samples a pool. Duplicate tables are dropped, so a family may end up
    /// with fewer members than requested.
    #[must_use]
    pub fn build(arity: u8, spec: &PoolSpec, rng: &mut Rng) -> Self {
        let mut members: Vec<PoolMember> = Vec::new();
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        for &family in &spec.families {
            let mut added = 0;
            let mut attempts = 0;
            while added < spec.per_family && attempts < spec.per_family * 20 {
                attempts += 1;
                let Target {
                    table,
                    family,
                    description,
                } = sample(family, arity, &spec.params, rng);
                let key = table.to_rows_string();
                if seen.contains_key(&key) {
                    continue;
                }
                seen.insert(key, members.len());
                members.push(PoolMember {
                    table,
                    family,
                    description,
                });
                added += 1;
            }
        }
        Self { arity, members }
    }

    /// Number of inputs.
    #[must_use]
    pub const fn arity(&self) -> u8 {
        self.arity
    }

    /// Rows per member.
    #[must_use]
    pub const fn rows(&self) -> usize {
        TruthTable::rows_for(self.arity)
    }

    /// Number of members.
    #[must_use]
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Whether the pool is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// The members.
    #[must_use]
    pub fn members(&self) -> &[PoolMember] {
        &self.members
    }

    /// Members per family.
    #[must_use]
    pub fn family_counts(&self) -> BTreeMap<Family, usize> {
        let mut counts = BTreeMap::new();
        for m in &self.members {
            *counts.entry(m.family).or_insert(0) += 1;
        }
        counts
    }

    /// Index of `table` in the pool, if present.
    #[must_use]
    pub fn index_of(&self, table: &TruthTable) -> Option<usize> {
        self.members.iter().position(|m| m.table == *table)
    }

    /// Indices of the members of `family`.
    #[must_use]
    pub fn indices_of(&self, family: Family) -> Vec<usize> {
        self.members
            .iter()
            .enumerate()
            .filter(|(_, m)| m.family == family)
            .map(|(i, _)| i)
            .collect()
    }

    /// The members' tables only.
    #[must_use]
    pub fn tables(&self) -> Vec<TruthTable> {
        self.members.iter().map(|m| m.table.clone()).collect()
    }

    /// The public documents: `hypothesis-pool` (member `i` occupies bits
    /// `i*rows .. (i+1)*rows`) and `hypothesis-pool-families` (a JSON array
    /// of labels).
    #[must_use]
    pub fn documents(&self) -> Vec<(String, Representation)> {
        let rows = self.rows();
        let bits = BitVector::from_fn(self.members.len() * rows, |b| {
            self.members[b / rows].table.eval(b % rows)
        });
        let families: Vec<&str> = self.members.iter().map(|m| m.family.label()).collect();
        vec![
            (POOL_DOCUMENT.to_owned(), Representation::Bits(bits)),
            (
                POOL_FAMILIES_DOCUMENT.to_owned(),
                Representation::Json(serde_json::json!(families)),
            ),
        ]
    }

    /// Hash of the public documents.
    #[must_use]
    pub fn hash(&self) -> ContentHash {
        ContentHash::of_json(&self.documents()).expect("documents are always serialisable")
    }

    /// The question parameters describing the pool.
    #[must_use]
    pub fn question_params(&self) -> serde_json::Value {
        serde_json::json!({
            "document": POOL_DOCUMENT,
            "families_document": POOL_FAMILIES_DOCUMENT,
            "count": self.members.len(),
            "rows": self.rows(),
            "hash": self.hash(),
            "family_counts": self.family_counts().iter().map(|(f, c)| (f.label().to_owned(), *c)).collect::<BTreeMap<_, _>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_is_deduplicated_and_publishes_documents() {
        let spec = PoolSpec {
            families: vec![Family::Affine, Family::Monotone],
            per_family: 10,
            params: FamilyParams::default(),
        };
        let pool = HypothesisPool::build(4, &spec, &mut Rng::seed_from_u64(1));
        assert!(pool.len() <= 20 && pool.len() >= 10);
        let keys: std::collections::BTreeSet<String> = pool
            .members()
            .iter()
            .map(|m| m.table.to_rows_string())
            .collect();
        assert_eq!(keys.len(), pool.len(), "no duplicate tables");
        let docs = pool.documents();
        let bits = docs[0].1.as_bits().unwrap();
        assert_eq!(bits.dim(), pool.len() * 16);
        for (i, m) in pool.members().iter().enumerate() {
            for row in 0..16 {
                assert_eq!(bits.bit(i * 16 + row), m.table.eval(row));
            }
        }
        assert_eq!(pool.index_of(&pool.members()[3].table), Some(3));
        assert!(pool.question_params()["count"].as_u64().unwrap() == pool.len() as u64);
        assert_eq!(
            pool.hash(),
            HypothesisPool::build(4, &spec, &mut Rng::seed_from_u64(1)).hash()
        );
    }
}
