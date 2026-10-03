//! Reference cognitive operators.
//!
//! Every operator here is written against `metron-core` ports only. None can
//! see a hidden target, a verdict, or a file. They are the building blocks
//! of the strategies the laboratory compares:
//!
//! * the exhaustive pipeline: [`ProbeNextUnobserved`] →
//!   [`ObservationsToPartialTable`] → [`CompleteByDefault`] →
//!   [`CommitTruthTable`];
//! * version-space identification over the published hypothesis pool:
//!   [`VersionSpaceFilter`] (optionally restricted to one family) →
//!   [`GreedySplitProbe`] → [`SingleSurvivorToTable`] → [`CommitTruthTable`];
//! * the affine shortcut: [`AffineProbe`] → [`AffineSolve`] →
//!   [`CommitTruthTable`];
//! * consultation: [`LlmProposeFormula`] → [`FormulaToTable`] (which
//!   verifies the proposal against every observation) → [`CommitTruthTable`];
//! * the structure-keyed learners of ADR 0015: [`SymmetricProbe`] →
//!   [`SymmetricSolve`], [`JuntaProbe`] → [`JuntaSolve`], verified with
//!   [`ProbeRandomUnobserved`], and [`StructureProfile`], the posterior over
//!   the promised classes that routers read.
//!
//! Each operator is exactly one of observe, transform, consult or commit, and
//! every transform publishes its contract.

pub mod formula;
pub mod frames;
pub mod views;

mod affine;
mod commit_table;
mod complete_table;
mod consult;
mod greedy_split;
mod partial_table;
mod probe_next;
mod structural;
mod support;
mod survivor;
mod version_space;

pub use affine::{AffineProbe, AffineSolve};
pub use commit_table::CommitTruthTable;
pub use complete_table::CompleteByDefault;
pub use consult::{FormulaToTable, LlmProposeFormula};
pub use greedy_split::GreedySplitProbe;
pub use partial_table::ObservationsToPartialTable;
pub use probe_next::ProbeNextUnobserved;
pub use structural::{
    JuntaProbe, JuntaSolve, ProbeRandomUnobserved, StructureProfile, SymmetricProbe, SymmetricSolve,
};
pub use survivor::SingleSurvivorToTable;
pub use version_space::VersionSpaceFilter;

use metron_core::operator::Operator;
use metron_core::world::{ExternalService, Knowledge, Oracle};

/// The exhaustive pipeline, in order.
#[must_use]
pub fn reference_operators<W: Oracle + 'static>() -> Vec<Box<dyn Operator<W>>> {
    vec![
        Box::new(ProbeNextUnobserved),
        Box::new(ObservationsToPartialTable),
        Box::new(CompleteByDefault),
        Box::new(CommitTruthTable),
    ]
}

/// Version-space operators over the published pool: the unrestricted
/// filter, one restricted filter per label in `families`, the greedy probe
/// and the single-survivor transform.
#[must_use]
pub fn version_space_operators<W: Oracle + Knowledge + 'static>(
    families: &[&str],
) -> Vec<Box<dyn Operator<W>>> {
    let mut ops: Vec<Box<dyn Operator<W>>> = vec![Box::new(VersionSpaceFilter::unrestricted())];
    for f in families {
        ops.push(Box::new(VersionSpaceFilter::restricted(*f)));
    }
    ops.push(Box::new(GreedySplitProbe));
    ops.push(Box::new(SingleSurvivorToTable));
    ops
}

/// The affine shortcut operators.
#[must_use]
pub fn affine_operators<W: Oracle + 'static>() -> Vec<Box<dyn Operator<W>>> {
    vec![Box::new(AffineProbe), Box::new(AffineSolve)]
}

/// The structure-keyed learners, their verification probe and the
/// structure profile.
#[must_use]
pub fn structural_operators<W: Oracle + 'static>() -> Vec<Box<dyn Operator<W>>> {
    vec![
        Box::new(ProbeRandomUnobserved),
        Box::new(SymmetricProbe),
        Box::new(SymmetricSolve),
        Box::new(JuntaProbe),
        Box::new(JuntaSolve),
        Box::new(StructureProfile),
    ]
}

/// The consultation operators.
#[must_use]
pub fn consultation_operators<W: ExternalService + 'static>() -> Vec<Box<dyn Operator<W>>> {
    vec![Box::new(LlmProposeFormula), Box::new(FormulaToTable)]
}

/// Every operator, for a world that offers every port.
#[must_use]
pub fn all_operators<W: Oracle + Knowledge + ExternalService + 'static>(
    families: &[&str],
) -> Vec<Box<dyn Operator<W>>> {
    let mut ops = reference_operators();
    ops.extend(version_space_operators(families));
    ops.extend(affine_operators());
    ops.extend(structural_operators());
    ops.extend(consultation_operators());
    ops
}
