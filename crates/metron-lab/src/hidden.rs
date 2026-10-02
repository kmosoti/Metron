//! The sealed hidden target.

use crate::truth_table::TruthTable;
use metron_core::hash::ContentHash;
use std::fmt;

/// A hidden Boolean function.
///
/// Nothing outside this crate can read the table: there is no accessor, no
/// `Serialize`, and `Debug` is redacted. The only way to learn about it is
/// through the oracle port, one receipted probe at a time.
#[derive(Clone)]
pub struct HiddenFunction {
    table: TruthTable,
}

impl HiddenFunction {
    /// Seals a truth table.
    #[must_use]
    pub const fn new(table: TruthTable) -> Self {
        Self { table }
    }

    /// Number of inputs. This is public information in the question.
    #[must_use]
    pub const fn arity(&self) -> u8 {
        self.table.arity()
    }

    /// A hash of the table, for matching results to fixtures. It is an
    /// identity, not a secret: small tables are trivially brute-forced.
    #[must_use]
    pub fn fingerprint(&self) -> ContentHash {
        ContentHash::of_bytes(self.table.to_rows_string().as_bytes())
    }

    pub(crate) fn evaluate(&self, row: usize) -> bool {
        self.table.eval(row)
    }

    pub(crate) const fn table(&self) -> &TruthTable {
        &self.table
    }
}

impl fmt::Debug for HiddenFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "HiddenFunction {{ arity: {}, table: <redacted> }}",
            self.arity()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_is_redacted() {
        let h = HiddenFunction::new(TruthTable::parse_rows(2, "0110").unwrap());
        let s = format!("{h:?}");
        assert!(s.contains("<redacted>"));
        assert!(!s.contains("0110"));
        assert_eq!(h.arity(), 2);
    }
}
