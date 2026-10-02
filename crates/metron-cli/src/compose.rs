//! Building blocks shared by the commands.

use crate::world::ComposedWorld;
use metron_adapters::read_text;
use metron_app::{
    FixedSchedule, OperatorRegistry, ScheduleItem, Scheduler, SurvivorCountSelector, SurvivorRule,
    Vocabulary,
};
use metron_core::id::{OperatorId, ViewId};
use metron_lab::manifest::{ScheduleStep, SelectorSpec, Strategy};
use metron_lab::{Family, Manifest};
use metron_operators::{
    CommitTruthTable, GreedySplitProbe, SingleSurvivorToTable, VersionSpaceFilter, all_operators,
    views,
};
use std::path::{Path, PathBuf};

/// Labels of every laboratory family, for the restricted filters.
#[must_use]
pub fn family_labels() -> Vec<&'static str> {
    Family::ALL.iter().map(|f| f.label()).collect()
}

/// Loads and validates a manifest.
pub fn load_manifest(path: &Path) -> Result<Manifest, String> {
    let text = read_text(path).map_err(|e| e.to_string())?;
    Manifest::from_json(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Resolves a fixture path: as given, then relative to the manifest's
/// directory, its parent, and its grandparent (the repository root when
/// manifests live in `experiments/manifests/`).
pub fn resolve_fixture(manifest_path: &Path, fixture: &str) -> Result<PathBuf, String> {
    let direct = PathBuf::from(fixture);
    if direct.exists() {
        return Ok(direct);
    }
    let mut candidates = Vec::new();
    if let Some(dir) = manifest_path.parent() {
        candidates.push(dir.join(fixture));
        if let Some(up) = dir.parent() {
            candidates.push(up.join(fixture));
            if let Some(root) = up.parent() {
                candidates.push(root.join(fixture));
            }
        }
    }
    candidates
        .into_iter()
        .find(|p| p.exists())
        .ok_or_else(|| format!("fixture `{fixture}` not found"))
}

/// A registry with every operator.
pub fn registry() -> Result<OperatorRegistry<ComposedWorld>, String> {
    let mut registry = OperatorRegistry::new();
    registry
        .register_all(all_operators(&family_labels()))
        .map_err(|e| e.to_string())?;
    Ok(registry)
}

fn item(step: &ScheduleStep) -> ScheduleItem {
    match step {
        ScheduleStep::Operator { operator, repeat } => {
            ScheduleItem::times(operator.as_str(), *repeat)
        }
        ScheduleStep::Sequence { sequence, repeat } => {
            ScheduleItem::sequence(sequence.iter().map(item), *repeat)
        }
    }
}

/// Turns manifest schedule steps into a fixed schedule.
#[must_use]
pub fn schedule_from(steps: &[ScheduleStep]) -> FixedSchedule {
    FixedSchedule::new(steps.iter().map(item))
}

/// The operator and view names selectors schedule, taken from the
/// registered operators rather than spelled out.
#[must_use]
pub fn vocabulary() -> Vocabulary {
    Vocabulary {
        filter: OperatorId::from(VersionSpaceFilter::ID),
        restricted_filter_prefix: format!("{}:", VersionSpaceFilter::ID),
        probe: OperatorId::from(GreedySplitProbe::ID),
        survivor_to_table: OperatorId::from(SingleSurvivorToTable::ID),
        commit: OperatorId::from(CommitTruthTable::ID),
        version_space_view: ViewId::from(views::VERSION_SPACE),
    }
}

/// The scheduler for a strategy: a fixed schedule or a selector.
#[must_use]
pub fn scheduler_for(strategy: &Strategy) -> Box<dyn Scheduler> {
    match &strategy.selector {
        Some(SelectorSpec::SurvivorCount {
            rule,
            prefix_probes,
            rounds,
        }) => Box::new(SurvivorCountSelector::new(
            vocabulary(),
            family_labels().into_iter().map(str::to_owned).collect(),
            if rule == "fewest" {
                SurvivorRule::Fewest
            } else {
                SurvivorRule::Most
            },
            *prefix_probes,
            *rounds,
        )),
        None => Box::new(schedule_from(&strategy.schedule)),
    }
}
