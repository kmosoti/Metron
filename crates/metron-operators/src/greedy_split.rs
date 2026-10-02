//! Observe: probe the row that best splits the surviving hypotheses.

use crate::support::{
    arity, member_bit, observed_set, pool_bits, pool_layout, probe_for_row, result_output, rows,
    version_space_mask, write_observations_view,
};
use crate::views;
use metron_core::cost::Cost;
use metron_core::id::OperatorId;
use metron_core::inquiry::Inquiry;
use metron_core::operator::{Operator, OperatorError, OperatorKind, OperatorOutcome, OperatorSpec};
use metron_core::rng::Rng;
use metron_core::world::{Knowledge, Oracle};

/// Probes the unobserved row that minimises the larger side of the split
/// of the current version space (Dasgupta's greedy rule), lowest row on
/// ties. One unit of work per (survivor, candidate row) pair examined.
#[derive(Debug, Default, Clone, Copy)]
pub struct GreedySplitProbe;

impl GreedySplitProbe {
    /// The operator identifier.
    pub const ID: &'static str = "greedy-split-probe";
}

impl<W: Oracle + Knowledge> Operator<W> for GreedySplitProbe {
    fn spec(&self) -> OperatorSpec {
        OperatorSpec::new(
            Self::ID,
            OperatorKind::Observe,
            "Probe the row that best halves the surviving members of the pool.",
        )
        .reads(views::VERSION_SPACE)
        .writes(views::OBSERVATIONS)
    }

    fn apply(
        &self,
        inquiry: &mut Inquiry,
        world: &mut W,
        _rng: &mut Rng,
    ) -> Result<OperatorOutcome, OperatorError> {
        let arity = arity(inquiry)?;
        let layout = pool_layout(inquiry)?;
        let bits = pool_bits(world, &layout)?.clone();
        let Some(mask) = version_space_mask(inquiry, &layout) else {
            return Err(OperatorError::Internal(
                "version-space view is malformed".into(),
            ));
        };
        let survivors: Vec<usize> = mask.ones_iter().collect();
        if survivors.len() <= 1 {
            return Ok(OperatorOutcome::no_change(Cost::work(1)).with_note(format!(
                "{} survivor(s): nothing left to split",
                survivors.len()
            )));
        }
        let observed = observed_set(inquiry, arity);
        let mut best: Option<(usize, usize)> = None; // (worst side, row)
        let mut examined = 0u64;
        for row in 0..rows(arity) {
            if observed.contains(&row) {
                continue;
            }
            let ones = survivors
                .iter()
                .filter(|&&m| member_bit(&bits, &layout, m, row))
                .count();
            examined += survivors.len() as u64;
            let zeros = survivors.len() - ones;
            if ones == 0 || zeros == 0 {
                continue;
            }
            let worst = ones.max(zeros);
            if best.is_none_or(|(w, _)| worst < w) {
                best = Some((worst, row));
            }
        }
        let Some((worst, row)) = best else {
            return Ok(OperatorOutcome::no_change(Cost::work(examined.max(1)))
                .with_note("no unobserved row separates the survivors"));
        };
        if world.probes_remaining() == Some(0) {
            return Ok(OperatorOutcome::no_change(Cost::work(examined.max(1)))
                .with_note("the protocol's probe cap is spent"));
        }
        let probe = probe_for_row(row, arity);
        let observation = world.probe(&OperatorId::from(Self::ID), inquiry.id, &probe)?;
        let output = result_output(&observation.result);
        inquiry.record_observation(observation);
        write_observations_view(inquiry, Self::ID, arity);
        Ok(
            OperatorOutcome::progressed(Cost::work(examined.max(1))).with_note(format!(
                "row {row} -> {} (worst side {worst} of {} survivors)",
                output.map_or("?".to_owned(), |b| u8::from(b).to_string()),
                survivors.len()
            )),
        )
    }
}
