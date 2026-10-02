//! Capability promotion: propose from the train split, judge on the test
//! split.

use crate::compose::{load_manifest, registry, schedule_from, scheduler_for};
use crate::world::ComposedWorld;
use metron_adapters::{SystemClock, write_json_pretty};
use metron_app::{
    EpisodeRunner, FixedSchedule, RunConfig, RunOutcome, compress_trace, executed_trace,
    program_schedule, propose,
};
use metron_core::capability::CapabilityCandidate;
use metron_core::id::{EpisodeId, InquiryId};
use metron_core::inquiry::Inquiry;
use metron_core::journal::Episode;
use metron_lab::manifest::Strategy;
use metron_lab::promotion::{
    HeldOutSummary, PromotionCase, PromotionGate, PromotionReport, render_promotion_markdown,
};
use metron_lab::{LabWorld, Manifest, Protocol, Split, Task, TaskSet, Verdict};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Options for `promote`.
#[derive(Clone, Debug)]
pub struct PromoteOptions {
    /// Root under which the report directory is written.
    pub results_root: PathBuf,
    /// Only this strategy (default: every fixed strategy).
    pub strategy: Option<String>,
    /// Repetitions given to generalised loops (default: the probe cap, or 64).
    pub rounds: Option<u32>,
}

impl Default for PromoteOptions {
    fn default() -> Self {
        Self {
            results_root: PathBuf::from("experiments/results"),
            strategy: None,
            rounds: None,
        }
    }
}

/// What `promote` produced.
#[derive(Clone, Debug)]
pub struct PromoteOutcome {
    /// Where the files went.
    pub dir: PathBuf,
    /// One report per strategy.
    pub reports: Vec<PromotionReport>,
    /// Markdown rendering.
    pub markdown: String,
}

struct Harness<'a> {
    manifest: &'a Manifest,
    set: &'a TaskSet,
    runner: EpisodeRunner<ComposedWorld>,
}

impl Harness<'_> {
    fn run(
        &self,
        task: &Task,
        scheduler: &mut dyn metron_app::Scheduler,
    ) -> Result<(Inquiry, Episode, Verdict), String> {
        let mut world = ComposedWorld::new(LabWorld::for_task(
            self.set,
            task,
            Protocol {
                max_probes: self.manifest.lab.max_probes(),
            },
        ));
        let mut inquiry = Inquiry::new(InquiryId(1), world.lab().question())
            .with_budget(self.manifest.system.budget);
        let outcome = self
            .runner
            .run(
                EpisodeId(1),
                self.set.seed,
                self.manifest.hash(),
                &mut inquiry,
                &mut world,
                scheduler,
            )
            .map_err(|e| e.to_string())?;
        let episode = match outcome {
            RunOutcome::Completed(e) => e,
            RunOutcome::Suspended(_) => {
                return Err("promotion runs have no service; a strategy consulted one".into());
            }
        };
        let verdict = world.lab().judge(&inquiry);
        Ok((inquiry, episode, verdict))
    }
}

/// Runs promotion for the manifest's strategies.
pub fn run_promote(
    manifest_path: &Path,
    options: &PromoteOptions,
) -> Result<PromoteOutcome, String> {
    let manifest = load_manifest(manifest_path)?;
    let spec = manifest
        .lab
        .tasks()
        .ok_or("promotion needs a task-set manifest")?;
    let rounds = options
        .rounds
        .or_else(|| {
            manifest
                .lab
                .max_probes()
                .map(|c| u32::try_from(c).unwrap_or(64))
        })
        .unwrap_or(64);
    let set = TaskSet::generate(spec, manifest.seed);
    set.verify_split_hygiene()?;
    let train = set.in_split(Split::Train);
    let test = set.in_split(Split::Test);
    if train.is_empty() || test.is_empty() {
        return Err("promotion needs non-empty train and test splits".into());
    }
    let harness = Harness {
        manifest: &manifest,
        set: &set,
        runner: EpisodeRunner::new(registry()?, SystemClock).with_config(RunConfig {
            max_steps: manifest.system.max_steps,
            stall_limit: manifest.system.stall_limit,
        }),
    };
    let gate = PromotionGate::v0();
    let strategies: Vec<Strategy> = manifest
        .strategies()
        .into_iter()
        .filter(Strategy::is_fixed)
        .filter(|s| options.strategy.as_ref().is_none_or(|n| n == &s.name))
        .collect();
    if strategies.is_empty() {
        return Err("no fixed strategy to promote from".into());
    }
    let mut reports = Vec::new();
    for strategy in &strategies {
        // Propose from the train split.
        let mut candidates: BTreeMap<String, (CapabilityCandidate, usize)> = BTreeMap::new();
        for task in &train {
            let mut scheduler = scheduler_for(strategy);
            let (inquiry, episode, verdict) = harness.run(task, scheduler.as_mut())?;
            if !verdict.correct {
                continue;
            }
            if let Some(candidate) = propose(&inquiry, &episode, rounds) {
                let entry = candidates.entry(candidate.key()).or_insert((candidate, 0));
                entry.1 += 1;
            }
        }
        // Reference runs and reuse on the test split.
        let mut reference_verdicts = Vec::new();
        let mut reference_keys = Vec::new();
        for task in &test {
            let mut scheduler = scheduler_for(strategy);
            let (_, episode, verdict) = harness.run(task, scheduler.as_mut())?;
            let program = compress_trace(&executed_trace(&episode), rounds);
            reference_keys.push(
                serde_json::to_value(&program)
                    .map_err(|e| e.to_string())?
                    .to_string(),
            );
            reference_verdicts.push(verdict);
        }
        let reference = HeldOutSummary::of(&reference_verdicts);
        // Judge each candidate on the test split.
        let mut cases = Vec::new();
        for (key, (candidate, support)) in candidates {
            let Some(items) = program_schedule(&candidate) else {
                continue;
            };
            let mut held_out = Vec::new();
            for task in &test {
                let mut schedule: FixedSchedule = FixedSchedule::new(items.clone());
                let (_, _, verdict) = harness.run(task, &mut schedule)?;
                held_out.push(verdict);
            }
            let decision = gate.judge(&candidate, &held_out);
            let reuse = reference_keys.iter().filter(|k| **k == key).count() as f64
                / reference_keys.len() as f64;
            cases.push(PromotionCase {
                candidate,
                train_support: support,
                train_episodes: train.len(),
                reuse_frequency: reuse,
                held_out: HeldOutSummary::of(&held_out),
                reference: reference.clone(),
                decision,
            });
        }
        cases.sort_by_key(|c| std::cmp::Reverse(c.train_support));
        reports.push(PromotionReport {
            strategy: strategy.name.clone(),
            manifest_hash: manifest.hash(),
            gate_fingerprint: gate.fingerprint(),
            cases,
        });
    }
    let title = format!(
        "Promotion: {} (arity {}, {} train / {} test tasks)",
        manifest.name,
        spec.arity,
        train.len(),
        test.len()
    );
    let markdown = render_promotion_markdown(&reports, &title);
    let dir = options.results_root.join(format!(
        "promotion-{}-{}",
        manifest.name,
        manifest.hash().short()
    ));
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    write_json_pretty(dir.join("manifest.json"), &manifest).map_err(|e| e.to_string())?;
    write_json_pretty(dir.join("promotion.json"), &reports).map_err(|e| e.to_string())?;
    fs::write(dir.join("promotion.md"), &markdown).map_err(|e| e.to_string())?;
    let _ = schedule_from;
    Ok(PromoteOutcome {
        dir,
        reports,
        markdown,
    })
}
