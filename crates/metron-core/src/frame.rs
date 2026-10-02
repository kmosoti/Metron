//! Frames and transform contracts.
//!
//! A [`Frame`] is a representation system: a truth table, a DNF formula, a
//! list of observations, a set of hypotheses. Views live in frames. Moving
//! information from one frame to another is only allowed through a
//! [`TransformContract`] that says which frames it connects, what it
//! preserves, what it loses, what it assumes, and whether it is exact or
//! approximate. The runner refuses to register a transform operator without a
//! valid contract and refuses views written outside the contract's target
//! frame.

use crate::id::FrameId;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The frame in which raw observations are kept. Observe operators may write
/// only here; every other frame is reached through a contract.
pub const OBSERVATIONS: &str = "observations";

/// The observations frame identifier.
#[must_use]
pub fn observations_frame() -> FrameId {
    FrameId::from(OBSERVATIONS)
}

/// The frame in which raw answers from external services are kept. Consult
/// operators may write only here; nothing in it is evidence until a
/// transform has checked it against observations.
pub const CONSULTATIONS: &str = "consultations";

/// The consultations frame identifier.
#[must_use]
pub fn consultations_frame() -> FrameId {
    FrameId::from(CONSULTATIONS)
}

/// A representation system.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Frame {
    /// Identifier, e.g. `truth-table/complete`.
    pub id: FrameId,
    /// What the frame can express.
    pub description: String,
}

impl Frame {
    /// Creates a frame.
    pub fn new(id: impl Into<FrameId>, description: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
        }
    }
}

/// Whether a transform is information-preserving on its declared inputs.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Fidelity {
    /// The output is determined by the inputs and determines them back
    /// within what `preserves` lists.
    Exact,
    /// The output involves a guess, a bias, or a lossy step.
    Approximate {
        /// What is approximate and how.
        description: String,
    },
}

/// Errors from validating a contract.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContractError {
    /// A contract must name at least one source frame.
    #[error("transform contract has no source frame")]
    NoSourceFrame,
    /// A source frame was listed twice.
    #[error("source frame {0} listed twice")]
    DuplicateSourceFrame(FrameId),
    /// An approximate contract must say what is approximate.
    #[error("approximate contract has an empty description")]
    ApproximateWithoutDescription,
    /// A list entry was empty.
    #[error("empty entry in `{0}`")]
    EmptyEntry(&'static str),
}

/// The explicit contract behind a representation change.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TransformContract {
    /// Frames the transform reads.
    pub from: Vec<FrameId>,
    /// Frame the transform writes.
    pub to: FrameId,
    /// Properties of the inputs that survive in the output.
    #[serde(default)]
    pub preserves: Vec<String>,
    /// Properties of the inputs that are discarded.
    #[serde(default)]
    pub loses: Vec<String>,
    /// Assumptions the output depends on (inductive biases, defaults).
    #[serde(default)]
    pub assumes: Vec<String>,
    /// Exact or approximate.
    pub fidelity: Fidelity,
}

impl TransformContract {
    /// An exact contract from `from` to `to` with nothing else declared.
    pub fn new<I, F>(from: I, to: impl Into<FrameId>) -> Self
    where
        I: IntoIterator<Item = F>,
        F: Into<FrameId>,
    {
        Self {
            from: from.into_iter().map(Into::into).collect(),
            to: to.into(),
            preserves: Vec::new(),
            loses: Vec::new(),
            assumes: Vec::new(),
            fidelity: Fidelity::Exact,
        }
    }

    /// Declares a preserved property.
    #[must_use]
    pub fn preserves(mut self, property: impl Into<String>) -> Self {
        self.preserves.push(property.into());
        self
    }

    /// Declares a lost property.
    #[must_use]
    pub fn loses(mut self, property: impl Into<String>) -> Self {
        self.loses.push(property.into());
        self
    }

    /// Declares an assumption.
    #[must_use]
    pub fn assumes(mut self, assumption: impl Into<String>) -> Self {
        self.assumes.push(assumption.into());
        self
    }

    /// Marks the contract approximate.
    #[must_use]
    pub fn approximate(mut self, description: impl Into<String>) -> Self {
        self.fidelity = Fidelity::Approximate {
            description: description.into(),
        };
        self
    }

    /// Whether the contract is exact.
    #[must_use]
    pub fn is_exact(&self) -> bool {
        matches!(self.fidelity, Fidelity::Exact)
    }

    /// Checks the contract is well formed.
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.from.is_empty() {
            return Err(ContractError::NoSourceFrame);
        }
        for (i, f) in self.from.iter().enumerate() {
            if self.from[..i].contains(f) {
                return Err(ContractError::DuplicateSourceFrame(f.clone()));
            }
        }
        if let Fidelity::Approximate { description } = &self.fidelity
            && description.trim().is_empty()
        {
            return Err(ContractError::ApproximateWithoutDescription);
        }
        for (name, list) in [
            ("preserves", &self.preserves),
            ("loses", &self.loses),
            ("assumes", &self.assumes),
        ] {
            if list.iter().any(|s| s.trim().is_empty()) {
                return Err(ContractError::EmptyEntry(name));
            }
        }
        Ok(())
    }
}

impl fmt::Display for TransformContract {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let from: Vec<&str> = self.from.iter().map(FrameId::as_str).collect();
        let fidelity = match &self.fidelity {
            Fidelity::Exact => "exact".to_owned(),
            Fidelity::Approximate { description } => format!("approximate: {description}"),
        };
        write!(f, "[{}] -> {} ({fidelity})", from.join(", "), self.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_catches_malformed_contracts() {
        let ok = TransformContract::new(["a"], "b").preserves("x").loses("y");
        ok.validate().unwrap();
        let no_source = TransformContract::new(Vec::<&str>::new(), "b");
        assert_eq!(no_source.validate(), Err(ContractError::NoSourceFrame));
        let dup = TransformContract::new(["a", "a"], "b");
        assert!(matches!(
            dup.validate(),
            Err(ContractError::DuplicateSourceFrame(_))
        ));
        let vague = TransformContract::new(["a"], "b").approximate("  ");
        assert_eq!(
            vague.validate(),
            Err(ContractError::ApproximateWithoutDescription)
        );
        let empty = TransformContract::new(["a"], "b").assumes("");
        assert_eq!(empty.validate(), Err(ContractError::EmptyEntry("assumes")));
    }

    #[test]
    fn display_and_serde() {
        let c = TransformContract::new(["observations"], "truth-table/partial")
            .approximate("unobserved rows unknown");
        assert_eq!(
            c.to_string(),
            "[observations] -> truth-table/partial (approximate: unobserved rows unknown)"
        );
        let json = serde_json::to_string(&c).unwrap();
        let back: TransformContract = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }
}
