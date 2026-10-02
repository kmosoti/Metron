//! Shared helpers.

use crate::{frames, views};
use metron_core::bits::BitVector;
use metron_core::id::{OperatorId, ViewId};
use metron_core::inquiry::{Derivation, Inquiry, Representation};
use metron_core::operator::OperatorError;
use metron_core::world::Knowledge;
use std::collections::BTreeSet;

/// Number of inputs of the hidden function, from the public question.
pub(crate) fn arity(inquiry: &Inquiry) -> Result<usize, OperatorError> {
    let arity = inquiry
        .question
        .param_u64("arity")
        .ok_or_else(|| OperatorError::Internal("question has no `arity` parameter".into()))?;
    usize::try_from(arity)
        .ok()
        .filter(|&a| a <= 16)
        .ok_or_else(|| OperatorError::Internal(format!("unsupported arity {arity}")))
}

/// Number of rows for `arity` inputs.
pub(crate) const fn rows(arity: usize) -> usize {
    1usize << arity
}

/// Row index of a probe `{"assignment": [...]}`, if well formed.
pub(crate) fn probe_row(probe: &Representation, arity: usize) -> Option<usize> {
    let assignment = probe.as_json()?.get("assignment")?.as_array()?;
    if assignment.len() != arity {
        return None;
    }
    let mut row = 0usize;
    for (j, v) in assignment.iter().enumerate() {
        let bit = match v {
            serde_json::Value::Bool(b) => *b,
            serde_json::Value::Number(n) => n.as_u64()? == 1,
            _ => return None,
        };
        if bit {
            row |= 1 << j;
        }
    }
    Some(row)
}

/// Output of a result `{"output": 0|1}`, if well formed.
pub(crate) fn result_output(result: &Representation) -> Option<bool> {
    match result.as_json()?.get("output")? {
        serde_json::Value::Bool(b) => Some(*b),
        serde_json::Value::Number(n) => Some(n.as_u64()? == 1),
        _ => None,
    }
}

/// The probe for a row index.
pub(crate) fn probe_for_row(row: usize, arity: usize) -> Representation {
    let assignment: Vec<u8> = (0..arity).map(|j| u8::from((row >> j) & 1 == 1)).collect();
    Representation::Json(serde_json::json!({ "assignment": assignment }))
}

/// Every observed `(row, output)` pair, in observation order.
pub(crate) fn observed_rows(inquiry: &Inquiry, arity: usize) -> Vec<(usize, bool)> {
    inquiry
        .observations
        .iter()
        .filter_map(|o| Some((probe_row(&o.probe, arity)?, result_output(&o.result)?)))
        .collect()
}

/// Rows already observed.
pub(crate) fn observed_set(inquiry: &Inquiry, arity: usize) -> BTreeSet<usize> {
    observed_rows(inquiry, arity)
        .into_iter()
        .map(|(r, _)| r)
        .collect()
}

/// Rewrites the observations view from the inquiry's observations. Observe
/// operators call this after recording what they obtained.
pub(crate) fn write_observations_view(inquiry: &mut Inquiry, operator: &str, arity: usize) {
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
        Derivation::by(OperatorId::from(operator), inquiry.steps).with_observations(ids),
    );
}

/// Where the published pool lives, from the question's `pool` parameter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PoolLayout {
    pub document: String,
    pub families_document: String,
    pub count: usize,
    pub rows: usize,
}

pub(crate) fn pool_layout(inquiry: &Inquiry) -> Result<PoolLayout, OperatorError> {
    let pool =
        inquiry.question.params.get("pool").ok_or_else(|| {
            OperatorError::Internal("question publishes no hypothesis pool".into())
        })?;
    let field = |name: &str| {
        pool.get(name)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| OperatorError::Internal(format!("pool parameter lacks `{name}`")))
    };
    let number = |name: &str| {
        pool.get(name)
            .and_then(serde_json::Value::as_u64)
            .and_then(|v| usize::try_from(v).ok())
            .ok_or_else(|| OperatorError::Internal(format!("pool parameter lacks `{name}`")))
    };
    Ok(PoolLayout {
        document: field("document")?,
        families_document: field("families_document")?,
        count: number("count")?,
        rows: number("rows")?,
    })
}

/// The pool's rows document.
pub(crate) fn pool_bits<'a, W: Knowledge>(
    world: &'a W,
    layout: &PoolLayout,
) -> Result<&'a BitVector, OperatorError> {
    let doc = world.document(&layout.document).ok_or_else(|| {
        OperatorError::Internal(format!("world has no `{}` document", layout.document))
    })?;
    let bits = doc
        .as_bits()
        .ok_or_else(|| OperatorError::Internal("pool document is not a bit vector".into()))?;
    if bits.dim() != layout.count * layout.rows {
        return Err(OperatorError::Internal(format!(
            "pool document has {} bits, expected {}",
            bits.dim(),
            layout.count * layout.rows
        )));
    }
    Ok(bits)
}

/// The pool's family labels.
pub(crate) fn pool_families<W: Knowledge>(
    world: &W,
    layout: &PoolLayout,
) -> Result<Vec<String>, OperatorError> {
    let doc = world.document(&layout.families_document).ok_or_else(|| {
        OperatorError::Internal(format!(
            "world has no `{}` document",
            layout.families_document
        ))
    })?;
    let labels: Vec<String> = doc
        .as_json()
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| OperatorError::Internal("families document is not a JSON array".into()))?
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_owned())
        .collect();
    if labels.len() != layout.count {
        return Err(OperatorError::Internal(
            "families document length does not match the pool".into(),
        ));
    }
    Ok(labels)
}

/// Value of pool member `member` at `row`.
pub(crate) fn member_bit(bits: &BitVector, layout: &PoolLayout, member: usize, row: usize) -> bool {
    bits.bit(member * layout.rows + row)
}

/// The version-space mask from the inquiry, if present and well formed.
pub(crate) fn version_space_mask<'a>(
    inquiry: &'a Inquiry,
    layout: &PoolLayout,
) -> Option<&'a BitVector> {
    let v = inquiry.view_str(views::VERSION_SPACE)?;
    let bits = v.content.as_bits()?;
    (bits.dim() == layout.count).then_some(bits)
}

/// Operators that produced a view and, transitively, its inputs, nearest
/// first.
pub(crate) fn provenance_path(inquiry: &Inquiry, view: &ViewId) -> Vec<OperatorId> {
    let mut path = Vec::new();
    let mut seen = BTreeSet::new();
    let mut stack = vec![view.clone()];
    while let Some(id) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if let Some(v) = inquiry.views.get(&id) {
            if !path.contains(&v.derivation.operator) {
                path.push(v.derivation.operator.clone());
            }
            stack.extend(v.derivation.inputs.iter().cloned());
        }
    }
    path.reverse();
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_round_trip() {
        for row in 0..8 {
            assert_eq!(probe_row(&probe_for_row(row, 3), 3), Some(row));
        }
        assert_eq!(probe_row(&probe_for_row(5, 3), 2), None);
        assert_eq!(
            result_output(&Representation::Json(serde_json::json!({"output": 1}))),
            Some(true)
        );
        assert_eq!(result_output(&Representation::Text("1".into())), None);
    }
}
