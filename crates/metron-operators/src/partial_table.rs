//! Transform (exact): observations -> partial truth table.

use crate::support::{arity, rows};
use crate::{frames, views};
use metron_core::cost::Cost;
use metron_core::frame::TransformContract;
use metron_core::id::OperatorId;
use metron_core::inquiry::{Derivation, Inquiry, Representation};
use metron_core::operator::{Operator, OperatorError, OperatorKind, OperatorOutcome, OperatorSpec};
use metron_core::rng::Rng;

/// Rewrites the observations view as one entry per row: `0`, `1`, or
/// `null` when the row is unobserved.
#[derive(Debug, Default, Clone, Copy)]
pub struct ObservationsToPartialTable;

impl ObservationsToPartialTable {
    /// The operator identifier.
    pub const ID: &'static str = "observations-to-partial-table";

    /// The contract this transform runs under.
    #[must_use]
    pub fn contract() -> TransformContract {
        TransformContract::new([frames::OBSERVATIONS], frames::TRUTH_TABLE_PARTIAL)
            .preserves("every observed (row, output) pair")
            .loses("the order in which rows were probed")
            .loses("which receipt paid for each observation")
    }
}

impl<W> Operator<W> for ObservationsToPartialTable {
    fn spec(&self) -> OperatorSpec {
        OperatorSpec::new(
            Self::ID,
            OperatorKind::Transform {
                contract: Self::contract(),
            },
            "Index the observations by row as a partial truth table.",
        )
        .reads(views::OBSERVATIONS)
        .writes(views::PARTIAL_TABLE)
    }

    fn apply(
        &self,
        inquiry: &mut Inquiry,
        _world: &mut W,
        _rng: &mut Rng,
    ) -> Result<OperatorOutcome, OperatorError> {
        let arity = arity(inquiry)?;
        let total = rows(arity);
        let source = inquiry
            .view_str(views::OBSERVATIONS)
            .ok_or(OperatorError::NotApplicable)?;
        let listing = source
            .content
            .as_json()
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                OperatorError::Internal("observations view is not a JSON array".into())
            })?;
        let mut outputs: Vec<serde_json::Value> = vec![serde_json::Value::Null; total];
        let mut used = 0u64;
        for entry in listing {
            let row = entry.get("row").and_then(serde_json::Value::as_u64);
            let output = entry.get("output").and_then(serde_json::Value::as_u64);
            if let (Some(row), Some(output)) = (row, output)
                && (row as usize) < total
            {
                outputs[row as usize] = serde_json::Value::from(output);
                used += 1;
            }
        }
        inquiry.write_view(
            views::PARTIAL_TABLE,
            frames::TRUTH_TABLE_PARTIAL,
            Representation::Json(serde_json::json!({ "arity": arity, "outputs": outputs })),
            Derivation::by(OperatorId::from(Self::ID), inquiry.steps)
                .from_view(views::OBSERVATIONS),
        );
        Ok(OperatorOutcome::progressed(Cost::work(used.max(1)))
            .with_note(format!("{used} of {total} rows known")))
    }
}
