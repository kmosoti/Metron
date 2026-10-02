//! The inquiry: the system's working state about one question.

use crate::bits::BitVector;
use crate::cost::{Budget, Cost};
use crate::evidence::{Evidence, Observation};
use crate::frame::TransformContract;
use crate::hash::ContentHash;
use crate::id::{FrameId, InquiryId, ObservationId, OperatorId, ViewId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The content of a view, a probe, a result or an answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Representation {
    /// Free text (a formula, a program, a description).
    Text(String),
    /// Structured data.
    Json(serde_json::Value),
    /// A binary vector (a truth table, a mask).
    Bits(BitVector),
}

impl Representation {
    /// The text, if this is a `Text`.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Representation::Text(s) => Some(s),
            _ => None,
        }
    }

    /// The value, if this is a `Json`.
    #[must_use]
    pub fn as_json(&self) -> Option<&serde_json::Value> {
        match self {
            Representation::Json(v) => Some(v),
            _ => None,
        }
    }

    /// The bits, if this is a `Bits`.
    #[must_use]
    pub fn as_bits(&self) -> Option<&BitVector> {
        match self {
            Representation::Bits(b) => Some(b),
            _ => None,
        }
    }

    /// Content hash of the canonical serialisation.
    #[must_use]
    pub fn content_hash(&self) -> ContentHash {
        ContentHash::of_json(self).expect("representations are always serialisable")
    }

    /// A short, single-line description for logs.
    #[must_use]
    pub fn summary(&self) -> String {
        fn clip(s: &str) -> String {
            let mut t: String = s.chars().take(60).collect();
            if s.chars().count() > 60 {
                t.push('…');
            }
            t
        }
        match self {
            Representation::Text(s) => format!("text({:?})", clip(s)),
            Representation::Json(v) => format!("json({})", clip(&v.to_string())),
            Representation::Bits(b) => format!("bits<{}>(ones={})", b.dim(), b.count_ones()),
        }
    }
}

/// What the system is asked. Everything in here is visible to the system;
/// nothing about the hidden target is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Question {
    /// Task family, e.g. `hidden-boolean-function`.
    pub kind: String,
    /// Human-readable statement.
    pub statement: String,
    /// Public parameters (e.g. the number of inputs).
    #[serde(default)]
    pub params: serde_json::Value,
    /// The frame an answer must be given in to be judged.
    pub answer_frame: FrameId,
}

impl Question {
    /// Reads an unsigned integer parameter.
    #[must_use]
    pub fn param_u64(&self, name: &str) -> Option<u64> {
        self.params.get(name).and_then(serde_json::Value::as_u64)
    }
}

/// How a view's current content was produced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Derivation {
    /// Operator that wrote the view.
    pub operator: OperatorId,
    /// Step at which it was written.
    pub step: u32,
    /// Views it was computed from.
    #[serde(default)]
    pub inputs: Vec<ViewId>,
    /// Observations it incorporates directly.
    #[serde(default)]
    pub observations: Vec<ObservationId>,
    /// The contract under which it was written (stamped by the runner for
    /// transform operators; `None` for the observations frame).
    #[serde(default)]
    pub contract: Option<TransformContract>,
}

impl Derivation {
    /// A derivation by `operator` at `step` with no inputs.
    #[must_use]
    pub fn by(operator: OperatorId, step: u32) -> Self {
        Self {
            operator,
            step,
            inputs: Vec::new(),
            observations: Vec::new(),
            contract: None,
        }
    }

    /// Adds an input view.
    #[must_use]
    pub fn from_view(mut self, view: impl Into<ViewId>) -> Self {
        self.inputs.push(view.into());
        self
    }

    /// Adds directly incorporated observations.
    #[must_use]
    pub fn with_observations(
        mut self,
        observations: impl IntoIterator<Item = ObservationId>,
    ) -> Self {
        self.observations.extend(observations);
        self
    }
}

/// A representation of the inquiry's state in some frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct View {
    /// The frame.
    pub frame: FrameId,
    /// The content.
    pub content: Representation,
    /// Incremented on every write.
    pub version: u32,
    /// Provenance of the current content.
    pub derivation: Derivation,
}

/// A committed answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    /// Frame of the answer.
    pub frame: FrameId,
    /// Content.
    pub content: Representation,
    /// Confidence in `[0, 1]`, as stated by the committing operator.
    pub confidence: f64,
    /// Evidence the answer rests on.
    #[serde(default)]
    pub evidence: Vec<Evidence>,
    /// Operator that committed.
    pub by: OperatorId,
}

/// The system's working state about one question.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Inquiry {
    /// Identifier.
    pub id: InquiryId,
    /// The question.
    pub question: Question,
    /// Named views.
    pub views: BTreeMap<ViewId, View>,
    /// Observations obtained so far, in order.
    pub observations: Vec<Observation>,
    /// The committed answer, if any.
    pub answer: Option<Answer>,
    /// Resources spent so far.
    pub spent: Cost,
    /// Resource limits.
    pub budget: Budget,
    /// Operator applications so far.
    pub steps: u32,
}

impl Inquiry {
    /// Creates an inquiry with an unlimited budget.
    #[must_use]
    pub fn new(id: InquiryId, question: Question) -> Self {
        Self {
            id,
            question,
            views: BTreeMap::new(),
            observations: Vec::new(),
            answer: None,
            spent: Cost::ZERO,
            budget: Budget::UNLIMITED,
            steps: 0,
        }
    }

    /// Sets the budget.
    #[must_use]
    pub fn with_budget(mut self, budget: Budget) -> Self {
        self.budget = budget;
        self
    }

    /// Looks up a view.
    #[must_use]
    pub fn view(&self, id: &ViewId) -> Option<&View> {
        self.views.get(id)
    }

    /// Looks up a view by name.
    #[must_use]
    pub fn view_str(&self, id: &str) -> Option<&View> {
        self.views.get(&ViewId::from(id))
    }

    /// Whether a view exists.
    #[must_use]
    pub fn has_view(&self, id: &ViewId) -> bool {
        self.views.contains_key(id)
    }

    /// Writes a view, returning its version.
    ///
    /// Writing the same frame and content that are already there is a no-op
    /// that keeps the existing version and derivation, so repeated
    /// applications of an idle operator do not look like progress.
    pub fn write_view(
        &mut self,
        id: impl Into<ViewId>,
        frame: impl Into<FrameId>,
        content: Representation,
        derivation: Derivation,
    ) -> u32 {
        let id = id.into();
        let frame = frame.into();
        if let Some(existing) = self.views.get(&id)
            && existing.frame == frame
            && existing.content == content
        {
            return existing.version;
        }
        let version = self.views.get(&id).map_or(1, |v| v.version + 1);
        self.views.insert(
            id,
            View {
                frame,
                content,
                version,
                derivation,
            },
        );
        version
    }

    /// Records an observation obtained from the oracle.
    pub fn record_observation(&mut self, observation: Observation) {
        self.observations.push(observation);
    }

    /// Identifiers of all recorded observations.
    #[must_use]
    pub fn observation_ids(&self) -> Vec<ObservationId> {
        self.observations.iter().map(|o| o.id).collect()
    }

    /// All observations a view ultimately rests on, following input views
    /// transitively. This is what an answer's evidence should cite.
    #[must_use]
    pub fn observation_closure(&self, view: &ViewId) -> BTreeSet<ObservationId> {
        let mut out = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let mut stack = vec![view.clone()];
        while let Some(id) = stack.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Some(v) = self.views.get(&id) {
                out.extend(v.derivation.observations.iter().copied());
                stack.extend(v.derivation.inputs.iter().cloned());
            }
        }
        out
    }

    /// Whether an answer has been committed.
    #[must_use]
    pub fn is_answered(&self) -> bool {
        self.answer.is_some()
    }

    /// Commits an answer. A second commit replaces the first.
    pub fn commit(&mut self, answer: Answer) {
        self.answer = Some(answer);
    }

    /// Hash of the epistemic state: views, observations and answer.
    ///
    /// Cost and step count are excluded so the hash answers "did this step
    /// change what the system knows?".
    #[must_use]
    pub fn state_hash(&self) -> ContentHash {
        #[derive(Serialize)]
        struct State<'a> {
            views: &'a BTreeMap<ViewId, View>,
            observations: &'a [Observation],
            answer: &'a Option<Answer>,
        }
        ContentHash::of_json(&State {
            views: &self.views,
            observations: &self.observations,
            answer: &self.answer,
        })
        .expect("inquiry state is always serialisable")
    }

    /// Frame and version of every view, for change detection.
    #[must_use]
    pub fn view_versions(&self) -> BTreeMap<ViewId, (FrameId, u32)> {
        self.views
            .iter()
            .map(|(id, v)| (id.clone(), (v.frame.clone(), v.version)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::ReceiptId;

    fn question() -> Question {
        Question {
            kind: "demo".into(),
            statement: "demo".into(),
            params: serde_json::json!({"n": 3}),
            answer_frame: FrameId::from("answer"),
        }
    }

    #[test]
    fn state_hash_tracks_epistemic_state_only() {
        let mut q = Inquiry::new(InquiryId(1), question());
        assert_eq!(q.question.param_u64("n"), Some(3));
        let h0 = q.state_hash();
        q.spent += Cost::operator_call();
        q.steps += 1;
        assert_eq!(q.state_hash(), h0);
        let v = q.write_view(
            "x",
            "frame",
            Representation::Text("hello".into()),
            Derivation::by(OperatorId::from("op"), 0),
        );
        assert_eq!(v, 1);
        assert_ne!(q.state_hash(), h0);
        assert_eq!(
            q.write_view(
                "x",
                "frame",
                Representation::Text("again".into()),
                Derivation::by(OperatorId::from("op"), 1),
            ),
            2
        );
        let h2 = q.state_hash();
        assert_eq!(
            q.write_view(
                "x",
                "frame",
                Representation::Text("again".into()),
                Derivation::by(OperatorId::from("op"), 2),
            ),
            2,
            "identical rewrite keeps the version"
        );
        assert_eq!(q.state_hash(), h2);
    }

    #[test]
    fn observation_closure_follows_inputs() {
        let mut q = Inquiry::new(InquiryId(1), question());
        let obs = |i: u64| Observation {
            id: ObservationId(i),
            receipt: ReceiptId(i),
            probe: Representation::Text("p".into()),
            result: Representation::Text("r".into()),
        };
        q.record_observation(obs(1));
        q.record_observation(obs(2));
        q.write_view(
            "raw",
            "observations",
            Representation::Text("..".into()),
            Derivation::by(OperatorId::from("probe"), 0)
                .with_observations([ObservationId(1), ObservationId(2)]),
        );
        q.write_view(
            "derived",
            "other",
            Representation::Text("..".into()),
            Derivation::by(OperatorId::from("transform"), 1).from_view("raw"),
        );
        q.write_view(
            "loop",
            "other",
            Representation::Text("..".into()),
            Derivation::by(OperatorId::from("transform"), 2)
                .from_view("derived")
                .from_view("loop"),
        );
        let closure = q.observation_closure(&ViewId::from("loop"));
        assert_eq!(closure.len(), 2);
        assert!(q.observation_closure(&ViewId::from("missing")).is_empty());
        assert_eq!(
            q.observation_ids(),
            vec![ObservationId(1), ObservationId(2)]
        );
    }
}
