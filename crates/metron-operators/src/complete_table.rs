//! Transform (approximate): partial truth table -> complete truth table.

use crate::support::{arity, rows};
use crate::{frames, views};
use metron_core::bits::BitVector;
use metron_core::cost::Cost;
use metron_core::frame::TransformContract;
use metron_core::id::OperatorId;
use metron_core::inquiry::{Derivation, Inquiry, Representation};
use metron_core::operator::{Operator, OperatorError, OperatorKind, OperatorOutcome, OperatorSpec};
use metron_core::rng::Rng;

/// Fills every unobserved row with `false`.
///
/// This is an inductive bias, not an inference, and the contract says so:
/// the transform is approximate and assumes unobserved rows are false.
#[derive(Debug, Default, Clone, Copy)]
pub struct CompleteByDefault;

impl CompleteByDefault {
    /// The operator identifier.
    pub const ID: &'static str = "complete-table-by-default";

    /// The contract this transform runs under.
    #[must_use]
    pub fn contract() -> TransformContract {
        TransformContract::new([frames::TRUTH_TABLE_PARTIAL], frames::TRUTH_TABLE_COMPLETE)
            .preserves("every observed row")
            .assumes("unobserved rows evaluate to false")
            .approximate("unobserved rows are filled with a default, not inferred")
    }
}

impl<W> Operator<W> for CompleteByDefault {
    fn spec(&self) -> OperatorSpec {
        OperatorSpec::new(
            Self::ID,
            OperatorKind::Transform {
                contract: Self::contract(),
            },
            "Complete the partial table by setting every unobserved row to false.",
        )
        .reads(views::PARTIAL_TABLE)
        .writes(views::COMPLETE_TABLE)
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
            .view_str(views::PARTIAL_TABLE)
            .ok_or(OperatorError::NotApplicable)?;
        let outputs = source
            .content
            .as_json()
            .and_then(|v| v.get("outputs"))
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                OperatorError::Internal("partial table has no `outputs` array".into())
            })?;
        if outputs.len() != total {
            return Err(OperatorError::Internal(format!(
                "partial table has {} rows, expected {total}",
                outputs.len()
            )));
        }
        let guessed = outputs.iter().filter(|v| v.is_null()).count();
        let bits = BitVector::from_fn(total, |i| outputs[i].as_u64() == Some(1));
        inquiry.write_view(
            views::COMPLETE_TABLE,
            frames::TRUTH_TABLE_COMPLETE,
            Representation::Bits(bits),
            Derivation::by(OperatorId::from(Self::ID), inquiry.steps)
                .from_view(views::PARTIAL_TABLE),
        );
        Ok(OperatorOutcome::progressed(Cost::work(total as u64))
            .with_note(format!("{guessed} of {total} rows guessed")))
    }
}
