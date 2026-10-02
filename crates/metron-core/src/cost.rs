//! The pre-registered cost model.
//!
//! Every hypothesis in the laboratory is scored under a cost model fixed
//! before experiments run. [`Cost`] is that model. It counts only things that
//! are reproducible: operator calls, operator-reported work units, oracle
//! probes and external calls. Wall-clock time is recorded on receipts and
//! journal entries as information, never used for a decision, because it does
//! not replay.

use serde::{Deserialize, Serialize};
use std::ops::{Add, AddAssign};

/// Reproducible resources spent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Cost {
    /// Operator applications.
    pub operator_calls: u64,
    /// Operator-reported units of work (candidate expansions, evaluations,
    /// comparisons). Each operator documents what one unit means for it.
    pub work_units: u64,
    /// Probes answered by an oracle.
    pub oracle_probes: u64,
    /// Calls to any other external service.
    pub external_calls: u64,
}

impl Cost {
    /// The zero cost.
    pub const ZERO: Cost = Cost {
        operator_calls: 0,
        work_units: 0,
        oracle_probes: 0,
        external_calls: 0,
    };

    /// One operator call.
    #[must_use]
    pub const fn operator_call() -> Cost {
        Cost {
            operator_calls: 1,
            ..Cost::ZERO
        }
    }

    /// `n` work units.
    #[must_use]
    pub const fn work(n: u64) -> Cost {
        Cost {
            work_units: n,
            ..Cost::ZERO
        }
    }

    /// `n` oracle probes.
    #[must_use]
    pub const fn probes(n: u64) -> Cost {
        Cost {
            oracle_probes: n,
            ..Cost::ZERO
        }
    }

    /// `n` external calls.
    #[must_use]
    pub const fn external(n: u64) -> Cost {
        Cost {
            external_calls: n,
            ..Cost::ZERO
        }
    }

    /// Componentwise saturating addition.
    #[must_use]
    pub const fn saturating_add(self, other: Cost) -> Cost {
        Cost {
            operator_calls: self.operator_calls.saturating_add(other.operator_calls),
            work_units: self.work_units.saturating_add(other.work_units),
            oracle_probes: self.oracle_probes.saturating_add(other.oracle_probes),
            external_calls: self.external_calls.saturating_add(other.external_calls),
        }
    }

    /// Componentwise saturating subtraction.
    #[must_use]
    pub const fn saturating_sub(self, other: Cost) -> Cost {
        Cost {
            operator_calls: self.operator_calls.saturating_sub(other.operator_calls),
            work_units: self.work_units.saturating_sub(other.work_units),
            oracle_probes: self.oracle_probes.saturating_sub(other.oracle_probes),
            external_calls: self.external_calls.saturating_sub(other.external_calls),
        }
    }
}

impl Add for Cost {
    type Output = Cost;

    fn add(self, rhs: Cost) -> Cost {
        self.saturating_add(rhs)
    }
}

impl AddAssign for Cost {
    fn add_assign(&mut self, rhs: Cost) {
        *self = self.saturating_add(rhs);
    }
}

/// The dimension of a [`Budget`] that was exceeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BudgetDimension {
    /// Too many operator calls.
    OperatorCalls,
    /// Too many work units.
    WorkUnits,
    /// Too many oracle probes.
    OracleProbes,
    /// Too many external calls.
    ExternalCalls,
}

/// Upper bounds on [`Cost`]. `None` means unlimited in that dimension.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Budget {
    /// Maximum operator calls.
    #[serde(default)]
    pub max_operator_calls: Option<u64>,
    /// Maximum work units.
    #[serde(default)]
    pub max_work_units: Option<u64>,
    /// Maximum oracle probes.
    #[serde(default)]
    pub max_oracle_probes: Option<u64>,
    /// Maximum external calls.
    #[serde(default)]
    pub max_external_calls: Option<u64>,
}

impl Budget {
    /// No limits.
    pub const UNLIMITED: Budget = Budget {
        max_operator_calls: None,
        max_work_units: None,
        max_oracle_probes: None,
        max_external_calls: None,
    };

    /// Sets the operator-call limit.
    #[must_use]
    pub const fn with_operator_calls(mut self, max: u64) -> Budget {
        self.max_operator_calls = Some(max);
        self
    }

    /// Sets the work-unit limit.
    #[must_use]
    pub const fn with_work_units(mut self, max: u64) -> Budget {
        self.max_work_units = Some(max);
        self
    }

    /// Sets the oracle-probe limit.
    #[must_use]
    pub const fn with_oracle_probes(mut self, max: u64) -> Budget {
        self.max_oracle_probes = Some(max);
        self
    }

    /// Sets the external-call limit.
    #[must_use]
    pub const fn with_external_calls(mut self, max: u64) -> Budget {
        self.max_external_calls = Some(max);
        self
    }

    /// The first dimension in which `spent` exceeds this budget.
    #[must_use]
    pub fn exceeded_by(&self, spent: &Cost) -> Option<BudgetDimension> {
        if self
            .max_operator_calls
            .is_some_and(|m| spent.operator_calls > m)
        {
            return Some(BudgetDimension::OperatorCalls);
        }
        if self.max_work_units.is_some_and(|m| spent.work_units > m) {
            return Some(BudgetDimension::WorkUnits);
        }
        if self
            .max_oracle_probes
            .is_some_and(|m| spent.oracle_probes > m)
        {
            return Some(BudgetDimension::OracleProbes);
        }
        if self
            .max_external_calls
            .is_some_and(|m| spent.external_calls > m)
        {
            return Some(BudgetDimension::ExternalCalls);
        }
        None
    }

    /// Oracle probes still available, or `None` if unlimited.
    #[must_use]
    pub fn remaining_oracle_probes(&self, spent: &Cost) -> Option<u64> {
        self.max_oracle_probes
            .map(|m| m.saturating_sub(spent.oracle_probes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addition_is_componentwise_and_saturating() {
        let a = Cost::operator_call() + Cost::work(3) + Cost::probes(2);
        assert_eq!(a.operator_calls, 1);
        assert_eq!(a.work_units, 3);
        assert_eq!(a.oracle_probes, 2);
        let big = Cost::work(u64::MAX) + Cost::work(1);
        assert_eq!(big.work_units, u64::MAX);
        assert_eq!((a.saturating_sub(Cost::work(10))).work_units, 0);
    }

    #[test]
    fn budget_reports_first_exceeded_dimension() {
        let budget = Budget::UNLIMITED
            .with_oracle_probes(4)
            .with_operator_calls(10);
        assert_eq!(budget.exceeded_by(&Cost::probes(4)), None);
        assert_eq!(
            budget.exceeded_by(&Cost::probes(5)),
            Some(BudgetDimension::OracleProbes)
        );
        assert_eq!(budget.remaining_oracle_probes(&Cost::probes(1)), Some(3));
        assert_eq!(Budget::UNLIMITED.remaining_oracle_probes(&Cost::ZERO), None);
    }
}
