//! Transform (exact, under the pool assumption): a single surviving member
//! becomes the complete truth table.

use crate::support::{member_bit, pool_bits, pool_layout, version_space_mask};
use crate::{frames, views};
use metron_core::bits::BitVector;
use metron_core::cost::Cost;
use metron_core::frame::TransformContract;
use metron_core::id::OperatorId;
use metron_core::inquiry::{Derivation, Inquiry, Representation};
use metron_core::operator::{Operator, OperatorError, OperatorKind, OperatorOutcome, OperatorSpec};
use metron_core::rng::Rng;
use metron_core::world::Knowledge;

/// When exactly one member of the pool survives, writes its rows as the
/// complete table.
#[derive(Debug, Default, Clone, Copy)]
pub struct SingleSurvivorToTable;

impl SingleSurvivorToTable {
    /// The operator identifier.
    pub const ID: &'static str = "single-survivor-to-table";

    /// The contract this transform runs under.
    #[must_use]
    pub fn contract() -> TransformContract {
        TransformContract::new([frames::VERSION_SPACE], frames::TRUTH_TABLE_COMPLETE)
            .preserves("every observed row")
            .assumes("the hidden function is a member of the published pool")
    }
}

impl<W: Knowledge> Operator<W> for SingleSurvivorToTable {
    fn spec(&self) -> OperatorSpec {
        OperatorSpec::new(
            Self::ID,
            OperatorKind::Transform {
                contract: Self::contract(),
            },
            "Copy the rows of the single surviving pool member into the complete table.",
        )
        .reads(views::VERSION_SPACE)
        .writes(views::COMPLETE_TABLE)
    }

    fn apply(
        &self,
        inquiry: &mut Inquiry,
        world: &mut W,
        _rng: &mut Rng,
    ) -> Result<OperatorOutcome, OperatorError> {
        let layout = pool_layout(inquiry)?;
        let bits = pool_bits(world, &layout)?.clone();
        let Some(mask) = version_space_mask(inquiry, &layout) else {
            return Err(OperatorError::Internal(
                "version-space view is malformed".into(),
            ));
        };
        let survivors: Vec<usize> = mask.ones_iter().collect();
        if survivors.len() != 1 {
            return Ok(OperatorOutcome::no_change(Cost::work(1))
                .with_note(format!("{} survivors; need exactly one", survivors.len())));
        }
        let member = survivors[0];
        let table = BitVector::from_fn(layout.rows, |row| member_bit(&bits, &layout, member, row));
        inquiry.write_view(
            views::COMPLETE_TABLE,
            frames::TRUTH_TABLE_COMPLETE,
            Representation::Bits(table),
            Derivation::by(OperatorId::from(Self::ID), inquiry.steps)
                .from_view(views::VERSION_SPACE),
        );
        Ok(OperatorOutcome::progressed(Cost::work(layout.rows as u64))
            .with_note(format!("pool member {member} is the single survivor")))
    }
}
