//! Schedules: who runs next.

use metron_core::id::OperatorId;
use metron_core::inquiry::Inquiry;
use serde::{Deserialize, Serialize};

/// Decides which operator to apply next.
pub trait Scheduler: Send {
    /// Name for the journal.
    fn name(&self) -> String;

    /// The next operator, or `None` when the schedule is exhausted.
    fn next(&mut self, inquiry: &Inquiry) -> Option<OperatorId>;

    /// Serialisable state, for checkpoints.
    fn state(&self) -> serde_json::Value {
        serde_json::Value::Null
    }

    /// Restores state saved by [`Scheduler::state`].
    fn restore(&mut self, _state: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }
}

const fn one() -> u32 {
    1
}

/// One item of a fixed schedule: an operator or a nested sequence, each
/// with a repeat count.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScheduleItem {
    /// Apply one operator `repeat` times in a row.
    Operator {
        /// The operator.
        operator: OperatorId,
        /// Consecutive applications.
        #[serde(default = "one")]
        repeat: u32,
    },
    /// Apply a sequence of items `repeat` times in a row.
    Sequence {
        /// The items.
        sequence: Vec<ScheduleItem>,
        /// Consecutive repetitions of the whole sequence.
        #[serde(default = "one")]
        repeat: u32,
    },
}

impl ScheduleItem {
    /// Apply `operator` once.
    pub fn once(operator: impl Into<OperatorId>) -> Self {
        Self::Operator {
            operator: operator.into(),
            repeat: 1,
        }
    }

    /// Apply `operator` `repeat` times.
    pub fn times(operator: impl Into<OperatorId>, repeat: u32) -> Self {
        Self::Operator {
            operator: operator.into(),
            repeat,
        }
    }

    /// Apply `items` `repeat` times.
    pub fn sequence(items: impl IntoIterator<Item = ScheduleItem>, repeat: u32) -> Self {
        Self::Sequence {
            sequence: items.into_iter().collect(),
            repeat,
        }
    }

    fn flatten_into(&self, out: &mut Vec<OperatorId>) {
        match self {
            ScheduleItem::Operator { operator, repeat } => {
                for _ in 0..*repeat {
                    out.push(operator.clone());
                }
            }
            ScheduleItem::Sequence { sequence, repeat } => {
                for _ in 0..*repeat {
                    for item in sequence {
                        item.flatten_into(out);
                    }
                }
            }
        }
    }
}

/// A hand-authored, fixed sequence of operators.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixedSchedule {
    items: Vec<OperatorId>,
    cursor: usize,
}

impl FixedSchedule {
    /// Builds a schedule from items, expanding repeats and sequences.
    pub fn new(items: impl IntoIterator<Item = ScheduleItem>) -> Self {
        let mut flat = Vec::new();
        for item in items {
            item.flatten_into(&mut flat);
        }
        Self {
            items: flat,
            cursor: 0,
        }
    }

    /// Builds a schedule applying each operator once.
    pub fn of<I, S>(operators: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OperatorId>,
    {
        Self::new(operators.into_iter().map(ScheduleItem::once))
    }

    /// Total number of applications in the schedule.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the schedule is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Applications not yet handed out.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.items.len().saturating_sub(self.cursor)
    }

    /// The flattened operator list.
    #[must_use]
    pub fn operators(&self) -> &[OperatorId] {
        &self.items
    }
}

impl Scheduler for FixedSchedule {
    fn name(&self) -> String {
        "fixed".into()
    }

    fn next(&mut self, _inquiry: &Inquiry) -> Option<OperatorId> {
        let item = self.items.get(self.cursor).cloned();
        if item.is_some() {
            self.cursor += 1;
        }
        item
    }

    fn state(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("schedules are always serialisable")
    }

    fn restore(&mut self, state: &serde_json::Value) -> Result<(), String> {
        let restored: FixedSchedule =
            serde_json::from_value(state.clone()).map_err(|e| e.to_string())?;
        if restored.items != self.items {
            return Err("checkpointed schedule does not match this schedule".into());
        }
        self.cursor = restored.cursor.min(self.items.len());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metron_core::id::{FrameId, InquiryId};
    use metron_core::inquiry::Question;

    fn inquiry() -> Inquiry {
        Inquiry::new(
            InquiryId(0),
            Question {
                kind: "k".into(),
                statement: "s".into(),
                params: serde_json::Value::Null,
                answer_frame: FrameId::from("f"),
            },
        )
    }

    #[test]
    fn repeats_and_sequences_expand_in_order() {
        let mut s = FixedSchedule::new([
            ScheduleItem::times("a", 2),
            ScheduleItem::sequence([ScheduleItem::once("b"), ScheduleItem::once("c")], 2),
            ScheduleItem::once("d"),
        ]);
        let q = inquiry();
        let expected = ["a", "a", "b", "c", "b", "c", "d"];
        assert_eq!(s.len(), expected.len());
        for e in expected {
            assert_eq!(s.next(&q), Some(OperatorId::from(e)));
        }
        assert_eq!(s.next(&q), None);
        assert_eq!(s.remaining(), 0);
    }

    #[test]
    fn state_round_trips_and_rejects_a_different_schedule() {
        let mut s = FixedSchedule::of(["a", "b", "c"]);
        let q = inquiry();
        s.next(&q);
        let state = s.state();
        let mut t = FixedSchedule::of(["a", "b", "c"]);
        t.restore(&state).unwrap();
        assert_eq!(t.next(&q), Some(OperatorId::from("b")));
        let mut other = FixedSchedule::of(["x"]);
        assert!(other.restore(&state).is_err());
    }

    #[test]
    fn items_parse_from_json() {
        let items: Vec<ScheduleItem> = serde_json::from_str(
            r#"[{"operator": "a", "repeat": 2}, {"sequence": [{"operator": "b"}], "repeat": 3}]"#,
        )
        .unwrap();
        assert_eq!(FixedSchedule::new(items).len(), 5);
    }
}
