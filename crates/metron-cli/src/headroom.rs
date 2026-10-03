//! Headroom measurement: every strategy on every task of every task-set
//! seed, scored under the manifest's cost model.

use crate::compose::{load_manifest, registry, scheduler_for};
use crate::world::ComposedWorld;
use metron_adapters::{SystemClock, write_json_pretty};
use metron_app::{EpisodeRunner, RunConfig, RunOutcome, Scheduler};
use metron_core::cost::Budget;
use metron_core::hash::ContentHash;
use metron_core::id::{EpisodeId, InquiryId};
use metron_core::inquiry::Inquiry;
use metron_lab::headroom::{CostTable, HeadroomReport, Score, TaskKey, analyze, render_markdown};
use metron_lab::{LabWorld, Protocol, Split, Task, TaskSet};
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

/// What every scored episode of a measurement shares.
pub(crate) struct EpisodeContext<'a> {
    pub runner: &'a EpisodeRunner<ComposedWorld>,
    pub manifest_hash: ContentHash,
    pub budget: Budget,
    pub protocol: Protocol,
}

/// Runs one episode of `scheduler` on `task`, judges it, and scores it.
/// Shared by the headroom and routing measurements.
pub(crate) fn run_scored(
    context: &EpisodeContext<'_>,
    set: &TaskSet,
    task: &Task,
    seed: u64,
    scheduler: &mut dyn Scheduler,
) -> Result<(Score, Inquiry), String> {
    let mut world = ComposedWorld::new(LabWorld::for_task(set, task, context.protocol));
    let mut inquiry =
        Inquiry::new(InquiryId(1), world.lab().question()).with_budget(context.budget);
    let outcome = context
        .runner
        .run(
            EpisodeId(1),
            seed,
            context.manifest_hash,
            &mut inquiry,
            &mut world,
            scheduler,
        )
        .map_err(|e| e.to_string())?;
    let episode = match outcome {
        RunOutcome::Completed(e) => e,
        RunOutcome::Suspended(_) => {
            return Err("it consults a service; measurement runs have no service".into());
        }
    };
    let verdict = world.lab().judge(&inquiry);
    let cost = episode.outcome.as_ref().map_or(inquiry.spent, |o| o.cost);
    Ok((
        Score {
            answered: verdict.answered,
            correct: verdict.correct,
            probes: cost.oracle_probes,
            work: cost.work_units,
            calls: cost.operator_calls,
            external: cost.external_calls,
        },
        inquiry,
    ))
}

/// The mean entropy floor over the task-set seeds, with the failure cost
/// (in probes) at or above which the floor binds: a wrong answer must cost
/// more than finishing the identification would, which holds once it
/// exceeds the probe cap plus `log2` of the support's size.
pub(crate) fn entropy_floor(
    floors: &[(f64, f64)],
    cap: Option<u64>,
    arity: u8,
) -> Option<(f64, f64)> {
    if floors.is_empty() {
        return None;
    }
    let h = floors.iter().map(|(h, _)| h).sum::<f64>() / floors.len() as f64;
    let largest = floors.iter().map(|&(_, n)| n).fold(1.0, f64::max);
    let cap = cap.unwrap_or(1u64 << arity) as f64;
    Some((h, cap + largest.log2()))
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
    let fixed = manifest.fixed_strategy_names();
    let mut table = CostTable::new(names.clone());
    let runner = EpisodeRunner::new(registry()?, SystemClock).with_config(RunConfig {
        max_steps: manifest.system.max_steps,
        stall_limit: manifest.system.stall_limit,
    });
    let protocol = Protocol {
        max_probes: manifest.lab.max_probes(),
    };
    let context = EpisodeContext {
        runner: &runner,
        manifest_hash: manifest.hash(),
        budget: manifest.system.budget,
        protocol,
    };
    let mut episodes = 0usize;
    let mut floors: Vec<(f64, f64)> = Vec::new();
    for offset in 0..u64::from(seeds) {
        let seed = manifest.seed.wrapping_add(offset);
        let set = TaskSet::generate(&spec, seed);
        set.verify_split_hygiene()?;
        floors.push((set.entropy_floor(), set.support_size()));
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
                let mut scheduler = scheduler_for(strategy);
                let (score, _) = run_scored(&context, &set, task, seed, scheduler.as_mut())
                    .map_err(|e| format!("strategy `{}`: {e}", strategy.name))?;
                episodes += 1;
                scores.push(score);
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
    let floor = entropy_floor(&floors, protocol.max_probes, spec.arity);
    let attach = |report: HeadroomReport| -> HeadroomReport {
        match floor {
            Some((h, needed))
                if report.cost_model.failure_cost >= needed * report.cost_model.probe_weight =>
            {
                report.with_entropy_floor(h)
            }
            _ => report,
        }
    };
    let report = attach(analyze(
        &table,
        &headroom.cost_model,
        Some(&fixed),
        headroom.resamples,
        manifest.seed,
    ));
    let title = format!(
        "Headroom: {} ({} seeds, arity {})",
        manifest.name, seeds, spec.arity
    );
    let mut markdown = render_markdown(&report, &title);
    let dir = options.results_root.join(format!(
        "headroom-{}-{}",
        manifest.name,
        manifest.hash().short()
    ));
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    write_json_pretty(dir.join("manifest.json"), &manifest).map_err(|e| e.to_string())?;
    write_json_pretty(dir.join("cost-table.json"), &table).map_err(|e| e.to_string())?;
    write_json_pretty(dir.join("headroom.json"), &report).map_err(|e| e.to_string())?;
    for (name, model) in &headroom.extra_cost_models {
        let extra = attach(analyze(
            &table,
            model,
            Some(&fixed),
            headroom.resamples,
            manifest.seed,
        ));
        let extra_md = render_markdown(&extra, &format!("{title}, cost model `{name}`"));
        write_json_pretty(dir.join(format!("headroom-{name}.json")), &extra)
            .map_err(|e| e.to_string())?;
        fs::write(dir.join(format!("headroom-{name}.md")), &extra_md).map_err(|e| e.to_string())?;
        markdown.push_str("\n\n");
        markdown.push_str(&extra_md);
    }
    fs::write(dir.join("headroom.md"), &markdown).map_err(|e| e.to_string())?;
    Ok(HeadroomOutcome {
        dir,
        report,
        table,
        markdown,
        episodes,
    })
}
