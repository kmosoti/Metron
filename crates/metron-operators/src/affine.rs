//! The affine shortcut: `n + 1` probes identify any affine function.

use crate::support::{
    arity, observed_rows, observed_set, probe_for_row, rows, write_observations_view,
};
use crate::{frames, views};
use metron_core::bits::BitVector;
use metron_core::cost::Cost;
use metron_core::frame::TransformContract;
use metron_core::id::OperatorId;
use metron_core::inquiry::{Derivation, Inquiry, Representation};
use metron_core::operator::{Operator, OperatorError, OperatorKind, OperatorOutcome, OperatorSpec};
use metron_core::rng::Rng;
use metron_core::world::Oracle;

/// Probes the zero assignment and the unit vectors (those not yet
/// observed), in one application.
#[derive(Debug, Default, Clone, Copy)]
pub struct AffineProbe;

impl AffineProbe {
    /// The operator identifier.
    pub const ID: &'static str = "affine-probe";

    /// The anchor rows: 0 and the unit vectors.
    fn anchors(arity: usize) -> Vec<usize> {
        std::iter::once(0)
            .chain((0..arity).map(|i| 1usize << i))
            .collect()
    }
}

impl<W: Oracle> Operator<W> for AffineProbe {
    fn spec(&self) -> OperatorSpec {
        OperatorSpec::new(
            Self::ID,
            OperatorKind::Observe,
            "Probe the zero assignment and every unit vector.",
        )
        .writes(views::OBSERVATIONS)
    }

    fn apply(
        &self,
        inquiry: &mut Inquiry,
        world: &mut W,
        _rng: &mut Rng,
    ) -> Result<OperatorOutcome, OperatorError> {
        let arity = arity(inquiry)?;
        let observed = observed_set(inquiry, arity);
        let mut probed = 0u64;
        let mut capped = false;
        for row in Self::anchors(arity) {
            if observed.contains(&row) {
                continue;
            }
            if world.probes_remaining() == Some(0) {
                capped = true;
                break;
            }
            let observation = world.probe(
                &OperatorId::from(Self::ID),
                inquiry.id,
                &probe_for_row(row, arity),
            )?;
            inquiry.record_observation(observation);
            probed += 1;
        }
        if probed == 0 {
            return Ok(
                OperatorOutcome::no_change(Cost::work(arity as u64 + 1)).with_note(if capped {
                    "the protocol's probe cap is spent"
                } else {
                    "anchor rows already observed"
                }),
            );
        }
        write_observations_view(inquiry, Self::ID, arity);
        Ok(
            OperatorOutcome::progressed(Cost::work(arity as u64 + 1)).with_note(format!(
                "probed {probed} anchor rows{}",
                if capped { " (cap reached)" } else { "" }
            )),
        )
    }
}

/// Builds the affine function through the anchor observations and writes
/// it as the complete table, unless any observation refutes it.
#[derive(Debug, Default, Clone, Copy)]
pub struct AffineSolve;

impl AffineSolve {
    /// The operator identifier.
    pub const ID: &'static str = "affine-solve";

    /// The contract this transform runs under.
    #[must_use]
    pub fn contract() -> TransformContract {
        TransformContract::new([frames::OBSERVATIONS], frames::TRUTH_TABLE_COMPLETE)
            .preserves("every observed row")
            .assumes("the hidden function is affine over GF(2)")
            .approximate(
                "rows other than the observed ones are extrapolated from the affine hypothesis",
            )
    }
}

impl<W> Operator<W> for AffineSolve {
    fn spec(&self) -> OperatorSpec {
        OperatorSpec::new(
            Self::ID,
            OperatorKind::Transform {
                contract: Self::contract(),
            },
            "Solve a·x ⊕ b from the anchor rows and extrapolate, unless an observation refutes it.",
        )
        .reads(views::OBSERVATIONS)
        .writes(views::COMPLETE_TABLE)
    }

    fn apply(
        &self,
        inquiry: &mut Inquiry,
        _world: &mut W,
        _rng: &mut Rng,
    ) -> Result<OperatorOutcome, OperatorError> {
        let arity = arity(inquiry)?;
        let observed = observed_rows(inquiry, arity);
        let value = |row: usize| observed.iter().find(|&&(r, _)| r == row).map(|&(_, o)| o);
        let Some(b) = value(0) else {
            return Ok(OperatorOutcome::no_change(Cost::work(1)).with_note("row 0 not observed"));
        };
        let mut a = 0usize;
        for i in 0..arity {
            let Some(v) = value(1 << i) else {
                return Ok(OperatorOutcome::no_change(Cost::work(1))
                    .with_note(format!("unit row {} not observed", 1 << i)));
            };
            if v ^ b {
                a |= 1 << i;
            }
        }
        let total = rows(arity);
        let table = BitVector::from_fn(total, |row| ((row & a).count_ones() % 2 == 1) ^ b);
        if let Some(&(row, out)) = observed.iter().find(|&&(row, out)| table.bit(row) != out) {
            return Ok(
                OperatorOutcome::no_change(Cost::work(observed.len() as u64)).with_note(format!(
                    "affine hypothesis refuted by row {row} (observed {}, predicted {})",
                    u8::from(out),
                    u8::from(!out)
                )),
            );
        }
        inquiry.write_view(
            views::COMPLETE_TABLE,
            frames::TRUTH_TABLE_COMPLETE,
            Representation::Bits(table),
            Derivation::by(OperatorId::from(Self::ID), inquiry.steps)
                .from_view(views::OBSERVATIONS),
        );
        let terms: Vec<String> = (0..arity)
            .filter(|i| (a >> i) & 1 == 1)
            .map(|i| format!("x{i}"))
            .collect();
        Ok(
            OperatorOutcome::progressed(Cost::work(total as u64)).with_note(format!(
                "affine hypothesis {}{} consistent with {} observations",
                terms.join(" ⊕ "),
                if b { " ⊕ 1" } else { "" },
                observed.len()
            )),
        )
    }
}
