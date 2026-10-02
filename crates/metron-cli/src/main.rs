//! `metron`: the composition root.
//!
//! This binary is the only place where the laboratory, the application layer,
//! the reference operators and the adapters meet. It reads a manifest, seals
//! the fixture into a laboratory world, runs one episode, asks the
//! laboratory for its verdict, and writes the run directory.

use metron_adapters::{ResultsWriter, RunRecord, SystemClock, read_text, results::read_journal};
use metron_app::{EpisodeRunner, FixedSchedule, OperatorRegistry, RunConfig, ScheduleItem};
use metron_core::id::{EpisodeId, InquiryId};
use metron_core::inquiry::Inquiry;
use metron_lab::{BooleanFixture, LabWorld, Manifest, Protocol};
use metron_operators::reference_operators;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
usage:
  metron run <manifest.json> [--results <dir>] [--seed <n>]
      Run one episode described by the manifest and write a run directory
      (default results root: experiments/results).
  metron verify <episode.jsonl>
      Verify a journal's hash chain and every receipt in it.
  metron explain <manifest.json>
      Print the schedule, the budget and every operator contract in force.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("run") => run(&args[1..]),
        Some("verify") => verify(&args[1..]),
        Some("explain") => explain(&args[1..]),
        Some("--help" | "-h" | "help") => {
            print!("{USAGE}");
            Ok(())
        }
        _ => {
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

struct RunArgs {
    manifest: PathBuf,
    results: PathBuf,
    seed: Option<u64>,
}

fn parse_run_args(args: &[String]) -> Result<RunArgs, String> {
    let mut manifest = None;
    let mut results = PathBuf::from("experiments/results");
    let mut seed = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--results" => {
                i += 1;
                results = PathBuf::from(args.get(i).ok_or("--results needs a directory")?);
            }
            "--seed" => {
                i += 1;
                seed = Some(
                    args.get(i)
                        .ok_or("--seed needs a number")?
                        .parse::<u64>()
                        .map_err(|e| format!("--seed: {e}"))?,
                );
            }
            other if other.starts_with("--") => return Err(format!("unknown flag {other}")),
            other => {
                if manifest.replace(PathBuf::from(other)).is_some() {
                    return Err("only one manifest may be given".into());
                }
            }
        }
        i += 1;
    }
    Ok(RunArgs {
        manifest: manifest.ok_or("a manifest path is required")?,
        results,
        seed,
    })
}

fn load_manifest(path: &Path) -> Result<Manifest, String> {
    let text = read_text(path).map_err(|e| e.to_string())?;
    Manifest::from_json(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Resolves a fixture path: as given, then relative to the manifest's
/// directory, then relative to the manifest's grandparent (the
/// `experiments/` root when manifests live in `experiments/manifests/`).
fn resolve_fixture(manifest_path: &Path, fixture: &str) -> Result<PathBuf, String> {
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

fn run(args: &[String]) -> Result<(), String> {
    let args = parse_run_args(args)?;
    let mut manifest = load_manifest(&args.manifest)?;
    if let Some(seed) = args.seed {
        manifest.seed = seed;
    }
    let fixture_path = resolve_fixture(&args.manifest, &manifest.lab.fixture)?;
    let fixture = BooleanFixture::from_json(&read_text(&fixture_path).map_err(|e| e.to_string())?)
        .map_err(|e| format!("{}: {e}", fixture_path.display()))?;
    let hidden = fixture
        .seal()
        .map_err(|e| format!("{}: {e}", fixture_path.display()))?;

    let mut world = LabWorld::new(
        hidden,
        Protocol {
            max_probes: manifest.lab.max_probes,
        },
    );
    let mut inquiry =
        Inquiry::new(InquiryId(1), world.question()).with_budget(manifest.system.budget);

    let mut registry = OperatorRegistry::new();
    registry
        .register_all(reference_operators())
        .map_err(|e| e.to_string())?;
    let mut schedule = FixedSchedule::new(
        manifest
            .system
            .schedule
            .iter()
            .map(|s| ScheduleItem::times(s.operator.as_str(), s.repeat)),
    );
    let runner = EpisodeRunner::new(registry, SystemClock).with_config(RunConfig {
        max_steps: manifest.system.max_steps,
        stall_limit: manifest.system.stall_limit,
    });

    let episode = runner
        .run(
            EpisodeId(1),
            manifest.seed,
            manifest.hash(),
            &mut inquiry,
            &mut world,
            &mut schedule,
        )
        .map_err(|e| e.to_string())?;
    episode.verify().map_err(|e| e.to_string())?;
    let verdict = world.judge(&inquiry);

    let record = RunRecord {
        name: manifest.name.clone(),
        manifest: serde_json::to_value(&manifest).map_err(|e| e.to_string())?,
        episode: episode.clone(),
        verdict: serde_json::to_value(&verdict).map_err(|e| e.to_string())?,
    };
    let dir = ResultsWriter::new(&args.results)
        .write(&record)
        .map_err(|e| e.to_string())?;

    let outcome = episode.outcome.as_ref().ok_or("episode has no outcome")?;
    println!("run        {} (seed {})", manifest.name, manifest.seed);
    println!("fixture    {} ({})", fixture.name, fixture_path.display());
    println!("manifest   {}", manifest.hash());
    println!("stop       {:?}", outcome.stop);
    println!(
        "steps      {}  cost: calls={} work={} probes={} external={}",
        outcome.steps,
        outcome.cost.operator_calls,
        outcome.cost.work_units,
        outcome.cost.oracle_probes,
        outcome.cost.external_calls
    );
    println!(
        "receipts   {} journaled, {} probes answered by the oracle",
        episode.receipts().count(),
        world.probes_answered()
    );
    println!(
        "verdict    answered={} frame_accepted={} correct={} agreement={:.3} ({}/{} rows)",
        verdict.answered,
        verdict.frame_accepted,
        verdict.correct,
        verdict.agreement,
        verdict.rows_correct,
        verdict.rows_total
    );
    for reason in &verdict.reasons {
        println!("           - {reason}");
    }
    println!(
        "journal    {} entries, head {}",
        episode.entries.len(),
        episode.head_hash()
    );
    println!("results    {}", dir.display());
    Ok(())
}

fn verify(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or("a journal path is required")?;
    let entries = read_journal(path).map_err(|e| e.to_string())?;
    let receipts = entries
        .iter()
        .filter(|e| matches!(e.event, metron_core::journal::JournalEvent::Receipt { .. }))
        .count();
    let head = entries
        .last()
        .map_or_else(|| "genesis".to_owned(), |e| e.hash.to_string());
    println!(
        "ok: {} entries, {receipts} receipts, head {head}",
        entries.len()
    );
    Ok(())
}

fn explain(args: &[String]) -> Result<(), String> {
    let path = PathBuf::from(args.first().ok_or("a manifest path is required")?);
    let manifest = load_manifest(&path)?;
    println!("experiment {} (seed {})", manifest.name, manifest.seed);
    println!("manifest   {}", manifest.hash());
    println!(
        "lab        family={} fixture={} max_probes={:?}",
        manifest.lab.family, manifest.lab.fixture, manifest.lab.max_probes
    );
    println!(
        "budget     calls={:?} work={:?} probes={:?} external={:?}; max_steps={} stall_limit={}",
        manifest.system.budget.max_operator_calls,
        manifest.system.budget.max_work_units,
        manifest.system.budget.max_oracle_probes,
        manifest.system.budget.max_external_calls,
        manifest.system.max_steps,
        manifest.system.stall_limit
    );
    println!("schedule");
    for step in &manifest.system.schedule {
        println!("  {} x{}", step.operator, step.repeat);
    }
    let mut registry: OperatorRegistry<LabWorld> = OperatorRegistry::new();
    registry
        .register_all(reference_operators())
        .map_err(|e| e.to_string())?;
    println!("operators");
    for spec in registry.specs() {
        let reads: Vec<&str> = spec.reads.iter().map(|v| v.as_str()).collect();
        let writes: Vec<&str> = spec.writes.iter().map(|v| v.as_str()).collect();
        println!(
            "  {} [{}] reads={reads:?} writes={writes:?}",
            spec.id,
            spec.kind.tag()
        );
        println!("      {}", spec.description);
        if let Some(contract) = spec.kind.contract() {
            println!("      contract  {contract}");
            for p in &contract.preserves {
                println!("      preserves {p}");
            }
            for l in &contract.loses {
                println!("      loses     {l}");
            }
            for a in &contract.assumes {
                println!("      assumes   {a}");
            }
        }
    }
    for step in &manifest.system.schedule {
        if registry.spec(&step.operator.as_str().into()).is_none() {
            println!(
                "warning: schedule names unknown operator `{}`",
                step.operator
            );
        }
    }
    Ok(())
}
