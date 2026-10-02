//! Experiment manifests.
//!
//! A manifest fixes everything about a run before it starts: the fixture
//! or task-set specification, the protocol, the system's strategies and
//! budget, the cost model, and the seed. Its hash is written into every
//! episode journal.

use crate::FAMILY_HIDDEN_BOOLEAN;
use crate::headroom::CostModel;
use crate::tasks::TaskSetSpec;
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

/// Laboratory side of a manifest: one fixture, or a generated task set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LabConfig {
    /// One hidden target from a fixture file.
    Fixture {
        /// Task family.
        family: String,
        /// Fixture path, relative to the repository root.
        fixture: String,
        /// Probe cap per episode.
        #[serde(default)]
        max_probes: Option<u64>,
    },
    /// A task set generated from families.
    Tasks {
        /// Task family.
        family: String,
        /// Task-set specification.
        tasks: TaskSetSpec,
        /// Probe cap per episode.
        #[serde(default)]
        max_probes: Option<u64>,
    },
}

impl LabConfig {
    /// The task family.
    #[must_use]
    pub fn family(&self) -> &str {
        match self {
            LabConfig::Fixture { family, .. } | LabConfig::Tasks { family, .. } => family,
        }
    }

    /// The probe cap.
    #[must_use]
    pub fn max_probes(&self) -> Option<u64> {
        match self {
            LabConfig::Fixture { max_probes, .. } | LabConfig::Tasks { max_probes, .. } => {
                *max_probes
            }
        }
    }

    /// The fixture path, for fixture configurations.
    #[must_use]
    pub fn fixture(&self) -> Option<&str> {
        match self {
            LabConfig::Fixture { fixture, .. } => Some(fixture),
            LabConfig::Tasks { .. } => None,
        }
    }

    /// The task-set specification, for task configurations.
    #[must_use]
    pub fn tasks(&self) -> Option<&TaskSetSpec> {
        match self {
            LabConfig::Tasks { tasks, .. } => Some(tasks),
            LabConfig::Fixture { .. } => None,
        }
    }
}

/// One schedule entry: an operator or a nested sequence, each repeated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScheduleStep {
    /// An operator.
    Operator {
        /// Operator identifier.
        operator: String,
        /// Consecutive applications.
        #[serde(default = "one")]
        repeat: u32,
    },
    /// A sequence.
    Sequence {
        /// The steps.
        sequence: Vec<ScheduleStep>,
        /// Consecutive repetitions.
        #[serde(default = "one")]
        repeat: u32,
    },
}

impl ScheduleStep {
    /// Every operator identifier mentioned, in order, with repetition.
    pub fn operators(&self, out: &mut Vec<String>) {
        match self {
            ScheduleStep::Operator { operator, repeat } => {
                for _ in 0..*repeat {
                    out.push(operator.clone());
                }
            }
            ScheduleStep::Sequence { sequence, repeat } => {
                for _ in 0..*repeat {
                    for s in sequence {
                        s.operators(out);
                    }
                }
            }
        }
    }
}

const fn one() -> u32 {
    1
}

const fn default_max_steps() -> u32 {
    1_000
}

/// A selector strategy: an adaptive scheduler rather than a fixed schedule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SelectorSpec {
    /// Probe a prefix, measure each family's survivors, commit to a family
    /// by `rule` (`most`, the Bayes rule under a uniform prior, or `fewest`,
    /// the negative control).
    SurvivorCount {
        /// `most` or `fewest`.
        #[serde(default = "default_rule")]
        rule: String,
        /// Greedy probes before measuring.
        #[serde(default)]
        prefix_probes: u32,
        /// Probe rounds after choosing.
        #[serde(default = "default_rounds")]
        rounds: u32,
    },
}

fn default_rule() -> String {
    "most".into()
}

const fn default_rounds() -> u32 {
    64
}

/// A named strategy: a fixed schedule or a selector.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Strategy {
    /// Name used in reports.
    pub name: String,
    /// The schedule, for fixed strategies.
    #[serde(default)]
    pub schedule: Vec<ScheduleStep>,
    /// The selector, for adaptive strategies.
    #[serde(default)]
    pub selector: Option<SelectorSpec>,
}

impl Strategy {
    /// Whether this is a fixed schedule (a candidate for the single best
    /// solver) rather than a selector.
    #[must_use]
    pub fn is_fixed(&self) -> bool {
        self.selector.is_none()
    }
}

/// System side of a manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemPlan {
    /// A single fixed schedule (used when `strategies` is empty).
    #[serde(default)]
    pub schedule: Vec<ScheduleStep>,
    /// Named strategies to compare.
    #[serde(default)]
    pub strategies: Vec<Strategy>,
    /// Cost budget for each inquiry.
    #[serde(default)]
    pub budget: Budget,
    /// Maximum scheduler ticks.
    #[serde(default = "default_max_steps")]
    pub max_steps: u32,
    /// Stall limit; `0` disables.
    #[serde(default)]
    pub stall_limit: u32,
}

/// Headroom-measurement settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HeadroomSpec {
    /// Independent task-set seeds to draw (the manifest seed is the first).
    #[serde(default = "one")]
    pub seeds: u32,
    /// Cost model.
    pub cost_model: CostModel,
    /// Bootstrap resamples.
    #[serde(default = "default_resamples")]
    pub resamples: usize,
    /// Splits to include (`train`, `validation`, `test`); empty means all.
    #[serde(default)]
    pub splits: Vec<String>,
    /// Further cost models to report under, by name.
    #[serde(default)]
    pub extra_cost_models: std::collections::BTreeMap<String, CostModel>,
}

const fn default_resamples() -> usize {
    2_000
}

/// An experiment manifest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// Experiment name.
    pub name: String,
    /// Seed for the episode's random stream and the first task set.
    pub seed: u64,
    /// Laboratory configuration.
    pub lab: LabConfig,
    /// System configuration.
    pub system: SystemPlan,
    /// Headroom settings, when the manifest is a comparison.
    #[serde(default)]
    pub headroom: Option<HeadroomSpec>,
    /// Retrieval workload settings, when the manifest is a retrieval
    /// measurement.
    #[serde(default)]
    pub retrieval: Option<crate::retrieval::RetrievalSpec>,
}

impl Manifest {
    /// Parses and validates a manifest from JSON text.
    pub fn from_json(text: &str) -> Result<Self, ManifestError> {
        let manifest: Manifest = serde_json::from_str(text)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// The strategies to run: `strategies` if given, else the single
    /// schedule as a strategy named `default`.
    #[must_use]
    pub fn strategies(&self) -> Vec<Strategy> {
        if self.system.strategies.is_empty() {
            vec![Strategy {
                name: "default".into(),
                schedule: self.system.schedule.clone(),
                selector: None,
            }]
        } else {
            self.system.strategies.clone()
        }
    }

    /// Names of the fixed strategies, the candidates for the single best
    /// solver.
    #[must_use]
    pub fn fixed_strategy_names(&self) -> Vec<String> {
        self.strategies()
            .into_iter()
            .filter(Strategy::is_fixed)
            .map(|s| s.name)
            .collect()
    }

    /// Checks the manifest describes something runnable.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.name.trim().is_empty() {
            return Err(ManifestError::Invalid("name is empty".into()));
        }
        if self.lab.family() != FAMILY_HIDDEN_BOOLEAN {
            return Err(ManifestError::Invalid(format!(
                "unknown family `{}` (known: `{FAMILY_HIDDEN_BOOLEAN}`)",
                self.lab.family()
            )));
        }
        if self.lab.fixture().is_some_and(|f| f.trim().is_empty()) {
            return Err(ManifestError::Invalid("fixture path is empty".into()));
        }
        if let Some(tasks) = self.lab.tasks() {
            if tasks.arity == 0 || tasks.arity > crate::truth_table::MAX_ARITY {
                return Err(ManifestError::Invalid(format!(
                    "arity {} is out of range",
                    tasks.arity
                )));
            }
            if tasks.pool.families.is_empty() {
                return Err(ManifestError::Invalid("task set names no families".into()));
            }
            if tasks.targets_per_family == 0 || tasks.pool.per_family == 0 {
                return Err(ManifestError::Invalid(
                    "task set needs targets and pool members".into(),
                ));
            }
        }
        let strategies = self.strategies();
        for s in &strategies {
            match (&s.selector, s.schedule.is_empty()) {
                (None, true) => {
                    return Err(ManifestError::Invalid(format!(
                        "strategy `{}` has an empty schedule",
                        s.name
                    )));
                }
                (Some(_), false) => {
                    return Err(ManifestError::Invalid(format!(
                        "strategy `{}` has both a schedule and a selector",
                        s.name
                    )));
                }
                _ => {}
            }
        }
        for s in &strategies {
            if let Some(SelectorSpec::SurvivorCount { rule, .. }) = &s.selector
                && !["most", "fewest"].contains(&rule.as_str())
            {
                return Err(ManifestError::Invalid(format!(
                    "strategy `{}`: unknown survivor rule `{rule}`",
                    s.name
                )));
            }
        }
        if self.headroom.is_some() && !strategies.iter().any(Strategy::is_fixed) {
            return Err(ManifestError::Invalid(
                "headroom needs at least one fixed strategy".into(),
            ));
        }
        for s in &strategies {
            if s.name.trim().is_empty() {
                return Err(ManifestError::Invalid("a strategy has no name".into()));
            }
            let mut ops = Vec::new();
            for step in &s.schedule {
                step.operators(&mut ops);
            }
            if ops.iter().any(|o| o.trim().is_empty()) {
                return Err(ManifestError::Invalid(format!(
                    "strategy `{}` names an empty operator",
                    s.name
                )));
            }
        }
        let mut names: Vec<&str> = strategies.iter().map(|s| s.name.as_str()).collect();
        names.sort_unstable();
        if names.windows(2).any(|w| w[0] == w[1]) {
            return Err(ManifestError::Invalid("duplicate strategy names".into()));
        }
        if self.system.max_steps == 0 {
            return Err(ManifestError::Invalid("max_steps must be positive".into()));
        }
        if let Some(r) = &self.retrieval {
            let rows = crate::truth_table::TruthTable::rows_for(
                r.arity.min(crate::truth_table::MAX_ARITY),
            );
            if r.arity == 0 || r.arity > crate::truth_table::MAX_ARITY {
                return Err(ManifestError::Invalid(format!(
                    "retrieval arity {} is out of range",
                    r.arity
                )));
            }
            if r.families.is_empty() || r.store_per_family == 0 || r.queries == 0 {
                return Err(ManifestError::Invalid(
                    "retrieval needs families, store members and queries".into(),
                ));
            }
            if r.observed_rows.iter().any(|&k| k == 0 || k > rows) {
                return Err(ManifestError::Invalid(
                    "retrieval observed_rows must be within 1..=rows".into(),
                ));
            }
            if r.dimensions.iter().any(|&d| d < 64) {
                return Err(ManifestError::Invalid(
                    "retrieval dimensions must be at least 64".into(),
                ));
            }
        }
        if let Some(h) = &self.headroom {
            if h.seeds == 0 {
                return Err(ManifestError::Invalid(
                    "headroom.seeds must be positive".into(),
                ));
            }
            if self.lab.tasks().is_none() {
                return Err(ManifestError::Invalid(
                    "headroom needs a task set, not a fixture".into(),
                ));
            }
            for s in &h.splits {
                if !["train", "validation", "test"].contains(&s.as_str()) {
                    return Err(ManifestError::Invalid(format!("unknown split `{s}`")));
                }
            }
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

    const FIXTURE: &str = r#"{
        "name": "smoke",
        "seed": 1,
        "lab": {"family": "hidden-boolean-function", "fixture": "f.json", "max_probes": 8},
        "system": {"schedule": [{"operator": "a", "repeat": 2}, {"sequence": [{"operator": "b"}, {"operator": "c"}], "repeat": 2}], "budget": {"max_oracle_probes": 8}}
    }"#;

    const TASKS: &str = r#"{
        "name": "headroom",
        "seed": 7,
        "lab": {"family": "hidden-boolean-function", "tasks": {"arity": 5, "pool": {"families": ["affine", "monotone"], "per_family": 20}, "targets_per_family": 4}, "max_probes": 32},
        "system": {"strategies": [{"name": "s1", "schedule": [{"operator": "a"}]}, {"name": "s2", "schedule": [{"operator": "b"}]}]},
        "headroom": {"seeds": 3, "cost_model": {"failure_cost": 64.0}}
    }"#;

    #[test]
    fn fixture_manifests_parse_and_hash() {
        let m = Manifest::from_json(FIXTURE).unwrap();
        assert_eq!(m.lab.fixture(), Some("f.json"));
        assert_eq!(m.lab.max_probes(), Some(8));
        assert_eq!(m.strategies().len(), 1);
        let mut ops = Vec::new();
        for s in &m.system.schedule {
            s.operators(&mut ops);
        }
        assert_eq!(ops, vec!["a", "a", "b", "c", "b", "c"]);
        assert_eq!(m.hash(), Manifest::from_json(FIXTURE).unwrap().hash());
    }

    #[test]
    fn task_manifests_parse() {
        let m = Manifest::from_json(TASKS).unwrap();
        let tasks = m.lab.tasks().unwrap();
        assert_eq!(tasks.arity, 5);
        assert_eq!(tasks.pool.families.len(), 2);
        assert_eq!(m.strategies().len(), 2);
        let h = m.headroom.as_ref().unwrap();
        assert_eq!(h.seeds, 3);
        assert_eq!(h.resamples, 2_000);
        assert_eq!(h.cost_model.probe_weight, 1.0);
    }

    #[test]
    fn selector_strategies_parse() {
        let text = TASKS.replace(
            r#"{"name": "s2", "schedule": [{"operator": "b"}]}"#,
            r#"{"name": "s2", "selector": {"kind": "survivor-count", "prefix_probes": 2}}"#,
        );
        let m = Manifest::from_json(&text).unwrap();
        assert_eq!(m.fixed_strategy_names(), vec!["s1".to_owned()]);
        assert!(matches!(
            &m.strategies()[1].selector,
            Some(SelectorSpec::SurvivorCount {
                rule,
                prefix_probes: 2,
                rounds: 64
            }) if rule == "most"
        ));
        let both = TASKS.replace(
            r#"{"name": "s2", "schedule": [{"operator": "b"}]}"#,
            r#"{"name": "s2", "schedule": [{"operator": "b"}], "selector": {"kind": "survivor-count"}}"#,
        );
        assert!(Manifest::from_json(&both).is_err());
    }

    #[test]
    fn rejects_invalid() {
        assert!(
            Manifest::from_json(&FIXTURE.replace("hidden-boolean-function", "mystery")).is_err()
        );
        assert!(
            Manifest::from_json(&TASKS.replace("\"name\": \"s2\"", "\"name\": \"s1\"")).is_err()
        );
        assert!(Manifest::from_json(&TASKS.replace("\"seeds\": 3", "\"seeds\": 0")).is_err());
        assert!(
            Manifest::from_json(&FIXTURE.replace("\"fixture\": \"f.json\"", "\"fixture\": \" \""))
                .is_err()
        );
    }
}
