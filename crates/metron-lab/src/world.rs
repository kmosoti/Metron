//! The laboratory world: an oracle over a sealed hidden function, the
//! public knowledge that comes with the question, and the judge.

use crate::families::Family;
use crate::hidden::HiddenFunction;
use crate::pool::HypothesisPool;
use crate::tasks::{Task, TaskSet};
use crate::truth_table::TruthTable;
use crate::verdict::Verdict;
use crate::{ANSWER_FRAME, FAMILY_HIDDEN_BOOLEAN};
use metron_core::cost::Cost;
use metron_core::evidence::Observation;
use metron_core::hash::ContentHash;
use metron_core::id::{FrameId, InquiryId, OperatorId};
use metron_core::inquiry::{Inquiry, Question, Representation};
use metron_core::receipt::{OperationKind, ResourceReceipt};
use metron_core::world::{Counter, IdSource, Knowledge, Oracle, OracleError, Receipts};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Instant;

/// The experimental protocol for one episode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Protocol {
    /// Probes the oracle will answer; `None` means uncapped.
    pub max_probes: Option<u64>,
}

/// Public state of a world, for checkpoints. Contains nothing hidden.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldState {
    /// Probes answered so far.
    pub probes_answered: u64,
    /// Identifier counters.
    pub counter: Counter,
}

/// An oracle over a hidden Boolean function, plus the knowledge the
/// question refers to, plus the judge.
#[derive(Debug)]
pub struct LabWorld {
    target: HiddenFunction,
    target_family: Option<Family>,
    target_description: Option<String>,
    protocol: Protocol,
    probes_answered: u64,
    receipts: Vec<ResourceReceipt>,
    counter: Counter,
    knowledge: BTreeMap<String, Representation>,
    pool_params: Option<serde_json::Value>,
}

impl LabWorld {
    /// Creates a world around a sealed target with no public knowledge.
    #[must_use]
    pub fn new(target: HiddenFunction, protocol: Protocol) -> Self {
        Self {
            target,
            target_family: None,
            target_description: None,
            protocol,
            probes_answered: 0,
            receipts: Vec::new(),
            counter: Counter::default(),
            knowledge: BTreeMap::new(),
            pool_params: None,
        }
    }

    /// Creates a world for one task of a task set: the task's target,
    /// sealed, with the set's pool as public knowledge.
    #[must_use]
    pub fn for_task(set: &TaskSet, task: &Task, protocol: Protocol) -> Self {
        Self::new(HiddenFunction::new(task.target.table.clone()), protocol)
            .with_pool(&set.pool)
            .with_target_info(task.target.family, task.target.description.clone())
    }

    /// Attaches a hypothesis pool as public knowledge.
    #[must_use]
    pub fn with_pool(mut self, pool: &HypothesisPool) -> Self {
        for (name, doc) in pool.documents() {
            self.knowledge.insert(name, doc);
        }
        self.pool_params = Some(pool.question_params());
        self
    }

    /// Records what the target is, for the verdict. Never shown to the
    /// system.
    #[must_use]
    pub fn with_target_info(mut self, family: Family, description: impl Into<String>) -> Self {
        self.target_family = Some(family);
        self.target_description = Some(description.into());
        self
    }

    /// The question as the system is allowed to see it.
    #[must_use]
    pub fn question(&self) -> Question {
        let arity = self.target.arity();
        let rows = TruthTable::rows_for(arity);
        let mut params = serde_json::json!({
            "arity": arity,
            "rows": rows,
            "max_probes": self.protocol.max_probes,
        });
        if let Some(pool) = &self.pool_params {
            params["pool"] = pool.clone();
        }
        let pool_sentence = if self.pool_params.is_some() {
            " The function is a member of the published hypothesis pool (see the `pool` parameter and the documents it names)."
        } else {
            ""
        };
        Question {
            kind: FAMILY_HIDDEN_BOOLEAN.into(),
            statement: format!(
                "Identify the hidden Boolean function of {arity} inputs by membership probes. \
                 A probe is {{\"assignment\": [b_0, ..., b_{}]}} with each b_j in {{0, 1}}; \
                 the result is {{\"output\": 0 or 1}}. Answer in frame `{ANSWER_FRAME}` with a \
                 bit vector of {rows} bits where bit i is f at the assignment whose x_j is bit j of i.{pool_sentence}",
                arity.saturating_sub(1)
            ),
            params,
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

    /// Public state, for checkpoints.
    #[must_use]
    pub fn state(&self) -> WorldState {
        WorldState {
            probes_answered: self.probes_answered,
            counter: self.counter.clone(),
        }
    }

    /// Restores public state from a checkpoint.
    pub fn restore(&mut self, state: WorldState) {
        self.probes_answered = state.probes_answered;
        self.counter = state.counter;
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
            target_family: self.target_family.map(|f| f.label().to_owned()),
            target_description: self.target_description.clone(),
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

    /// Executes one probe, drawing identifiers from `ids`. Composed worlds
    /// that hold several ports use this with their shared counter.
    pub fn probe_using(
        &mut self,
        ids: &mut dyn IdSource,
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
        let observation_id = ids.next_observation();
        let receipt_id = ids.next_receipt();
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

impl Receipts for LabWorld {
    fn drain_receipts(&mut self) -> Vec<ResourceReceipt> {
        std::mem::take(&mut self.receipts)
    }
}

impl Oracle for LabWorld {
    fn probe(
        &mut self,
        operator: &OperatorId,
        inquiry: InquiryId,
        probe: &Representation,
    ) -> Result<Observation, OracleError> {
        let mut counter = std::mem::take(&mut self.counter);
        let result = self.probe_using(&mut counter, operator, inquiry, probe);
        self.counter = counter;
        result
    }

    fn probes_remaining(&self) -> Option<u64> {
        self.protocol
            .max_probes
            .map(|cap| cap.saturating_sub(self.probes_answered))
    }
}

impl Knowledge for LabWorld {
    fn document(&self, name: &str) -> Option<&Representation> {
        self.knowledge.get(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::families::FamilyParams;
    use crate::pool::{POOL_DOCUMENT, PoolSpec};
    use crate::tasks::TaskSetSpec;
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
        assert_ne!(a.id, b.id);
        assert!(w.drain_receipts().is_empty());
        assert!(matches!(
            w.probe(&op, InquiryId(1), &probe(&[1])),
            Err(OracleError::MalformedProbe(_))
        ));
        let state = w.state();
        assert_eq!(state.probes_answered, 2);
        let mut fresh = world(Some(2));
        fresh.restore(state);
        assert_eq!(fresh.probes_remaining(), Some(0));
    }

    #[test]
    fn judge_compares_answers_to_the_target() {
        let w = world(None).with_target_info(Family::Affine, "x0 ⊕ x1");
        let mut inquiry = Inquiry::new(InquiryId(1), w.question());
        assert_eq!(inquiry.question.param_u64("arity"), Some(2));
        let v = w.judge(&inquiry);
        assert!(!v.answered && !v.correct);
        assert_eq!(v.target_family.as_deref(), Some("affine"));
        inquiry.commit(Answer {
            frame: FrameId::from(ANSWER_FRAME),
            content: Representation::Bits(BitVector::from_bools(&[false, true, true, false])),
            confidence: 1.0,
            evidence: Vec::new(),
            by: OperatorId::from("commit"),
        });
        let v = w.judge(&inquiry);
        assert!(v.frame_accepted && v.correct);
        inquiry.commit(Answer {
            frame: FrameId::from("dnf"),
            content: Representation::Text("x0 xor x1".into()),
            confidence: 1.0,
            evidence: Vec::new(),
            by: OperatorId::from("commit"),
        });
        assert!(!w.judge(&inquiry).frame_accepted);
    }

    #[test]
    fn task_worlds_publish_the_pool_and_hide_the_target() {
        let spec = TaskSetSpec {
            arity: 4,
            pool: PoolSpec {
                families: vec![Family::Affine, Family::Monotone],
                per_family: 6,
                params: FamilyParams::default(),
            },
            targets_per_family: 2,
            split: Default::default(),
        };
        let set = TaskSet::generate(&spec, 1);
        let task = &set.tasks[0];
        let w = LabWorld::for_task(
            &set,
            task,
            Protocol {
                max_probes: Some(16),
            },
        );
        let q = w.question();
        assert_eq!(
            q.params["pool"]["count"].as_u64().unwrap(),
            set.pool.len() as u64
        );
        let doc = w.document(POOL_DOCUMENT).unwrap().as_bits().unwrap();
        assert_eq!(doc.dim(), set.pool.len() * 16);
        assert!(w.document("nope").is_none());
        let shown = format!("{w:?}");
        assert!(!shown.contains(&task.target.table.to_rows_string()));
        assert!(shown.contains("<redacted>"));
    }
}
