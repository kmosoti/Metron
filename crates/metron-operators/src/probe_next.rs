//! Observe: probe the lowest unobserved row.

use crate::support::{arity, probe_for_row, probe_row, result_output, rows};
use crate::{frames, views};
use metron_core::cost::Cost;
use metron_core::id::OperatorId;
use metron_core::inquiry::{Derivation, Inquiry, Representation};
use metron_core::operator::{Operator, OperatorError, OperatorKind, OperatorOutcome, OperatorSpec};
use metron_core::rng::Rng;
use metron_core::world::Oracle;
use std::collections::BTreeSet;

/// Probes the lowest row index that has not been observed yet and keeps the
/// observations view up to date.
///
/// One unit of work per row scanned. This is deliberately the dumbest
/// possible probing policy; smarter ones (greedy splitting, family-specific
/// probes) are future operators, not changes to this one.
#[derive(Debug, Default, Clone, Copy)]
pub struct ProbeNextUnobserved;

impl ProbeNextUnobserved {
    /// The operator identifier.
    pub const ID: &'static str = "probe-next-unobserved";
}

impl<W: Oracle> Operator<W> for ProbeNextUnobserved {
    fn spec(&self) -> OperatorSpec {
        OperatorSpec::new(
            Self::ID,
            OperatorKind::Observe,
            "Probe the lowest-index row not yet observed and record the result.",
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
        let observed: BTreeSet<usize> = inquiry
            .observations
            .iter()
            .filter_map(|o| probe_row(&o.probe, arity))
            .collect();
        let total = rows(arity);
        let Some(row) = (0..total).find(|r| !observed.contains(r)) else {
            return Ok(OperatorOutcome::no_change(Cost::work(total as u64))
                .with_note("every row is already observed"));
        };
        let scanned = (row + 1) as u64;
        if world.probes_remaining() == Some(0) {
            return Ok(OperatorOutcome::no_change(Cost::work(scanned))
                .with_note("the protocol's probe cap is spent"));
        }
        let probe = probe_for_row(row, arity);
        let observation = world.probe(&OperatorId::from(Self::ID), inquiry.id, &probe)?;
        let output = result_output(&observation.result);
        inquiry.record_observation(observation);

        let listing: Vec<serde_json::Value> = inquiry
            .observations
            .iter()
            .map(|o| {
                serde_json::json!({
                    "observation": o.id.0,
                    "row": probe_row(&o.probe, arity),
                    "output": result_output(&o.result).map(u8::from),
                })
            })
            .collect();
        let ids = inquiry.observation_ids();
        inquiry.write_view(
            views::OBSERVATIONS,
            frames::OBSERVATIONS,
            Representation::Json(serde_json::Value::Array(listing)),
            Derivation::by(OperatorId::from(Self::ID), inquiry.steps).with_observations(ids),
        );
        Ok(
            OperatorOutcome::progressed(Cost::work(scanned)).with_note(format!(
                "row {row} -> {}",
                output.map_or("?".to_owned(), |b| u8::from(b).to_string())
            )),
        )
    }
}
