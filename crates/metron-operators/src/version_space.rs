//! Transform (exact): observations -> version space over the published pool.

use crate::support::{arity, member_bit, observed_rows, pool_bits, pool_families, pool_layout};
use crate::{frames, views};
use metron_core::bits::BitVector;
use metron_core::cost::Cost;
use metron_core::frame::TransformContract;
use metron_core::id::{OperatorId, ViewId};
use metron_core::inquiry::{Derivation, Inquiry, Representation};
use metron_core::operator::{Operator, OperatorError, OperatorKind, OperatorOutcome, OperatorSpec};
use metron_core::rng::Rng;
use metron_core::world::Knowledge;

/// Computes the mask of pool members consistent with every observation,
/// optionally restricted to the members of one family.
///
/// The restriction is an assumption and the contract says so. A restricted
/// filter whose family does not contain the target ends with an empty
/// version space; a fixed strategy then falls through to an unrestricted
/// filter, or fails. One unit of work per member scanned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionSpaceFilter {
    restrict: Option<String>,
}

impl VersionSpaceFilter {
    /// The identifier of the unrestricted filter.
    pub const ID: &'static str = "version-space-filter";

    /// A filter over the whole pool.
    #[must_use]
    pub const fn unrestricted() -> Self {
        Self { restrict: None }
    }

    /// A filter over the members labelled `family`.
    pub fn restricted(family: impl Into<String>) -> Self {
        Self {
            restrict: Some(family.into()),
        }
    }

    /// The identifier of a restricted filter.
    #[must_use]
    pub fn id_for(family: &str) -> String {
        format!("{}:{family}", Self::ID)
    }

    /// This filter's identifier.
    #[must_use]
    pub fn id(&self) -> String {
        match &self.restrict {
            Some(f) => Self::id_for(f),
            None => Self::ID.to_owned(),
        }
    }

    /// The contract this transform runs under.
    #[must_use]
    pub fn contract(&self) -> TransformContract {
        let c = TransformContract::new([frames::OBSERVATIONS], frames::VERSION_SPACE)
            .preserves("agreement of every surviving member with every observed row")
            .loses("the order in which rows were probed")
            .assumes("the hidden function is a member of the published pool");
        match &self.restrict {
            Some(f) => c.assumes(format!("the hidden function belongs to the `{f}` family")),
            None => c,
        }
    }
}

impl<W: Knowledge> Operator<W> for VersionSpaceFilter {
    fn spec(&self) -> OperatorSpec {
        OperatorSpec::new(
            self.id(),
            OperatorKind::Transform {
                contract: self.contract(),
            },
            match &self.restrict {
                Some(f) => {
                    format!("Keep the `{f}` members of the pool that agree with every observation.")
                }
                None => {
                    "Keep the members of the pool that agree with every observation.".to_owned()
                }
            },
        )
        .writes(views::VERSION_SPACE)
    }

    fn apply(
        &self,
        inquiry: &mut Inquiry,
        world: &mut W,
        _rng: &mut Rng,
    ) -> Result<OperatorOutcome, OperatorError> {
        let arity = arity(inquiry)?;
        let layout = pool_layout(inquiry)?;
        let bits = pool_bits(world, &layout)?;
        let candidates: Vec<usize> = match &self.restrict {
            Some(family) => pool_families(world, &layout)?
                .iter()
                .enumerate()
                .filter(|(_, f)| *f == family)
                .map(|(i, _)| i)
                .collect(),
            None => (0..layout.count).collect(),
        };
        let observed = observed_rows(inquiry, arity);
        let mut survivors = 0usize;
        let mut mask = BitVector::zeros(layout.count);
        for &i in &candidates {
            if observed
                .iter()
                .all(|&(row, out)| member_bit(bits, &layout, i, row) == out)
            {
                mask.set(i, true).expect("index within pool");
                survivors += 1;
            }
        }
        let mut derivation = Derivation::by(OperatorId::from(self.id()), inquiry.steps);
        if inquiry.has_view(&ViewId::from(views::OBSERVATIONS)) {
            derivation = derivation.from_view(views::OBSERVATIONS);
        }
        inquiry.write_view(
            views::VERSION_SPACE,
            frames::VERSION_SPACE,
            Representation::Bits(mask),
            derivation,
        );
        Ok(
            OperatorOutcome::progressed(Cost::work(candidates.len() as u64)).with_note(format!(
                "{survivors} of {} candidates survive {} observations",
                candidates.len(),
                observed.len()
            )),
        )
    }
}
