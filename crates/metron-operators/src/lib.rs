//! Reference cognitive operators.
//!
//! Every operator here is written against `metron-core` ports only. None can
//! see a hidden target, a verdict, or a file. Together they form one boring,
//! complete pipeline for the hidden-Boolean-function question:
//!
//! 1. [`ProbeNextUnobserved`] (observe) asks the oracle for the lowest row
//!    not yet observed.
//! 2. [`ObservationsToPartialTable`] (transform, exact) rewrites the
//!    observations as a partial truth table.
//! 3. [`CompleteByDefault`] (transform, approximate) fills unobserved rows
//!    with `false` and says so in its contract.
//! 4. [`CommitTruthTable`] (commit) commits the complete table with evidence
//!    tracing back to every observation it rests on.

pub mod frames;
pub mod views;

mod commit_table;
mod complete_table;
mod partial_table;
mod probe_next;
mod support;

pub use commit_table::CommitTruthTable;
pub use complete_table::CompleteByDefault;
pub use partial_table::ObservationsToPartialTable;
pub use probe_next::ProbeNextUnobserved;

use metron_core::operator::Operator;
use metron_core::world::Oracle;

/// All reference operators, in pipeline order.
#[must_use]
pub fn reference_operators<W: Oracle + 'static>() -> Vec<Box<dyn Operator<W>>> {
    vec![
        Box::new(ProbeNextUnobserved),
        Box::new(ObservationsToPartialTable),
        Box::new(CompleteByDefault),
        Box::new(CommitTruthTable),
    ]
}
