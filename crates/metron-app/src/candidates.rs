//! Capability candidates proposed from episodes.
//!
//! An episode that reached an answer executed a trace of operators. The
//! trace is specific to its target (it probed as often as that target
//! needed), so a reusable candidate generalises it: maximal repeated units
//! become repeatable sequences, and the number of repetitions is left to
//! the budget. The candidate carries the distinct operators along the
//! answer's provenance, a claimed end-to-end contract composed from the
//! contracts along that path, the answer's evidence, and the generalised
//! schedule as its program.
//!
//! Proposing is the system's job; judging is the laboratory's.

use crate::schedule::ScheduleItem;
use metron_core::capability::CapabilityCandidate;
use metron_core::frame::{Fidelity, TransformContract};
use metron_core::id::{EpisodeId, OperatorId, ViewId};
use metron_core::inquiry::Inquiry;
use metron_core::journal::{Episode, JournalEvent};
use metron_core::operator::StepStatus;
use std::collections::BTreeSet;

/// Compresses an executed trace into schedule items: a maximal run of a
/// repeated unit (of length one to three) becomes a sequence repeated
/// `rounds` times; everything else is applied once.
#[must_use]
pub fn compress_trace(trace: &[OperatorId], rounds: u32) -> Vec<ScheduleItem> {
    let mut items = Vec::new();
    let mut i = 0;
    while i < trace.len() {
        let mut best: Option<(usize, usize)> = None; // (unit length, repeats)
        for len in 1..=3.min(trace.len() - i) {
            let unit = &trace[i..i + len];
            let mut repeats = 1;
            while i + (repeats + 1) * len <= trace.len()
                && &trace[i + repeats * len..i + (repeats + 1) * len] == unit
            {
                repeats += 1;
            }
            if repeats >= 2 && best.is_none_or(|(l, r)| repeats * len > r * l) {
                best = Some((len, repeats));
            }
        }
        match best {
            Some((len, repeats)) => {
                let unit: Vec<ScheduleItem> = trace[i..i + len]
                    .iter()
                    .map(|op| ScheduleItem::once(op.clone()))
                    .collect();
                items.push(ScheduleItem::sequence(unit, rounds));
                i += len * repeats;
            }
            None => {
                items.push(ScheduleItem::once(trace[i].clone()));
                i += 1;
            }
        }
    }
    items
}

/// The operators an episode applied, in order, excluding applications
/// that changed nothing.
#[must_use]
pub fn executed_trace(episode: &Episode) -> Vec<OperatorId> {
    episode
        .entries
        .iter()
        .filter_map(|e| match &e.event {
            JournalEvent::OperatorApplied {
                operator, status, ..
            } if *status != StepStatus::NoChange => Some(operator.clone()),
            _ => None,
        })
        .collect()
}

/// Composes the contracts along a provenance path into one claimed
/// contract: the first source frames, the last target frame, every
/// assumption and loss, the final preserved properties, and approximate
/// fidelity if any step was approximate.
#[must_use]
pub fn compose_contracts(contracts: &[TransformContract]) -> Option<TransformContract> {
    let first = contracts.first()?;
    let last = contracts.last()?;
    let mut composed = TransformContract::new(first.from.iter().cloned(), last.to.clone());
    composed.preserves = last.preserves.clone();
    let mut seen = BTreeSet::new();
    for c in contracts {
        for l in &c.loses {
            if seen.insert(("loses", l.clone())) {
                composed.loses.push(l.clone());
            }
        }
        for a in &c.assumes {
            if seen.insert(("assumes", a.clone())) {
                composed.assumes.push(a.clone());
            }
        }
    }
    let approximate: Vec<String> = contracts
        .iter()
        .filter_map(|c| match &c.fidelity {
            Fidelity::Approximate { description } => Some(description.clone()),
            Fidelity::Exact => None,
        })
        .collect();
    if !approximate.is_empty() {
        composed = composed.approximate(approximate.join("; "));
    }
    Some(composed)
}

/// Proposes a candidate from an answered inquiry and its episode, or
/// `None` if the inquiry has no answer.
#[must_use]
pub fn propose(inquiry: &Inquiry, episode: &Episode, rounds: u32) -> Option<CapabilityCandidate> {
    let answer = inquiry.answer.as_ref()?;
    // The view the answer was committed from: same frame and content.
    let root: Option<ViewId> = inquiry
        .views
        .iter()
        .find(|(_, v)| v.frame == answer.frame && v.content == answer.content)
        .map(|(id, _)| id.clone());
    let mut path: Vec<OperatorId> = Vec::new();
    let mut contracts: Vec<TransformContract> = Vec::new();
    if let Some(root) = root {
        let mut seen = BTreeSet::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Some(v) = inquiry.views.get(&id) {
                if !path.contains(&v.derivation.operator) {
                    path.push(v.derivation.operator.clone());
                }
                if let Some(c) = &v.derivation.contract {
                    contracts.push(c.clone());
                }
                stack.extend(v.derivation.inputs.iter().cloned());
            }
        }
        path.reverse();
        contracts.reverse();
    }
    path.push(answer.by.clone());
    let trace = executed_trace(episode);
    let program = compress_trace(&trace, rounds);
    let name = path
        .iter()
        .map(|o| o.as_str())
        .collect::<Vec<_>>()
        .join(" -> ");
    Some(CapabilityCandidate {
        name: name.clone(),
        description: format!(
            "Composition that answered {} in episode {}",
            inquiry.question.kind, episode.id
        ),
        composition: path,
        claimed_contract: compose_contracts(&contracts),
        evidence: answer.evidence.clone(),
        proposed_in: EpisodeId(episode.id.0),
        program: serde_json::to_value(&program).expect("schedules are always serialisable"),
    })
}

/// The schedule stored in a candidate's program, if any.
#[must_use]
pub fn program_schedule(candidate: &CapabilityCandidate) -> Option<Vec<ScheduleItem>> {
    serde_json::from_value(candidate.program.clone()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::FixedSchedule;

    fn ops(names: &[&str]) -> Vec<OperatorId> {
        names.iter().map(|n| OperatorId::from(*n)).collect()
    }

    #[test]
    fn repeated_units_become_repeatable_sequences() {
        let trace = ops(&["f", "p", "f", "p", "f", "p", "f", "s", "c"]);
        let items = compress_trace(&trace, 5);
        assert_eq!(items.len(), 4, "{items:?}");
        assert_eq!(
            items[0],
            ScheduleItem::sequence([ScheduleItem::once("f"), ScheduleItem::once("p")], 5)
        );
        assert_eq!(items[1], ScheduleItem::once("f"));
        assert_eq!(items[2], ScheduleItem::once("s"));
        assert_eq!(items[3], ScheduleItem::once("c"));
        assert_eq!(FixedSchedule::new(items).len(), 1 + 10 + 2);
        let single = compress_trace(&ops(&["a", "b", "c"]), 3);
        assert_eq!(single.len(), 3);
        let runs = compress_trace(&ops(&["a", "a", "a", "b"]), 2);
        assert_eq!(
            runs[0],
            ScheduleItem::sequence([ScheduleItem::once("a")], 2)
        );
        assert_eq!(runs[1], ScheduleItem::once("b"));
    }

    #[test]
    fn contracts_compose_end_to_end() {
        let a = TransformContract::new(["x"], "y")
            .loses("order")
            .assumes("pool");
        let b = TransformContract::new(["y"], "z")
            .preserves("rows")
            .assumes("pool")
            .approximate("guessed");
        let c = compose_contracts(&[a, b]).unwrap();
        assert_eq!(c.from.len(), 1);
        assert_eq!(c.to.as_str(), "z");
        assert_eq!(c.assumes, vec!["pool".to_owned()]);
        assert_eq!(c.loses, vec!["order".to_owned()]);
        assert_eq!(c.preserves, vec!["rows".to_owned()]);
        assert!(!c.is_exact());
        assert!(compose_contracts(&[]).is_none());
    }
}
