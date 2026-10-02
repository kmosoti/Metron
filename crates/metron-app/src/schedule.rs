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
}

/// One item of a fixed schedule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleItem {
    /// The operator.
    pub operator: OperatorId,
    /// How many times in a row to apply it.
    #[serde(default = "one")]
    pub repeat: u32,
}

const fn one() -> u32 {
    1
}

impl ScheduleItem {
    /// Apply `operator` once.
    pub fn once(operator: impl Into<OperatorId>) -> Self {
        Self {
            operator: operator.into(),
            repeat: 1,
        }
    }

    /// Apply `operator` `repeat` times.
    pub fn times(operator: impl Into<OperatorId>, repeat: u32) -> Self {
        Self {
            operator: operator.into(),
            repeat,
        }
    }
}

/// A hand-authored, fixed sequence of operators.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixedSchedule {
    items: Vec<OperatorId>,
    cursor: usize,
}

impl FixedSchedule {
    /// Builds a schedule from items, expanding repeats.
    pub fn new(items: impl IntoIterator<Item = ScheduleItem>) -> Self {
        let mut flat = Vec::new();
        for item in items {
            for _ in 0..item.repeat {
                flat.push(item.operator.clone());
            }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use metron_core::id::{FrameId, InquiryId};
    use metron_core::inquiry::Question;

    #[test]
    fn repeats_expand_in_order() {
        let mut s = FixedSchedule::new([ScheduleItem::times("a", 2), ScheduleItem::once("b")]);
        let q = Inquiry::new(
            InquiryId(0),
            Question {
                kind: "k".into(),
                statement: "s".into(),
                params: serde_json::Value::Null,
                answer_frame: FrameId::from("f"),
            },
        );
        assert_eq!(s.len(), 3);
        assert_eq!(s.next(&q), Some(OperatorId::from("a")));
        assert_eq!(s.next(&q), Some(OperatorId::from("a")));
        assert_eq!(s.remaining(), 1);
        assert_eq!(s.next(&q), Some(OperatorId::from("b")));
        assert_eq!(s.next(&q), None);
        assert_eq!(s.next(&q), None);
    }
}
