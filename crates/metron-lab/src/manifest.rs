//! Experiment manifests.
//!
//! A manifest fixes everything about a run before it starts: the fixture,
//! the protocol, the system's schedule and budget, and the seed. Its hash is
//! written into every episode journal.

use crate::FAMILY_HIDDEN_BOOLEAN;
use metron_core::cost::Budget;
use metron_core::hash::ContentHash;
use serde::{Deserialize, Serialize};

/// Manifest errors.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    /// Not valid JSON for a manifest.
    #[error("manifest is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The manifest is well-formed JSON but not a valid experiment.
    #[error("invalid manifest: {0}")]
    Invalid(String),
}

/// Laboratory side of a manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabConfig {
    /// Task family.
    pub family: String,
    /// Fixture path, relative to the repository root.
    pub fixture: String,
    /// Probe cap per episode.
    #[serde(default)]
    pub max_probes: Option<u64>,
}

/// One schedule entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleStep {
    /// Operator identifier.
    pub operator: String,
    /// Consecutive applications.
    #[serde(default = "one")]
    pub repeat: u32,
}

const fn one() -> u32 {
    1
}

const fn default_max_steps() -> u32 {
    1_000
}

/// System side of a manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemPlan {
    /// Fixed operator schedule.
    pub schedule: Vec<ScheduleStep>,
    /// Cost budget for the inquiry.
    #[serde(default)]
    pub budget: Budget,
    /// Maximum scheduler ticks.
    #[serde(default = "default_max_steps")]
    pub max_steps: u32,
    /// Stall limit; `0` disables.
    #[serde(default)]
    pub stall_limit: u32,
}

/// An experiment manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Experiment name.
    pub name: String,
    /// Seed for the episode's random stream.
    pub seed: u64,
    /// Laboratory configuration.
    pub lab: LabConfig,
    /// System configuration.
    pub system: SystemPlan,
}

impl Manifest {
    /// Parses and validates a manifest from JSON text.
    pub fn from_json(text: &str) -> Result<Self, ManifestError> {
        let manifest: Manifest = serde_json::from_str(text)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Checks the manifest describes something runnable.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.name.trim().is_empty() {
            return Err(ManifestError::Invalid("name is empty".into()));
        }
        if self.lab.family != FAMILY_HIDDEN_BOOLEAN {
            return Err(ManifestError::Invalid(format!(
                "unknown family `{}` (known: `{FAMILY_HIDDEN_BOOLEAN}`)",
                self.lab.family
            )));
        }
        if self.lab.fixture.trim().is_empty() {
            return Err(ManifestError::Invalid("fixture path is empty".into()));
        }
        if self.system.schedule.is_empty() {
            return Err(ManifestError::Invalid("schedule is empty".into()));
        }
        if self
            .system
            .schedule
            .iter()
            .any(|s| s.operator.trim().is_empty())
        {
            return Err(ManifestError::Invalid(
                "schedule names an empty operator".into(),
            ));
        }
        if self.system.max_steps == 0 {
            return Err(ManifestError::Invalid("max_steps must be positive".into()));
        }
        Ok(())
    }

    /// Canonical pretty JSON.
    #[must_use]
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("manifests are always serialisable")
    }

    /// Hash of the canonical JSON.
    #[must_use]
    pub fn hash(&self) -> ContentHash {
        ContentHash::of_json(self).expect("manifests are always serialisable")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = r#"{
        "name": "smoke",
        "seed": 1,
        "lab": {"family": "hidden-boolean-function", "fixture": "f.json", "max_probes": 8},
        "system": {"schedule": [{"operator": "a", "repeat": 2}, {"operator": "b"}], "budget": {"max_oracle_probes": 8}}
    }"#;

    #[test]
    fn parses_and_hashes() {
        let m = Manifest::from_json(TEXT).unwrap();
        assert_eq!(m.system.schedule[1].repeat, 1);
        assert_eq!(m.system.max_steps, 1_000);
        assert_eq!(m.system.budget.max_oracle_probes, Some(8));
        assert_eq!(m.hash(), Manifest::from_json(TEXT).unwrap().hash());
        let mut other = m.clone();
        other.seed = 2;
        assert_ne!(m.hash(), other.hash());
    }

    #[test]
    fn rejects_invalid() {
        let bad = TEXT.replace("hidden-boolean-function", "mystery");
        assert!(matches!(
            Manifest::from_json(&bad),
            Err(ManifestError::Invalid(_))
        ));
        let empty = TEXT.replace(
            r#"[{"operator": "a", "repeat": 2}, {"operator": "b"}]"#,
            "[]",
        );
        assert!(Manifest::from_json(&empty).is_err());
    }
}
