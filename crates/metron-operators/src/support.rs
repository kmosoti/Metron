//! Shared helpers.

use metron_core::id::{OperatorId, ViewId};
use metron_core::inquiry::{Inquiry, Representation};
use metron_core::operator::OperatorError;
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
