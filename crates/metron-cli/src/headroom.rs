//! Headroom measurement: every strategy on every task of every task-set
//! seed, scored under the manifest's cost model.

use crate::compose::{load_manifest, registry, schedule_from};
use crate::world::ComposedWorld;
use metron_adapters::{SystemClock, write_json_pretty};
use metron_app::{EpisodeRunner, RunConfig, RunOutcome};
use metron_core::id::{EpisodeId, InquiryId};
use metron_core::inquiry::Inquiry;
use metron_lab::headroom::{CostTable, HeadroomReport, Score, TaskKey, analyze, render_markdown};
use metron_lab::{LabWorld, Protocol, Split, TaskSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Options for `headroom`.
#[derive(Clone, Debug)]
pub struct HeadroomOptions {
    /// Root under which the report directory is written.
    pub results_root: PathBuf,
    /// Overrides the manifest's number of seeds.
    pub seeds: Option<u32>,
    /// Prints progress to stderr.
    pub verbose: bool,
}

impl Default for HeadroomOptions {
    fn default() -> Self {
        Self {
            results_root: PathBuf::from("experiments/results"),
            seeds: None,
            verbose: false,
        }
    }
}

/// What `headroom` produced.
#[derive(Clone, Debug)]
pub struct HeadroomOutcome {
    /// Where the files went.
    pub dir: PathBuf,
    /// The analysis.
    pub report: HeadroomReport,
    /// The raw table.
    pub table: CostTable,
    /// Markdown rendering.
    pub markdown: String,
    /// Episodes run.
    pub episodes: usize,
}

fn split_of(label: &str) -> Option<Split> {
    match label {
        "train" => Some(Split::Train),
        "validation" => Some(Split::Validation),
        "test" => Some(Split::Test),
        _ => None,
    }
}

/// Runs the headroom measurement described by a manifest.
pub fn run_headroom(
    manifest_path: &Path,
    options: &HeadroomOptions,
) -> Result<HeadroomOutcome, String> {
    let manifest = load_manifest(manifest_path)?;
    let spec = manifest
        .lab
        .tasks()
        .ok_or("headroom needs a task-set manifest")?
        .clone();
    let headroom = manifest
        .headroom
        .clone()
        .ok_or("manifest has no `headroom` section")?;
    let seeds = options.seeds.unwrap_or(headroom.seeds).max(1);
    let splits: Vec<Split> = headroom.splits.iter().filter_map(|s| split_of(s)).collect();
    let strategies = manifest.strategies();
    let names: Vec<String> = strategies.iter().map(|s| s.name.clone()).collect();
    let mut table = CostTable::new(names.clone());
    let runner = EpisodeRunner::new(registry()?, SystemClock).with_config(RunConfig {
        max_steps: manifest.system.max_steps,
        stall_limit: manifest.system.stall_limit,
    });
    let protocol = Protocol {
        max_probes: manifest.lab.max_probes(),
    };
    let mut episodes = 0usize;
    for offset in 0..u64::from(seeds) {
        let seed = manifest.seed.wrapping_add(offset);
        let set = TaskSet::generate(&spec, seed);
        set.verify_split_hygiene()?;
        if options.verbose {
            eprintln!(
                "seed {seed}: pool {} members, {} tasks, {} NPN classes",
                set.pool.len(),
                set.tasks.len(),
                set.class_count()
            );
        }
        for task in &set.tasks {
            if !splits.is_empty() && !splits.contains(&task.split) {
                continue;
            }
            let mut scores = Vec::with_capacity(strategies.len());
            for strategy in &strategies {
                let mut world = ComposedWorld::new(LabWorld::for_task(&set, task, protocol));
                let mut inquiry = Inquiry::new(InquiryId(1), world.lab().question())
                    .with_budget(manifest.system.budget);
                let mut schedule = schedule_from(&strategy.schedule);
                let outcome = runner
                    .run(
                        EpisodeId(1),
                        seed,
                        manifest.hash(),
                        &mut inquiry,
                        &mut world,
                        &mut schedule,
                    )
                    .map_err(|e| e.to_string())?;
                let episode = match outcome {
                    RunOutcome::Completed(e) => e,
                    RunOutcome::Suspended(_) => {
                        return Err(format!(
                            "strategy `{}` consults a service; headroom runs have no service",
                            strategy.name
                        ));
                    }
                };
                episodes += 1;
                let verdict = world.lab().judge(&inquiry);
                let cost = episode.outcome.as_ref().map_or(inquiry.spent, |o| o.cost);
                scores.push(Score {
                    answered: verdict.answered,
                    correct: verdict.correct,
                    probes: cost.oracle_probes,
                    work: cost.work_units,
                    calls: cost.operator_calls,
                    external: cost.external_calls,
                });
            }
            table.push(
                TaskKey {
                    id: task.id.clone(),
                    family: task.target.family.label().to_owned(),
                    split: task.split.label().to_owned(),
                    seed,
                },
                scores,
            );
        }
    }
    let report = analyze(
        &table,
        &headroom.cost_model,
        Some(&names),
        headroom.resamples,
        manifest.seed,
    );
    let title = format!(
        "Headroom: {} ({} seeds, arity {})",
        manifest.name, seeds, spec.arity
    );
    let markdown = render_markdown(&report, &title);
    let dir = options.results_root.join(format!(
        "headroom-{}-{}",
        manifest.name,
        manifest.hash().short()
    ));
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    write_json_pretty(dir.join("manifest.json"), &manifest).map_err(|e| e.to_string())?;
    write_json_pretty(dir.join("cost-table.json"), &table).map_err(|e| e.to_string())?;
    write_json_pretty(dir.join("headroom.json"), &report).map_err(|e| e.to_string())?;
    fs::write(dir.join("headroom.md"), &markdown).map_err(|e| e.to_string())?;
    Ok(HeadroomOutcome {
        dir,
        report,
        table,
        markdown,
        episodes,
    })
}
