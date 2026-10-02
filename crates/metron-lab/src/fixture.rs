//! Fixtures: hidden targets as data.
//!
//! A fixture is laboratory property. It is parsed from a string here so
//! the laboratory itself never touches the file system; the composition root
//! reads the file and hands the text over.

use crate::hidden::HiddenFunction;
use crate::truth_table::{TruthTable, TruthTableError};
use metron_core::hash::ContentHash;
use serde::{Deserialize, Serialize};

/// Fixture errors.
#[derive(Debug, thiserror::Error)]
pub enum FixtureError {
    /// Not valid JSON for a fixture.
    #[error("fixture is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The truth table is malformed.
    #[error("fixture truth table: {0}")]
    TruthTable(#[from] TruthTableError),
}

/// A hidden Boolean function fixture.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BooleanFixture {
    /// Name.
    pub name: String,
    /// Number of inputs.
    pub arity: u8,
    /// Rows as a `0`/`1` string, row 0 first (`x_0` is bit 0 of the index).
    pub rows: String,
    /// Free-form notes for people.
    #[serde(default)]
    pub notes: String,
}

impl BooleanFixture {
    /// Parses a fixture from JSON text.
    pub fn from_json(text: &str) -> Result<Self, FixtureError> {
        Ok(serde_json::from_str(text)?)
    }

    /// Seals the fixture into a hidden function.
    pub fn seal(&self) -> Result<HiddenFunction, FixtureError> {
        Ok(HiddenFunction::new(TruthTable::parse_rows(
            self.arity, &self.rows,
        )?))
    }

    /// Hash of the fixture's canonical JSON.
    #[must_use]
    pub fn fingerprint(&self) -> ContentHash {
        ContentHash::of_json(self).expect("fixtures are always serialisable")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_seal() {
        let f = BooleanFixture::from_json(r#"{"name":"xor2","arity":2,"rows":"0110"}"#).unwrap();
        let h = f.seal().unwrap();
        assert_eq!(h.arity(), 2);
        assert!(
            BooleanFixture::from_json(r#"{"name":"bad","arity":2,"rows":"01"}"#)
                .unwrap()
                .seal()
                .is_err()
        );
    }
}
