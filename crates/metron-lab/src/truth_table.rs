//! Truth tables of Boolean functions on up to eight inputs.

use metron_core::bits::BitVector;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Largest supported number of inputs.
pub const MAX_ARITY: u8 = 8;

/// Truth-table errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TruthTableError {
    /// More inputs than supported.
    #[error("arity {0} exceeds the maximum of {MAX_ARITY}")]
    ArityTooLarge(u8),
    /// The bit vector has the wrong length for the arity.
    #[error("expected {expected} rows, got {actual}")]
    WrongLength {
        /// Rows required.
        expected: usize,
        /// Rows supplied.
        actual: usize,
    },
    /// A character other than `0` or `1`.
    #[error("unexpected character {0:?} in truth table")]
    BadCharacter(char),
}

/// A complete truth table. Row `i` holds `f` at the assignment whose
/// variable `x_j` is bit `j` of `i`.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TruthTable {
    arity: u8,
    bits: BitVector,
}

impl TruthTable {
    /// Number of rows for `arity` inputs.
    #[must_use]
    pub const fn rows_for(arity: u8) -> usize {
        1usize << arity
    }

    /// Builds a table from its rows.
    pub fn from_bits(arity: u8, bits: BitVector) -> Result<Self, TruthTableError> {
        if arity > MAX_ARITY {
            return Err(TruthTableError::ArityTooLarge(arity));
        }
        let expected = Self::rows_for(arity);
        if bits.dim() != expected {
            return Err(TruthTableError::WrongLength {
                expected,
                actual: bits.dim(),
            });
        }
        Ok(Self { arity, bits })
    }

    /// Builds a table by evaluating `f` at every row index.
    pub fn from_fn(arity: u8, f: impl FnMut(usize) -> bool) -> Result<Self, TruthTableError> {
        if arity > MAX_ARITY {
            return Err(TruthTableError::ArityTooLarge(arity));
        }
        Ok(Self {
            arity,
            bits: BitVector::from_fn(Self::rows_for(arity), f),
        })
    }

    /// Parses a string of `0`/`1` characters, row 0 first. Whitespace and
    /// underscores are ignored.
    pub fn parse_rows(arity: u8, text: &str) -> Result<Self, TruthTableError> {
        if arity > MAX_ARITY {
            return Err(TruthTableError::ArityTooLarge(arity));
        }
        let mut rows = Vec::with_capacity(Self::rows_for(arity));
        for c in text.chars() {
            match c {
                '0' => rows.push(false),
                '1' => rows.push(true),
                c if c.is_whitespace() || c == '_' => {}
                c => return Err(TruthTableError::BadCharacter(c)),
            }
        }
        if rows.len() != Self::rows_for(arity) {
            return Err(TruthTableError::WrongLength {
                expected: Self::rows_for(arity),
                actual: rows.len(),
            });
        }
        Ok(Self {
            arity,
            bits: BitVector::from_bools(&rows),
        })
    }

    /// Number of inputs.
    #[must_use]
    pub const fn arity(&self) -> u8 {
        self.arity
    }

    /// Number of rows.
    #[must_use]
    pub const fn rows(&self) -> usize {
        Self::rows_for(self.arity)
    }

    /// The function's value at row `index`.
    #[must_use]
    pub fn eval(&self, index: usize) -> bool {
        self.bits.bit(index)
    }

    /// The rows as a bit vector.
    #[must_use]
    pub const fn bits(&self) -> &BitVector {
        &self.bits
    }

    /// Row index of an assignment given as `x_0, x_1, ...`.
    #[must_use]
    pub fn row_index(assignment: &[bool]) -> usize {
        assignment
            .iter()
            .enumerate()
            .filter(|(_, b)| **b)
            .map(|(j, _)| 1usize << j)
            .sum()
    }

    /// Rows as a `0`/`1` string, row 0 first.
    #[must_use]
    pub fn to_rows_string(&self) -> String {
        self.bits
            .iter()
            .map(|b| if b { '1' } else { '0' })
            .collect()
    }

    /// Number of rows on which `candidate` agrees with this table, or
    /// `None` if the dimensions differ.
    #[must_use]
    pub fn agreement(&self, candidate: &BitVector) -> Option<usize> {
        if candidate.dim() != self.bits.dim() {
            return None;
        }
        Some(self.rows() - self.bits.hamming(candidate))
    }
}

impl fmt::Debug for TruthTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "TruthTable(arity={}, rows={})",
            self.arity,
            self.to_rows_string()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_evaluate_majority_of_three() {
        // maj(x0,x1,x2): rows 0..8 with x0 = bit 0.
        let t = TruthTable::parse_rows(3, "0001 0111").unwrap();
        assert_eq!(t.rows(), 8);
        assert!(!t.eval(0));
        assert!(t.eval(3)); // x0=1,x1=1,x2=0
        assert!(t.eval(5)); // x0=1,x2=1
        assert!(!t.eval(4));
        assert_eq!(TruthTable::row_index(&[true, false, true]), 5);
        assert_eq!(t.to_rows_string(), "00010111");
        assert_eq!(t.agreement(t.bits()), Some(8));
        assert_eq!(t.agreement(&BitVector::zeros(8)), Some(4));
        assert_eq!(t.agreement(&BitVector::zeros(4)), None);
    }

    #[test]
    fn rejects_malformed_input() {
        assert_eq!(
            TruthTable::parse_rows(2, "01x1"),
            Err(TruthTableError::BadCharacter('x'))
        );
        assert_eq!(
            TruthTable::parse_rows(2, "011"),
            Err(TruthTableError::WrongLength {
                expected: 4,
                actual: 3
            })
        );
        assert_eq!(
            TruthTable::parse_rows(9, ""),
            Err(TruthTableError::ArityTooLarge(9))
        );
        assert!(TruthTable::from_bits(2, BitVector::zeros(5)).is_err());
    }
}
