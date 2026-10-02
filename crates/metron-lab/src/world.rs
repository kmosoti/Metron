//! The laboratory world: an oracle over a sealed hidden function.

use crate::hidden::HiddenFunction;
use crate::truth_table::TruthTable;
use crate::verdict::Verdict;
use crate::{ANSWER_FRAME, FAMILY_HIDDEN_BOOLEAN};
use metron_core::cost::Cost;
use metron_core::evidence::Observation;
use metron_core::hash::ContentHash;
use metron_core::id::{FrameId, InquiryId, ObservationId, OperatorId, ReceiptId};
use metron_core::inquiry::{Inquiry, Question, Representation};
use metron_core::receipt::{OperationKind, ResourceReceipt};
use metron_core::world::{Oracle, OracleError};
use std::time::Instant;

/// The experimental protocol for one episode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Protocol {
    /// Probes the oracle will answer; `None` means uncapped.
    pub max_probes: Option<u64>,
}

/// An oracle over a hidden Boolean function, plus the judge.
#[derive(Debug)]
pub struct LabWorld {
    target: HiddenFunction,
    protocol: Protocol,
    probes_answered: u64,
    receipts: Vec<ResourceReceipt>,
    next_receipt: ReceiptId,
    next_observation: ObservationId,
}

impl LabWorld {
    /// Creates a world around a sealed target.
    #[must_use]
    pub fn new(target: HiddenFunction, protocol: Protocol) -> Self {
        Self {
            target,
            protocol,
            probes_answered: 0,
            receipts: Vec::new(),
            next_receipt: ReceiptId(1),
            next_observation: ObservationId(1),
        }
    }

    /// The question as the system is allowed to see it.
    #[must_use]
    pub fn question(&self) -> Question {
        let arity = self.target.arity();
        let rows = TruthTable::rows_for(arity);
        Question {
            kind: FAMILY_HIDDEN_BOOLEAN.into(),
            statement: format!(
                "Identify the hidden Boolean function of {arity} inputs by membership probes. \
                 A probe is {{\"assignment\": [b_0, ..., b_{}]}} with each b_j in {{0, 1}}; \
                 the result is {{\"output\": 0 or 1}}. Answer in frame `{ANSWER_FRAME}` with a \
                 bit vector of {rows} bits where bit i is f at the assignment whose x_j is bit j of i.",
                arity.saturating_sub(1)
            ),
            params: serde_json::json!({
                "arity": arity,
                "rows": rows,
                "max_probes": self.protocol.max_probes,
            }),
            answer_frame: FrameId::from(ANSWER_FRAME),
        }
    }

    /// Probes answered so far.
    #[must_use]
    pub const fn probes_answered(&self) -> u64 {
        self.probes_answered
    }

    /// The protocol.
    #[must_use]
    pub const fn protocol(&self) -> Protocol {
        self.protocol
    }

    /// Fingerprint of the hidden target.
    #[must_use]
    pub fn target_fingerprint(&self) -> ContentHash {
        self.target.fingerprint()
    }

    /// Judges the inquiry's committed answer against the hidden target.
    #[must_use]
    pub fn judge(&self, inquiry: &Inquiry) -> Verdict {
        let table = self.target.table();
        let rows_total = table.rows();
        let mut verdict = Verdict {
            answered: false,
            frame_accepted: false,
            correct: false,
            rows_total,
            rows_correct: 0,
            agreement: 0.0,
            probes_answered: self.probes_answered,
            cost: inquiry.spent,
            target_fingerprint: self.target.fingerprint(),
            reasons: Vec::new(),
        };
        let Some(answer) = &inquiry.answer else {
            verdict.reasons.push("no answer was committed".into());
            return verdict;
        };
        verdict.answered = true;
        if answer.frame.as_str() != ANSWER_FRAME {
            verdict.reasons.push(format!(
                "answer frame `{}` is not `{ANSWER_FRAME}`",
                answer.frame
            ));
            return verdict;
        }
        let Some(bits) = answer.content.as_bits() else {
            verdict
                .reasons
                .push("answer content is not a bit vector".into());
            return verdict;
        };
        let Some(rows_correct) = table.agreement(bits) else {
            verdict.reasons.push(format!(
                "answer has {} rows, the target has {rows_total}",
                bits.dim()
            ));
            return verdict;
        };
        verdict.frame_accepted = true;
        verdict.rows_correct = rows_correct;
        verdict.agreement = rows_correct as f64 / rows_total as f64;
        verdict.correct = rows_correct == rows_total;
        if !verdict.correct {
            verdict.reasons.push(format!(
                "{} of {rows_total} rows disagree with the target",
                rows_total - rows_correct
            ));
        }
        verdict
    }

    fn parse_probe(&self, probe: &Representation) -> Result<usize, OracleError> {
        let arity = usize::from(self.target.arity());
        let value = probe
            .as_json()
            .ok_or_else(|| OracleError::MalformedProbe("probe must be JSON".into()))?;
        let assignment = value
            .get("assignment")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                OracleError::MalformedProbe("probe needs an `assignment` array".into())
            })?;
        if assignment.len() != arity {
            return Err(OracleError::MalformedProbe(format!(
                "assignment has {} entries, the function has {arity} inputs",
                assignment.len()
            )));
        }
        let mut bools = Vec::with_capacity(arity);
        for v in assignment {
            let b = match v {
                serde_json::Value::Bool(b) => *b,
                serde_json::Value::Number(n) if n.as_u64() == Some(0) => false,
                serde_json::Value::Number(n) if n.as_u64() == Some(1) => true,
                other => {
                    return Err(OracleError::MalformedProbe(format!(
                        "assignment entry {other} is not 0, 1, true or false"
                    )));
                }
            };
            bools.push(b);
        }
        Ok(TruthTable::row_index(&bools))
    }
}

impl Oracle for LabWorld {
    fn probe(
        &mut self,
        operator: &OperatorId,
        inquiry: InquiryId,
        probe: &Representation,
    ) -> Result<Observation, OracleError> {
        let started = Instant::now();
        let row = self.parse_probe(probe)?;
        if self
            .protocol
            .max_probes
            .is_some_and(|cap| self.probes_answered >= cap)
        {
            return Err(OracleError::BudgetExhausted);
        }
        let output = self.target.evaluate(row);
        self.probes_answered += 1;
        let result = Representation::Json(serde_json::json!({ "output": u8::from(output) }));
        let observation_id = self.next_observation;
        self.next_observation = observation_id.next();
        let receipt_id = self.next_receipt;
        self.next_receipt = receipt_id.next();
        let receipt = ResourceReceipt::seal(
            receipt_id,
            OperationKind::OracleProbe,
            operator.clone(),
            inquiry,
            probe.content_hash(),
            result.content_hash(),
            Cost::probes(1),
            vec![observation_id],
            u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
        );
        self.receipts.push(receipt);
        Ok(Observation {
            id: observation_id,
            receipt: receipt_id,
            probe: probe.clone(),
            result,
        })
    }

    fn drain_receipts(&mut self) -> Vec<ResourceReceipt> {
        std::mem::take(&mut self.receipts)
    }

    fn probes_remaining(&self) -> Option<u64> {
        self.protocol
            .max_probes
            .map(|cap| cap.saturating_sub(self.probes_answered))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metron_core::bits::BitVector;
    use metron_core::inquiry::Answer;

    fn world(cap: Option<u64>) -> LabWorld {
        LabWorld::new(
            HiddenFunction::new(TruthTable::parse_rows(2, "0110").unwrap()),
            Protocol { max_probes: cap },
        )
    }

    fn probe(bits: &[u8]) -> Representation {
        Representation::Json(serde_json::json!({ "assignment": bits }))
    }

    #[test]
    fn probes_issue_receipts_and_respect_the_cap() {
        let mut w = world(Some(2));
        let op = OperatorId::from("probe");
        let a = w.probe(&op, InquiryId(1), &probe(&[1, 0])).unwrap();
        assert_eq!(a.result.as_json().unwrap()["output"], 1);
        let b = w.probe(&op, InquiryId(1), &probe(&[1, 1])).unwrap();
        assert_eq!(b.result.as_json().unwrap()["output"], 0);
        assert_eq!(w.probes_remaining(), Some(0));
        assert_eq!(
            w.probe(&op, InquiryId(1), &probe(&[0, 0])),
            Err(OracleError::BudgetExhausted)
        );
        let receipts = w.drain_receipts();
        assert_eq!(receipts.len(), 2);
        assert!(receipts.iter().all(ResourceReceipt::verify));
        assert_eq!(receipts[0].observations, vec![a.id]);
        assert_eq!(receipts[1].id, b.receipt);
        assert!(w.drain_receipts().is_empty());
        assert!(matches!(
            w.probe(&op, InquiryId(1), &probe(&[1])),
            Err(OracleError::MalformedProbe(_))
        ));
    }

    #[test]
    fn judge_compares_answers_to_the_target() {
        let w = world(None);
        let mut inquiry = Inquiry::new(InquiryId(1), w.question());
        assert_eq!(inquiry.question.param_u64("arity"), Some(2));
        let v = w.judge(&inquiry);
        assert!(!v.answered && !v.correct);
        inquiry.commit(Answer {
            frame: FrameId::from(ANSWER_FRAME),
            content: Representation::Bits(BitVector::from_bools(&[false, true, true, false])),
            confidence: 1.0,
            evidence: Vec::new(),
            by: OperatorId::from("commit"),
        });
        let v = w.judge(&inquiry);
        assert!(v.frame_accepted && v.correct);
        assert_eq!(v.agreement, 1.0);
        inquiry.commit(Answer {
            frame: FrameId::from(ANSWER_FRAME),
            content: Representation::Bits(BitVector::from_bools(&[false, true, false, false])),
            confidence: 0.5,
            evidence: Vec::new(),
            by: OperatorId::from("commit"),
        });
        let v = w.judge(&inquiry);
        assert!(v.frame_accepted && !v.correct);
        assert_eq!(v.rows_correct, 3);
        inquiry.commit(Answer {
            frame: FrameId::from("dnf"),
            content: Representation::Text("x0 xor x1".into()),
            confidence: 1.0,
            evidence: Vec::new(),
            by: OperatorId::from("commit"),
        });
        assert!(!w.judge(&inquiry).frame_accepted);
    }
}
