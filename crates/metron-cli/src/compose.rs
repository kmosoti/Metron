//! Building blocks shared by the commands.

use crate::world::ComposedWorld;
use metron_adapters::read_text;
use metron_app::{FixedSchedule, OperatorRegistry, ScheduleItem};
use metron_lab::manifest::ScheduleStep;
use metron_lab::{Family, Manifest};
use metron_operators::all_operators;
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
