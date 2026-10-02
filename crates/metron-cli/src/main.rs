//! `metron`: the command-line composition root.

use metron_adapters::results::read_journal;
use metron_cli::run::Backend;
use metron_cli::{
    HeadroomOptions, PromoteOptions, RunOptions, RunStatus, answer, load_manifest, registry,
    replay, resume, run, run_headroom, run_promote,
};
use metron_core::journal::JournalEvent;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
usage:
  metron run <manifest.json> [--results <dir>] [--seed <n>] [--task <id|index>]
             [--strategy <name>] [--no-llm]
      Run one episode. If a consult operator needs an answer, the run
      suspends and prints the request; answer it, then `metron resume`.
  metron resume <run-dir>
      Continue a suspended run.
  metron answer <run-dir> [--request <id>] --text <formula> [--by <who>]
      Write the answer to a pending request.
  metron replay <run-dir>
      Re-run a completed run with its recorded answers and compare journals.
  metron headroom <manifest.json> [--results <dir>] [--seeds <n>] [--verbose]
      Run every strategy on every task and report SBS, VBS and gap closed.
  metron promote <manifest.json> [--results <dir>] [--strategy <name>] [--rounds <n>]
      Propose capability candidates from the train split and let the
      laboratory's gate judge them on the test split.
  metron verify <episode.jsonl>
      Verify a journal's hash chain and every receipt in it.
  metron explain <manifest.json>
      Print the strategies, the budget and every operator contract.
  metron npn-classes <arity>
      Count NPN classes by exhaustive enumeration (arity <= 4).
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("run") => cmd_run(&args[1..]),
        Some("resume") => cmd_resume(&args[1..]),
        Some("answer") => cmd_answer(&args[1..]),
        Some("replay") => cmd_replay(&args[1..]),
        Some("headroom") => cmd_headroom(&args[1..]),
        Some("promote") => cmd_promote(&args[1..]),
        Some("verify") => cmd_verify(&args[1..]),
        Some("explain") => cmd_explain(&args[1..]),
        Some("npn-classes") => cmd_npn(&args[1..]),
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

struct Flags {
    positional: Vec<String>,
    values: Vec<(String, String)>,
    switches: Vec<String>,
}

fn parse_flags(args: &[String], valued: &[&str], switches: &[&str]) -> Result<Flags, String> {
    let mut out = Flags {
        positional: Vec::new(),
        values: Vec::new(),
        switches: Vec::new(),
    };
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if let Some(name) = a.strip_prefix("--") {
            if switches.contains(&name) {
                out.switches.push(name.to_owned());
            } else if valued.contains(&name) {
                i += 1;
                let v = args
                    .get(i)
                    .ok_or_else(|| format!("--{name} needs a value"))?;
                out.values.push((name.to_owned(), v.clone()));
            } else {
                return Err(format!("unknown flag --{name}"));
            }
        } else {
            out.positional.push(a.to_owned());
        }
        i += 1;
    }
    Ok(out)
}

impl Flags {
    fn value(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    fn has(&self, name: &str) -> bool {
        self.switches.iter().any(|s| s == name)
    }
}

fn print_status(status: &RunStatus) {
    match status {
        RunStatus::Completed {
            dir,
            record,
            verdict,
            probes_answered,
        } => {
            let outcome = record.episode.outcome.as_ref();
            println!("run        {} (seed {})", record.name, record.episode.seed);
            println!("manifest   {}", record.episode.manifest_hash);
            if let Some(o) = outcome {
                println!("stop       {:?}", o.stop);
                println!(
                    "steps      {}  cost: calls={} work={} probes={} external={}",
                    o.steps,
                    o.cost.operator_calls,
                    o.cost.work_units,
                    o.cost.oracle_probes,
                    o.cost.external_calls
                );
            }
            println!(
                "receipts   {} journaled, {probes_answered} probes answered, {} service answers",
                record.episode.receipts().count(),
                record.episode.service_answers().count()
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
            if let (Some(f), Some(d)) = (&verdict.target_family, &verdict.target_description) {
                println!("target     [{f}] {d}");
            }
            for reason in &verdict.reasons {
                println!("           - {reason}");
            }
            println!(
                "journal    {} entries, head {}",
                record.episode.entries.len(),
                record.episode.head_hash()
            );
            println!("results    {}", dir.display());
        }
        RunStatus::Suspended {
            dir,
            request_id,
            request_path,
            prompt,
        } => {
            println!("suspended  waiting for request {request_id}");
            println!("request    {}", request_path.display());
            if let Some(p) = prompt {
                println!("prompt     ---\n{p}\n           ---");
            }
            println!(
                "answer     metron answer {} --text \"<formula>\" --by \"<who>\"   then   metron resume {}",
                dir.display(),
                dir.display()
            );
        }
    }
}

fn cmd_run(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args, &["results", "seed", "task", "strategy"], &["no-llm"])?;
    let manifest = flags
        .positional
        .first()
        .ok_or("a manifest path is required")?;
    let mut options = RunOptions::default();
    if let Some(r) = flags.value("results") {
        options.results_root = PathBuf::from(r);
    }
    if let Some(s) = flags.value("seed") {
        options.seed = Some(s.parse().map_err(|e| format!("--seed: {e}"))?);
    }
    options.task = flags.value("task").map(str::to_owned);
    options.strategy = flags.value("strategy").map(str::to_owned);
    if flags.has("no-llm") {
        options.backend = Backend::None;
    }
    let status = run(Path::new(manifest), &options)?;
    print_status(&status);
    Ok(())
}

fn cmd_resume(args: &[String]) -> Result<(), String> {
    let dir = args.first().ok_or("a run directory is required")?;
    let status = resume(Path::new(dir))?;
    print_status(&status);
    Ok(())
}

fn cmd_answer(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args, &["request", "text", "by"], &[])?;
    let dir = flags
        .positional
        .first()
        .ok_or("a run directory is required")?;
    let text = flags.value("text").ok_or("--text is required")?;
    let path = answer(
        Path::new(dir),
        flags.value("request"),
        text,
        flags.value("by").unwrap_or("claude-code"),
    )?;
    println!("answered   {}", path.display());
    Ok(())
}

fn cmd_replay(args: &[String]) -> Result<(), String> {
    let dir = args.first().ok_or("a run directory is required")?;
    let report = replay(Path::new(dir))?;
    println!(
        "replay     {}: {} recorded vs {} replayed effective events; answer {}",
        if report.matches { "match" } else { "MISMATCH" },
        report.recorded_events,
        report.replayed_events,
        if report.answer_matches {
            "matches"
        } else {
            "DIFFERS"
        }
    );
    println!("recorded   {}", report.recorded_head);
    println!("replayed   {}", report.replayed_head);
    if let Some(i) = report.first_difference {
        println!("differs    at effective event {i}");
    }
    if report.matches && report.answer_matches {
        Ok(())
    } else {
        Err("replay does not match".into())
    }
}

fn cmd_headroom(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args, &["results", "seeds"], &["verbose"])?;
    let manifest = flags
        .positional
        .first()
        .ok_or("a manifest path is required")?;
    let mut options = HeadroomOptions::default();
    if let Some(r) = flags.value("results") {
        options.results_root = PathBuf::from(r);
    }
    if let Some(s) = flags.value("seeds") {
        options.seeds = Some(s.parse().map_err(|e| format!("--seeds: {e}"))?);
    }
    options.verbose = flags.has("verbose");
    let outcome = run_headroom(Path::new(manifest), &options)?;
    print!("{}", outcome.markdown);
    println!("\nepisodes   {}", outcome.episodes);
    println!("results    {}", outcome.dir.display());
    Ok(())
}

fn cmd_promote(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args, &["results", "strategy", "rounds"], &[])?;
    let manifest = flags
        .positional
        .first()
        .ok_or("a manifest path is required")?;
    let mut options = PromoteOptions::default();
    if let Some(r) = flags.value("results") {
        options.results_root = PathBuf::from(r);
    }
    options.strategy = flags.value("strategy").map(str::to_owned);
    if let Some(r) = flags.value("rounds") {
        options.rounds = Some(r.parse().map_err(|e| format!("--rounds: {e}"))?);
    }
    let outcome = run_promote(Path::new(manifest), &options)?;
    print!("{}", outcome.markdown);
    println!("results    {}", outcome.dir.display());
    Ok(())
}

fn cmd_verify(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or("a journal path is required")?;
    let entries = read_journal(path).map_err(|e| e.to_string())?;
    let receipts = entries
        .iter()
        .filter(|e| matches!(e.event, JournalEvent::Receipt { .. }))
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

fn cmd_explain(args: &[String]) -> Result<(), String> {
    let path = PathBuf::from(args.first().ok_or("a manifest path is required")?);
    let manifest = load_manifest(&path)?;
    println!("experiment {} (seed {})", manifest.name, manifest.seed);
    println!("manifest   {}", manifest.hash());
    println!(
        "lab        family={} fixture={:?} tasks={} max_probes={:?}",
        manifest.lab.family(),
        manifest.lab.fixture(),
        manifest.lab.tasks().map_or("none".to_owned(), |t| format!(
            "arity {} / {} families x {} pool / {} targets",
            t.arity,
            t.pool.families.len(),
            t.pool.per_family,
            t.targets_per_family
        )),
        manifest.lab.max_probes()
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
    let registry = registry()?;
    for strategy in manifest.strategies() {
        let mut ops = Vec::new();
        for step in &strategy.schedule {
            step.operators(&mut ops);
        }
        println!("strategy   {} ({} applications)", strategy.name, ops.len());
        for op in ops.iter().collect::<std::collections::BTreeSet<_>>() {
            if registry.spec(&op.as_str().into()).is_none() {
                println!("  warning: unknown operator `{op}`");
            }
        }
    }
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
    Ok(())
}

fn cmd_npn(args: &[String]) -> Result<(), String> {
    let arity: u8 = args
        .first()
        .ok_or("an arity is required")?
        .parse()
        .map_err(|e| format!("arity: {e}"))?;
    if arity > 4 {
        return Err("exhaustive enumeration is only feasible for arity <= 4".into());
    }
    println!("{}", metron_lab::npn::class_count(arity));
    Ok(())
}
