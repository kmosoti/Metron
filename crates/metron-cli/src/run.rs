//! Single-episode commands: run, resume, replay, answer.

use crate::compose::{load_manifest, registry, resolve_fixture, scheduler_for};
use crate::world::{ComposedState, ComposedWorld};
use metron_adapters::claude_code::ResponseFile;
use metron_adapters::{
    ClaudeCodeBridge, ReplayService, ResultsWriter, RunRecord, ServiceBackend, SystemClock,
    load_json, read_text, write_json_pretty,
};
use metron_app::{EpisodeRunner, RunConfig, RunOutcome};
use metron_core::checkpoint::EpisodeCheckpoint;
use metron_core::id::{EpisodeId, InquiryId};
use metron_core::inquiry::Inquiry;
use metron_core::journal::Episode;
use metron_lab::{BooleanFixture, LabWorld, Manifest, Protocol, Split, TaskSet, Verdict};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Which service backend to compose.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Backend {
    /// The Claude Code file bridge, rooted at the run directory.
    #[default]
    ClaudeCode,
    /// No service: consultations fail.
    None,
}

/// Options for `run`.
#[derive(Clone, Debug)]
pub struct RunOptions {
    /// Root under which run directories are written.
    pub results_root: PathBuf,
    /// Overrides the manifest seed.
    pub seed: Option<u64>,
    /// For task-set manifests: the task identifier or index to run
    /// (default: the first test-split task).
    pub task: Option<String>,
    /// For manifests with several strategies: which one (default: first).
    pub strategy: Option<String>,
    /// Service backend.
    pub backend: Backend,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            results_root: PathBuf::from("experiments/results"),
            seed: None,
            task: None,
            strategy: None,
            backend: Backend::ClaudeCode,
        }
    }
}

/// How a target was chosen, recorded so the run can be resumed or replayed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TargetSelection {
    /// A fixture file.
    Fixture {
        /// Resolved path.
        path: String,
    },
    /// A task from a generated set.
    Task {
        /// Task-set seed.
        set_seed: u64,
        /// Index into the set's task list.
        index: usize,
        /// Task identifier.
        id: String,
    },
}

/// Metadata of a run directory.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunMeta {
    /// The manifest, verbatim.
    pub manifest: Manifest,
    /// Seed used.
    pub seed: u64,
    /// Target.
    pub target: TargetSelection,
    /// Strategy name.
    pub strategy: String,
    /// Backend in use.
    pub backend: String,
}

/// Outcome of `run` or `resume`.
#[derive(Clone, Debug)]
pub enum RunStatus {
    /// The episode completed and was judged.
    Completed {
        /// Run directory.
        dir: PathBuf,
        /// Everything written.
        record: Box<RunRecord>,
        /// The verdict.
        verdict: Verdict,
        /// Probes the oracle answered.
        probes_answered: u64,
    },
    /// The episode is waiting for an answer.
    Suspended {
        /// Run directory.
        dir: PathBuf,
        /// Pending request identifier.
        request_id: String,
        /// Where the request was written.
        request_path: PathBuf,
        /// The prompt, if the request carries one.
        prompt: Option<String>,
    },
}

impl RunStatus {
    /// The run directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        match self {
            RunStatus::Completed { dir, .. } | RunStatus::Suspended { dir, .. } => dir,
        }
    }
}

fn run_dir(root: &Path, manifest: &Manifest, seed: u64, target: &TargetSelection) -> PathBuf {
    let name: String = manifest
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let target_tag = match target {
        TargetSelection::Fixture { .. } => "fixture".to_owned(),
        TargetSelection::Task { id, .. } => id.clone(),
    };
    root.join(format!(
        "{name}-seed{seed}-{}-{target_tag}",
        manifest.hash().short()
    ))
}

fn select_target(
    manifest: &Manifest,
    manifest_path: &Path,
    seed: u64,
    task: Option<&str>,
) -> Result<TargetSelection, String> {
    if let Some(fixture) = manifest.lab.fixture() {
        let path = resolve_fixture(manifest_path, fixture)?;
        return Ok(TargetSelection::Fixture {
            path: path.display().to_string(),
        });
    }
    let spec = manifest
        .lab
        .tasks()
        .ok_or("manifest has neither a fixture nor tasks")?;
    let set = TaskSet::generate(spec, seed);
    let index = match task {
        Some(t) => {
            if let Ok(i) = t.parse::<usize>() {
                if i >= set.tasks.len() {
                    return Err(format!(
                        "task index {i} out of range ({} tasks)",
                        set.tasks.len()
                    ));
                }
                i
            } else {
                set.tasks
                    .iter()
                    .position(|x| x.id == t)
                    .ok_or_else(|| format!("no task named `{t}`"))?
            }
        }
        None => set
            .tasks
            .iter()
            .position(|x| x.split == Split::Test)
            .unwrap_or(0),
    };
    Ok(TargetSelection::Task {
        set_seed: seed,
        index,
        id: set.tasks[index].id.clone(),
    })
}

fn lab_world(manifest: &Manifest, target: &TargetSelection) -> Result<LabWorld, String> {
    let protocol = Protocol {
        max_probes: manifest.lab.max_probes(),
    };
    match target {
        TargetSelection::Fixture { path } => {
            let fixture = BooleanFixture::from_json(&read_text(path).map_err(|e| e.to_string())?)
                .map_err(|e| format!("{path}: {e}"))?;
            let hidden = fixture.seal().map_err(|e| format!("{path}: {e}"))?;
            Ok(LabWorld::new(hidden, protocol))
        }
        TargetSelection::Task {
            set_seed, index, ..
        } => {
            let spec = manifest.lab.tasks().ok_or("manifest has no task set")?;
            let set = TaskSet::generate(spec, *set_seed);
            let task = set.tasks.get(*index).ok_or("task index out of range")?;
            Ok(LabWorld::for_task(&set, task, protocol))
        }
    }
}

fn backend_for(backend: &Backend, dir: &Path) -> Option<Box<dyn ServiceBackend>> {
    match backend {
        Backend::ClaudeCode => Some(Box::new(ClaudeCodeBridge::new(dir))),
        Backend::None => None,
    }
}

fn runner(manifest: &Manifest) -> Result<EpisodeRunner<ComposedWorld>, String> {
    Ok(
        EpisodeRunner::new(registry()?, SystemClock).with_config(RunConfig {
            max_steps: manifest.system.max_steps,
            stall_limit: manifest.system.stall_limit,
        }),
    )
}

fn finish(
    dir: &Path,
    meta: &RunMeta,
    outcome: RunOutcome,
    inquiry: &Inquiry,
    world: &mut ComposedWorld,
    scheduler_state: serde_json::Value,
) -> Result<RunStatus, String> {
    match outcome {
        RunOutcome::Completed(episode) => {
            episode.verify().map_err(|e| e.to_string())?;
            let verdict = world.lab().judge(inquiry);
            let record = RunRecord {
                name: meta.manifest.name.clone(),
                manifest: serde_json::to_value(&meta.manifest).map_err(|e| e.to_string())?,
                episode,
                verdict: serde_json::to_value(&verdict).map_err(|e| e.to_string())?,
            };
            ResultsWriter::into_dir(dir)
                .write(&record)
                .map_err(|e| e.to_string())?;
            let _ = fs::remove_file(dir.join("checkpoint.json"));
            Ok(RunStatus::Completed {
                dir: dir.to_path_buf(),
                record: Box::new(record),
                verdict,
                probes_answered: world.lab().probes_answered(),
            })
        }
        RunOutcome::Suspended(mut checkpoint) => {
            checkpoint.world_state =
                serde_json::to_value(world.state()).map_err(|e| e.to_string())?;
            checkpoint.scheduler_state = scheduler_state;
            write_json_pretty(dir.join("checkpoint.json"), &*checkpoint)
                .map_err(|e| e.to_string())?;
            let request_id = checkpoint.pending.request_id.clone();
            let bridge = ClaudeCodeBridge::new(dir);
            let request_path = bridge.request_path(&request_id);
            let prompt = load_json::<serde_json::Value>(&request_path)
                .ok()
                .and_then(|v| {
                    v.get("request")?
                        .get("value")?
                        .get("prompt")?
                        .as_str()
                        .map(str::to_owned)
                });
            Ok(RunStatus::Suspended {
                dir: dir.to_path_buf(),
                request_id,
                request_path,
                prompt,
            })
        }
    }
}

/// Runs one episode from a manifest.
pub fn run(manifest_path: &Path, options: &RunOptions) -> Result<RunStatus, String> {
    let mut manifest = load_manifest(manifest_path)?;
    if let Some(seed) = options.seed {
        manifest.seed = seed;
    }
    let seed = manifest.seed;
    let target = select_target(&manifest, manifest_path, seed, options.task.as_deref())?;
    let strategies = manifest.strategies();
    let strategy = match &options.strategy {
        Some(name) => strategies
            .iter()
            .find(|s| &s.name == name)
            .ok_or_else(|| format!("no strategy named `{name}`"))?
            .clone(),
        None => strategies[0].clone(),
    };
    let dir = run_dir(&options.results_root, &manifest, seed, &target);
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let meta = RunMeta {
        manifest: manifest.clone(),
        seed,
        target: target.clone(),
        strategy: strategy.name.clone(),
        backend: match options.backend {
            Backend::ClaudeCode => "claude-code".into(),
            Backend::None => "none".into(),
        },
    };
    write_json_pretty(dir.join("run.json"), &meta).map_err(|e| e.to_string())?;

    let mut world = ComposedWorld::new(lab_world(&manifest, &target)?);
    if let Some(b) = backend_for(&options.backend, &dir) {
        world = world.with_backend(b);
    }
    let mut inquiry =
        Inquiry::new(InquiryId(1), world.lab().question()).with_budget(manifest.system.budget);
    let mut scheduler = scheduler_for(&strategy);
    let runner = runner(&manifest)?;
    let outcome = runner
        .run(
            EpisodeId(1),
            seed,
            manifest.hash(),
            &mut inquiry,
            &mut world,
            scheduler.as_mut(),
        )
        .map_err(|e| e.to_string())?;
    let scheduler_state = scheduler.state();
    finish(&dir, &meta, outcome, &inquiry, &mut world, scheduler_state)
}

/// Resumes a suspended run directory.
pub fn resume(dir: &Path) -> Result<RunStatus, String> {
    let meta: RunMeta = load_json(dir.join("run.json")).map_err(|e| e.to_string())?;
    let checkpoint: EpisodeCheckpoint = load_json(dir.join("checkpoint.json"))
        .map_err(|e| format!("no checkpoint to resume: {e}"))?;
    let strategy = meta
        .manifest
        .strategies()
        .into_iter()
        .find(|s| s.name == meta.strategy)
        .ok_or("run names an unknown strategy")?;
    let mut world = ComposedWorld::new(lab_world(&meta.manifest, &meta.target)?);
    let backend = if meta.backend == "none" {
        Backend::None
    } else {
        Backend::ClaudeCode
    };
    if let Some(b) = backend_for(&backend, dir) {
        world = world.with_backend(b);
    }
    let state: ComposedState =
        serde_json::from_value(checkpoint.world_state.clone()).map_err(|e| e.to_string())?;
    world.restore(state);
    let mut scheduler = scheduler_for(&strategy);
    scheduler.restore(&checkpoint.scheduler_state)?;
    let mut inquiry = checkpoint.inquiry.clone();
    let runner = runner(&meta.manifest)?;
    let outcome = runner
        .resume(checkpoint, &mut inquiry, &mut world, scheduler.as_mut())
        .map_err(|e| e.to_string())?;
    let scheduler_state = scheduler.state();
    finish(dir, &meta, outcome, &inquiry, &mut world, scheduler_state)
}

/// Writes an answer for a pending request of a run directory. With no
/// `request_id`, answers the only pending request.
pub fn answer(
    dir: &Path,
    request_id: Option<&str>,
    text: &str,
    answered_by: &str,
) -> Result<PathBuf, String> {
    let bridge = ClaudeCodeBridge::new(dir);
    let id = match request_id {
        Some(id) => id.to_owned(),
        None => {
            let pending = bridge.pending_requests();
            match pending.as_slice() {
                [one] => one.clone(),
                [] => return Err("no pending request".into()),
                many => return Err(format!("several pending requests: {}", many.join(", "))),
            }
        }
    };
    if !bridge.request_path(&id).exists() {
        return Err(format!("no request `{id}` in {}", dir.display()));
    }
    let path = bridge.response_path(&id);
    write_json_pretty(
        &path,
        &ResponseFile {
            response: serde_json::Value::String(text.trim().to_owned()),
            answered_by: answered_by.to_owned(),
        },
    )
    .map_err(|e| e.to_string())?;
    Ok(path)
}

/// Result of a replay.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplayReport {
    /// Whether the replayed effective events equal the recorded ones.
    pub matches: bool,
    /// Recorded journal head.
    pub recorded_head: String,
    /// Replayed journal head.
    pub replayed_head: String,
    /// Recorded effective event count.
    pub recorded_events: usize,
    /// Replayed effective event count.
    pub replayed_events: usize,
    /// First differing event index, if any.
    pub first_difference: Option<usize>,
    /// Whether the replayed answer equals the recorded answer hash.
    pub answer_matches: bool,
}

/// Re-runs a completed run directory with recorded service answers and
/// compares the journals.
pub fn replay(dir: &Path) -> Result<ReplayReport, String> {
    let meta: RunMeta = load_json(dir.join("run.json")).map_err(|e| e.to_string())?;
    let recorded: Episode =
        load_json(dir.join("episode.json")).map_err(|e| format!("no completed episode: {e}"))?;
    let strategy = meta
        .manifest
        .strategies()
        .into_iter()
        .find(|s| s.name == meta.strategy)
        .ok_or("run names an unknown strategy")?;
    let world_base = lab_world(&meta.manifest, &meta.target)?;
    let mut world = ComposedWorld::new(world_base).with_backend(Box::new(
        ReplayService::from_episode(&recorded, metron_adapters::claude_code::SERVICE),
    ));
    let mut inquiry =
        Inquiry::new(InquiryId(1), world.lab().question()).with_budget(meta.manifest.system.budget);
    let mut scheduler = scheduler_for(&strategy);
    let runner = runner(&meta.manifest)?;
    let outcome = runner
        .run(
            recorded.id,
            recorded.seed,
            recorded.manifest_hash,
            &mut inquiry,
            &mut world,
            scheduler.as_mut(),
        )
        .map_err(|e| e.to_string())?;
    let replayed = match outcome {
        RunOutcome::Completed(e) => e,
        RunOutcome::Suspended(_) => {
            return Err("replay suspended: a recorded answer is missing".into());
        }
    };
    let a = recorded.effective_events();
    let b = replayed.effective_events();
    let first_difference = a
        .iter()
        .zip(&b)
        .position(|(x, y)| x != y)
        .or_else(|| (a.len() != b.len()).then_some(a.len().min(b.len())));
    let recorded_answer = recorded.entries.iter().find_map(|e| match &e.event {
        metron_core::journal::JournalEvent::EpisodeEnded { answer_hash, .. } => Some(*answer_hash),
        _ => None,
    });
    let replayed_answer = replayed.entries.iter().find_map(|e| match &e.event {
        metron_core::journal::JournalEvent::EpisodeEnded { answer_hash, .. } => Some(*answer_hash),
        _ => None,
    });
    Ok(ReplayReport {
        matches: first_difference.is_none(),
        recorded_head: recorded.head_hash().to_string(),
        replayed_head: replayed.head_hash().to_string(),
        recorded_events: a.len(),
        replayed_events: b.len(),
        first_difference,
        answer_matches: recorded_answer == replayed_answer,
    })
}
