//! The operator contract.
//!
//! An operator is a capability of one of three kinds:
//!
//! * **observe** — asks the oracle and records what comes back, writing only
//!   the observations frame;
//! * **transform** — moves information between frames under a
//!   [`TransformContract`], writing only the contract's target frame;
//! * **consult** — asks an external service (a language model session) and
//!   records the answer in the consultations frame. What comes back is not
//!   evidence: a transform must verify it against observations before it
//!   can reach any other frame;
//! * **commit** — turns a view into the inquiry's answer.
//!
//! Operators are generic over the world `W` they need. Bounds on `W` (such as
//! [`crate::world::Oracle`]) are the only way an operator can reach outside.

use crate::cost::{BudgetDimension, Cost};
use crate::frame::{ContractError, TransformContract};
use crate::id::{OperatorId, ViewId};
use crate::inquiry::Inquiry;
use crate::rng::Rng;
use serde::{Deserialize, Serialize};

/// What kind of thing an operator does.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OperatorKind {
    /// Obtains observations from the oracle.
    Observe,
    /// Changes representation under a contract.
    Transform {
        /// The contract.
        contract: TransformContract,
    },
    /// Consults an external service.
    Consult {
        /// Service name, e.g. `llm`.
        service: String,
    },
    /// Commits an answer.
    Commit,
}

impl OperatorKind {
    /// Short tag for logs.
    #[must_use]
    pub const fn tag(&self) -> &'static str {
        match self {
            OperatorKind::Observe => "observe",
            OperatorKind::Transform { .. } => "transform",
            OperatorKind::Consult { .. } => "consult",
            OperatorKind::Commit => "commit",
        }
    }

    /// The contract, if this is a transform.
    #[must_use]
    pub const fn contract(&self) -> Option<&TransformContract> {
        match self {
            OperatorKind::Transform { contract } => Some(contract),
            _ => None,
        }
    }
}

/// Errors from validating an operator's spec.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpecError {
    /// The identifier was empty.
    #[error("operator id is empty")]
    EmptyId,
    /// A transform's contract was malformed.
    #[error("operator {operator}: {source}")]
    Contract {
        /// The operator.
        operator: OperatorId,
        /// The underlying error.
        #[source]
        source: ContractError,
    },
    /// A transform declared no output view.
    #[error("transform operator {0} declares no written view")]
    TransformWritesNothing(OperatorId),
    /// A commit operator declared written views.
    #[error("commit operator {0} must not write views")]
    CommitWritesViews(OperatorId),
    /// A consult operator named no service or no written view.
    #[error("consult operator {0} must name a service and a written view")]
    ConsultIncomplete(OperatorId),
}

/// Static description of an operator.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OperatorSpec {
    /// Identifier.
    pub id: OperatorId,
    /// Kind, including the transform contract where applicable.
    pub kind: OperatorKind,
    /// Views that must exist for the operator to apply.
    #[serde(default)]
    pub reads: Vec<ViewId>,
    /// Views the operator may write.
    #[serde(default)]
    pub writes: Vec<ViewId>,
    /// What the operator does, for people.
    pub description: String,
}

impl OperatorSpec {
    /// Creates a spec.
    pub fn new(
        id: impl Into<OperatorId>,
        kind: OperatorKind,
        description: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind,
            reads: Vec::new(),
            writes: Vec::new(),
            description: description.into(),
        }
    }

    /// Declares a read view.
    #[must_use]
    pub fn reads(mut self, view: impl Into<ViewId>) -> Self {
        self.reads.push(view.into());
        self
    }

    /// Declares a written view.
    #[must_use]
    pub fn writes(mut self, view: impl Into<ViewId>) -> Self {
        self.writes.push(view.into());
        self
    }

    /// Checks the spec is well formed.
    pub fn validate(&self) -> Result<(), SpecError> {
        if self.id.as_str().trim().is_empty() {
            return Err(SpecError::EmptyId);
        }
        match &self.kind {
            OperatorKind::Transform { contract } => {
                contract.validate().map_err(|source| SpecError::Contract {
                    operator: self.id.clone(),
                    source,
                })?;
                if self.writes.is_empty() {
                    return Err(SpecError::TransformWritesNothing(self.id.clone()));
                }
            }
            OperatorKind::Commit => {
                if !self.writes.is_empty() {
                    return Err(SpecError::CommitWritesViews(self.id.clone()));
                }
            }
            OperatorKind::Consult { service } => {
                if service.trim().is_empty() || self.writes.is_empty() {
                    return Err(SpecError::ConsultIncomplete(self.id.clone()));
                }
            }
            OperatorKind::Observe => {}
        }
        Ok(())
    }
}

/// What an application achieved.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum StepStatus {
    /// The inquiry moved forward.
    Progressed,
    /// Nothing useful happened.
    NoChange,
    /// An answer was committed.
    Answered,
    /// The operator ran but could not do its job.
    Failed {
        /// Why.
        message: String,
    },
}

/// Result of applying an operator.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OperatorOutcome {
    /// Status.
    pub status: StepStatus,
    /// Work performed inside the operator. The runner adds the operator call
    /// itself and whatever the oracle charged.
    pub work: Cost,
    /// Note for the journal.
    #[serde(default)]
    pub note: String,
}

impl OperatorOutcome {
    /// Progress.
    #[must_use]
    pub fn progressed(work: Cost) -> Self {
        Self {
            status: StepStatus::Progressed,
            work,
            note: String::new(),
        }
    }

    /// No change.
    #[must_use]
    pub fn no_change(work: Cost) -> Self {
        Self {
            status: StepStatus::NoChange,
            work,
            note: String::new(),
        }
    }

    /// Answered.
    #[must_use]
    pub fn answered(work: Cost) -> Self {
        Self {
            status: StepStatus::Answered,
            work,
            note: String::new(),
        }
    }

    /// Failed.
    #[must_use]
    pub fn failed(work: Cost, message: impl Into<String>) -> Self {
        Self {
            status: StepStatus::Failed {
                message: message.into(),
            },
            work,
            note: String::new(),
        }
    }

    /// Attaches a note.
    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = note.into();
        self
    }
}

/// Errors an operator raises instead of returning an outcome.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OperatorError {
    /// Applied to an inquiry it declared itself inapplicable to.
    #[error("operator not applicable to this inquiry")]
    NotApplicable,
    /// The operation would exceed the budget; nothing was done.
    #[error("budget exceeded in dimension {0:?}")]
    BudgetExceeded(BudgetDimension),
    /// The oracle refused.
    #[error("oracle error: {0}")]
    Oracle(#[from] crate::world::OracleError),
    /// An external service has not answered yet; the episode suspends and
    /// the operator is retried on resume. The operator must have changed
    /// nothing before returning this.
    #[error("waiting for {service} request {request_id}")]
    Pending {
        /// The service.
        service: String,
        /// The pending request.
        request_id: String,
    },
    /// An external service failed.
    #[error("service error: {0}")]
    Service(String),
    /// Any other failure.
    #[error("operator error: {0}")]
    Internal(String),
}

impl From<crate::world::ServiceError> for OperatorError {
    fn from(error: crate::world::ServiceError) -> Self {
        match error {
            crate::world::ServiceError::Pending { request_id } => OperatorError::Pending {
                service: String::new(),
                request_id,
            },
            other => OperatorError::Service(other.to_string()),
        }
    }
}

/// A pluggable capability.
pub trait Operator<W>: Send + Sync {
    /// Static description.
    fn spec(&self) -> OperatorSpec;

    /// Whether the operator can run now. The default requires every view in
    /// `spec().reads` to exist.
    fn applicable(&self, inquiry: &Inquiry, _world: &W) -> bool {
        self.spec().reads.iter().all(|v| inquiry.has_view(v))
    }

    /// Applies the operator.
    fn apply(
        &self,
        inquiry: &mut Inquiry,
        world: &mut W,
        rng: &mut Rng,
    ) -> Result<OperatorOutcome, OperatorError>;
}

impl<W> Operator<W> for Box<dyn Operator<W>> {
    fn spec(&self) -> OperatorSpec {
        (**self).spec()
    }

    fn applicable(&self, inquiry: &Inquiry, world: &W) -> bool {
        (**self).applicable(inquiry, world)
    }

    fn apply(
        &self,
        inquiry: &mut Inquiry,
        world: &mut W,
        rng: &mut Rng,
    ) -> Result<OperatorOutcome, OperatorError> {
        (**self).apply(inquiry, world, rng)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_validation() {
        let t = OperatorSpec::new(
            "t",
            OperatorKind::Transform {
                contract: TransformContract::new(["a"], "b"),
            },
            "demo",
        );
        assert_eq!(
            t.validate(),
            Err(SpecError::TransformWritesNothing(OperatorId::from("t")))
        );
        t.clone().writes("v").validate().unwrap();
        let bad = OperatorSpec::new(
            "t",
            OperatorKind::Transform {
                contract: TransformContract::new(Vec::<&str>::new(), "b"),
            },
            "demo",
        )
        .writes("v");
        assert!(matches!(bad.validate(), Err(SpecError::Contract { .. })));
        let c = OperatorSpec::new("c", OperatorKind::Commit, "demo").writes("v");
        assert_eq!(
            c.validate(),
            Err(SpecError::CommitWritesViews(OperatorId::from("c")))
        );
        assert_eq!(
            OperatorSpec::new(" ", OperatorKind::Observe, "demo").validate(),
            Err(SpecError::EmptyId)
        );
        let consult = OperatorSpec::new(
            "ask",
            OperatorKind::Consult {
                service: "llm".into(),
            },
            "demo",
        );
        assert_eq!(
            consult.validate(),
            Err(SpecError::ConsultIncomplete(OperatorId::from("ask")))
        );
        consult.writes("consultations.llm").validate().unwrap();
    }
}
