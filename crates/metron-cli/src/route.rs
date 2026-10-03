//! Routing measurement (ADR 0015): every label-free fixed strategy and
//! every router candidate after every prefix, on every task of every seed;
//! routers fitted on the train split, chosen on validation, judged once on
//! test; the hand-authored and the selected routers replayed as real
//! schedulers on the test split to confirm the simulated choices.

use crate::compose::{load_manifest, registry, schedule_from};
use crate::headroom::{EpisodeContext, entropy_floor, run_scored};
use metron_adapters::{SystemClock, write_json_pretty};
use metron_app::{
    Candidate, EpisodeRunner, Example, Posterior, RouterSelector, RoutingPolicy, RunConfig,
    fit_ridge, fit_tabular,
};
use metron_core::id::{OperatorId, ViewId};
use metron_lab::manifest::{RouteSpec, ScheduleStep, Strategy};
use metron_lab::routing::{
    RouteReport, RouteTable, RouteTask, RouterChoices, analyze_routes, render_route_markdown,
};
use metron_lab::tasks::verify_pooled_split_hygiene;
use metron_lab::{CostModel, Protocol, TaskSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// The hand-authored control's name in reports.
pub const CONTROL: &str = "posterior-order";

/// Options for `route`.
#[derive(Clone, Debug)]
pub struct RouteOptions {
    /// Root under which the report directory is written.
    pub results_root: PathBuf,
    /// Overrides the manifest's number of seeds.
    pub seeds: Option<u32>,
    /// Prints progress to stderr.
    pub verbose: bool,
}

impl Default for RouteOptions {
    fn default() -> Self {
        Self {
            results_root: PathBuf::from("experiments/results"),
            seeds: None,
            verbose: false,
        }
    }
}

/// What `route` produced.
#[derive(Clone, Debug)]
pub struct RouteOutcome {
    /// Where the files went.
    pub dir: PathBuf,
    /// One report per prefix, the primary first.
    pub reports: Vec<RouteReport>,
    /// Markdown rendering.
    pub markdown: String,
    /// Episodes run, including the replays.
    pub episodes: usize,
    /// Test tasks on which a replayed router matched its simulated choice.
    pub replayed: usize,
}

fn strategy<'a>(strategies: &'a [Strategy], name: &str) -> Result<&'a Strategy, String> {
    strategies
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| format!("no strategy named `{name}`"))
}

fn operators(steps: &[ScheduleStep]) -> Vec<OperatorId> {
    let mut names = Vec::new();
    for s in steps {
        s.operators(&mut names);
    }
    names.into_iter().map(OperatorId::from).collect()
}

/// The fitted routers for one prefix, with the policy behind each.
struct Fitted {
    choices: Vec<RouterChoices>,
    policies: Vec<(String, RoutingPolicy)>,
    selection: Vec<String>,
}

fn fit_routers(
    spec: &RouteSpec,
    candidates: &[Candidate],
    table: &RouteTable,
    posteriors: &[Posterior],
    model: &CostModel,
) -> Fitted {
    let costs: Vec<Vec<f64>> = table
        .tasks
        .iter()
        .map(|t| t.prefixed.iter().map(|s| model.cost(s)).collect())
        .collect();
    let rows = |split: &str| -> Vec<usize> {
        table
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.split == split)
            .map(|(i, _)| i)
            .collect()
    };
    let train = rows("train");
    let validation = rows("validation");
    let examples: Vec<Example> = train
        .iter()
        .map(|&i| (posteriors[i].clone(), costs[i].clone()))
        .collect();
    let choose_all = |policy: &RoutingPolicy| -> Vec<usize> {
        posteriors
            .iter()
            .map(|p| policy.choose(p, candidates))
            .collect()
    };
    let mean_on = |choices: &[usize], rows: &[usize]| -> f64 {
        if rows.is_empty() {
            return 0.0;
        }
        rows.iter().map(|&i| costs[i][choices[i]]).sum::<f64>() / rows.len() as f64
    };
    // The candidate cheapest on train, for buckets the train split never
    // filled.
    let train_best = (0..candidates.len())
        .min_by(|&a, &b| {
            let ca: f64 = train.iter().map(|&i| costs[i][a]).sum();
            let cb: f64 = train.iter().map(|&i| costs[i][b]).sum();
            ca.total_cmp(&cb)
        })
        .unwrap_or(0);
    let control = RoutingPolicy::PosteriorOrder {
        highest_first: true,
    };
    let reverse = RoutingPolicy::PosteriorOrder {
        highest_first: false,
    };
    let tabular = fit_tabular(&examples, &spec.tabular_bins, train_best);
    let ridge = spec
        .ridge_lambdas
        .iter()
        .map(|&l| fit_ridge(&examples, l))
        .min_by(|a, b| {
            mean_on(&choose_all(a), &validation).total_cmp(&mean_on(&choose_all(b), &validation))
        })
        .unwrap_or_else(|| fit_ridge(&examples, 1.0));
    let named = vec![
        (CONTROL.to_owned(), control, false),
        ("posterior-reverse".to_owned(), reverse, false),
        ("tabular".to_owned(), tabular, true),
        (ridge.label(), ridge, true),
    ];
    let selection = vec![CONTROL.to_owned(), "tabular".to_owned(), named[3].0.clone()];
    Fitted {
        choices: named
            .iter()
            .map(|(name, policy, learned)| RouterChoices {
                name: name.clone(),
                learned: *learned,
                choices: choose_all(policy),
            })
            .collect(),
        policies: named
            .into_iter()
            .map(|(name, policy, _)| (name, policy))
            .collect(),
        selection,
    }
}

/// Runs the routing measurement described by a manifest.
pub fn run_route(manifest_path: &Path, options: &RouteOptions) -> Result<RouteOutcome, String> {
    let manifest = load_manifest(manifest_path)?;
    let spec = manifest
        .lab
        .tasks()
        .ok_or("route needs a task-set manifest")?
        .clone();
    let headroom = manifest
        .headroom
        .clone()
        .ok_or("route needs a `headroom` section")?;
    let route = manifest
        .route
        .clone()
        .ok_or("manifest has no `route` section")?;
    let seeds = options.seeds.unwrap_or(headroom.seeds).max(1);
    let strategies = manifest.strategies();
    let fixed_names = manifest.fixed_strategy_names();
    let fixed: Vec<&Strategy> = fixed_names
        .iter()
        .map(|n| strategy(&strategies, n))
        .collect::<Result<_, _>>()?;
    let candidates: Vec<Candidate> = route
        .candidates
        .iter()
        .map(|c| Candidate {
            name: c.strategy.clone(),
            order: c.order.clone(),
        })
        .collect();
    let candidate_steps: Vec<Vec<ScheduleStep>> = route
        .candidates
        .iter()
        .map(|c| strategy(&strategies, &c.strategy).map(|s| s.schedule.clone()))
        .collect::<Result<_, _>>()?;
    let profile_view = ViewId::from(route.profile_view.as_str());
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

    let sets: Vec<TaskSet> = (0..u64::from(seeds))
        .map(|offset| TaskSet::generate(&spec, manifest.seed.wrapping_add(offset)))
        .collect();
    for set in &sets {
        set.verify_split_hygiene()?;
    }
    verify_pooled_split_hygiene(&sets)?;
    let floors: Vec<(f64, f64)> = sets
        .iter()
        .map(|s| (s.entropy_floor(), s.support_size()))
        .collect();

    let mut tables: Vec<RouteTable> = route
        .prefixes
        .iter()
        .map(|p| RouteTable {
            prefix: p.name.clone(),
            fixed: fixed_names.clone(),
            candidates: candidates.iter().map(|c| c.name.clone()).collect(),
            tasks: Vec::new(),
        })
        .collect();
    let mut posteriors: Vec<Vec<Posterior>> = vec![Vec::new(); route.prefixes.len()];
    let mut episodes = 0usize;
    for set in &sets {
        if options.verbose {
            eprintln!(
                "seed {}: {} tasks, {} NPN classes",
                set.seed,
                set.tasks.len(),
                set.class_count()
            );
        }
        for task in &set.tasks {
            let mut fixed_scores = Vec::with_capacity(fixed.len());
            for s in &fixed {
                let mut scheduler = schedule_from(&s.schedule);
                let (score, _) = run_scored(&context, set, task, set.seed, &mut scheduler)
                    .map_err(|e| format!("strategy `{}`: {e}", s.name))?;
                fixed_scores.push(score);
                episodes += 1;
            }
            for (p, prefix) in route.prefixes.iter().enumerate() {
                let mut prefixed = Vec::with_capacity(candidates.len());
                let mut seen: Option<Posterior> = None;
                for (c, steps) in candidate_steps.iter().enumerate() {
                    let mut all = prefix.steps.clone();
                    all.extend(steps.iter().cloned());
                    let mut scheduler = schedule_from(&all);
                    let (score, inquiry) =
                        run_scored(&context, set, task, set.seed, &mut scheduler).map_err(|e| {
                            format!(
                                "prefix `{}` then `{}`: {e}",
                                prefix.name, candidates[c].name
                            )
                        })?;
                    episodes += 1;
                    let posterior =
                        Posterior::from_view(&inquiry, &profile_view).ok_or_else(|| {
                            format!(
                                "prefix `{}` wrote no `{}` view on task {}",
                                prefix.name, route.profile_view, task.id
                            )
                        })?;
                    if let Some(prev) = &seen
                        && prev != &posterior
                    {
                        return Err(format!(
                            "prefix `{}` gave different posteriors on task {}",
                            prefix.name, task.id
                        ));
                    }
                    seen = Some(posterior);
                    prefixed.push(score);
                }
                posteriors[p].push(seen.ok_or("no candidates")?);
                tables[p].tasks.push(RouteTask {
                    id: format!("{}:{}", set.seed, task.id),
                    class: task.target.family.label().to_owned(),
                    split: task.split.label().to_owned(),
                    fixed: fixed_scores.clone(),
                    prefixed,
                });
            }
        }
    }

    let model = headroom.cost_model;
    let floor = entropy_floor(&floors, protocol.max_probes, spec.arity)
        .filter(|&(_, needed)| model.failure_cost >= needed * model.probe_weight)
        .map(|(h, _)| h * model.probe_weight);
    let mut reports = Vec::new();
    let mut replayed = 0usize;
    for (p, table) in tables.iter().enumerate() {
        let fitted = fit_routers(&route, &candidates, table, &posteriors[p], &model);
        let mut report = analyze_routes(
            table,
            &model,
            &fitted.choices,
            CONTROL,
            &fitted.selection,
            headroom.resamples,
            manifest.seed ^ (p as u64 + 1),
        );
        report.entropy_floor = floor;
        if p == 0 {
            // Replay the control and the selected router as real schedulers
            // on every test task: the simulated choice and cost must match.
            let index: Vec<(u64, &metron_lab::Task, &TaskSet)> = sets
                .iter()
                .flat_map(|s| s.tasks.iter().map(move |t| (s.seed, t, s)))
                .collect();
            for name in [CONTROL.to_owned(), report.selected.clone()] {
                let (_, policy) = fitted
                    .policies
                    .iter()
                    .find(|(n, _)| *n == name)
                    .ok_or("selected router has no policy")?;
                let simulated = &fitted
                    .choices
                    .iter()
                    .find(|r| r.name == name)
                    .ok_or("selected router has no choices")?
                    .choices;
                for (i, (seed, task, set)) in index.iter().enumerate() {
                    if table.tasks[i].split != "test" {
                        continue;
                    }
                    let pairs = candidates
                        .iter()
                        .cloned()
                        .zip(candidate_steps.iter().map(|s| schedule_from(s)))
                        .collect();
                    let mut router = RouterSelector::new(
                        operators(&route.prefixes[0].steps),
                        profile_view.clone(),
                        pairs,
                        policy.clone(),
                    );
                    let (score, _) = run_scored(&context, set, task, *seed, &mut router)?;
                    episodes += 1;
                    let chosen = router
                        .chosen()
                        .and_then(|c| candidates.iter().position(|x| x == c))
                        .ok_or("the router chose nothing")?;
                    if chosen != simulated[i] || score != table.tasks[i].prefixed[chosen] {
                        return Err(format!(
                            "router `{name}` replayed on task {} chose {} (simulated {}) or scored differently",
                            table.tasks[i].id, chosen, simulated[i]
                        ));
                    }
                    replayed += 1;
                }
            }
        }
        reports.push(report);
    }

    let markdown = render(&manifest.name, seeds, spec.arity, &reports, replayed);
    let dir = options.results_root.join(format!(
        "route-{}-{}",
        manifest.name,
        manifest.hash().short()
    ));
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    write_json_pretty(dir.join("manifest.json"), &manifest).map_err(|e| e.to_string())?;
    write_json_pretty(dir.join("route-tables.json"), &tables).map_err(|e| e.to_string())?;
    write_json_pretty(dir.join("route.json"), &reports).map_err(|e| e.to_string())?;
    fs::write(dir.join("route.md"), &markdown).map_err(|e| e.to_string())?;
    Ok(RouteOutcome {
        dir,
        reports,
        markdown,
        episodes,
        replayed,
    })
}

/// The pre-registered verdicts of ADR 0015 on the primary prefix.
fn verdicts(report: &RouteReport) -> Vec<String> {
    let mut out = Vec::new();
    let excludes_zero =
        |r: &metron_lab::routing::RouterResult| r.gap_closed_ci.is_some_and(|(lo, _)| lo > 0.0);
    let paying: Vec<&str> = report
        .routers
        .iter()
        .filter(|r| r.name != "posterior-reverse" && excludes_zero(r))
        .map(|r| r.name.as_str())
        .collect();
    out.push(format!(
        "Routing pays: {}.",
        if paying.is_empty() {
            "no; no router's gap-closed interval on test excludes zero".to_owned()
        } else {
            format!(
                "yes; the gap-closed interval on test excludes zero for {}",
                paying
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    ));
    let learned_better: Vec<&str> = report
        .routers
        .iter()
        .filter(|r| r.learned && r.minus_control.is_some_and(|(_, _, hi)| hi < 0.0))
        .map(|r| r.name.as_str())
        .collect();
    out.push(format!(
        "Learning pays: {}.",
        if learned_better.is_empty() {
            "no; no learned router beats the control with an interval on the paired difference that excludes zero".to_owned()
        } else {
            format!(
                "yes; {} beat the control",
                learned_better
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    ));
    if let Some(control) = report.routers.iter().find(|r| r.name == CONTROL) {
        out.push(format!(
            "Prediction 3 (the control closes at least a third of the gap): {}; it closes {} (95% CI {}).",
            if control.gap_closed.is_some_and(|g| g >= 1.0 / 3.0) {
                "holds"
            } else {
                "fails"
            },
            control
                .gap_closed
                .map_or("n/a".to_owned(), |g| format!("{g:.3}")),
            control
                .gap_closed_ci
                .map_or("n/a".to_owned(), |(lo, hi)| format!("{lo:.3} to {hi:.3}"))
        ));
    }
    out.push(format!(
        "Prediction 4 (no learned router beats the control): {}.",
        if learned_better.is_empty() {
            "holds"
        } else {
            "fails"
        }
    ));
    out
}

fn render(name: &str, seeds: u32, arity: u8, reports: &[RouteReport], replayed: usize) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Routing: {name} ({seeds} seeds, arity {arity})\n");
    if let Some(primary) = reports.first() {
        let _ = writeln!(
            out,
            "Tasks: {} train, {} validation, {} test; splits by NPN-class hash, pooled hygiene verified. Cost model: probe weight {}, failure cost {}. Routers fitted on train, chosen on validation, judged once on test. The control and the selected router were replayed as real schedulers on {replayed} test episodes; every choice and score matched.\n",
            primary.tasks["train"],
            primary.tasks["validation"],
            primary.tasks["test"],
            primary.cost_model.probe_weight,
            primary.cost_model.failure_cost
        );
        let _ = writeln!(
            out,
            "## Verdicts (primary prefix, pre-registered in ADR 0015)\n"
        );
        for v in verdicts(primary) {
            let _ = writeln!(out, "- {v}");
        }
        let _ = writeln!(out);
    }
    for (i, r) in reports.iter().enumerate() {
        if i == 1 {
            let _ = writeln!(out, "# Secondary prefixes\n");
        }
        out.push_str(&render_route_markdown(r));
        out.push('\n');
    }
    out
}
