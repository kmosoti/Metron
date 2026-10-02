//! Headroom analysis: single best solver, virtual best solver, and the gap
//! between them, following the ASlib conventions (Bischl et al. 2016).
//!
//! `gap closed` scores a selector between the single best solver (0) and
//! the virtual best solver (1). If the gap is small there is nothing for an
//! adaptive router to win and routing work stops.

use crate::stats::{iqm, mean, stratified_bootstrap};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// What one strategy achieved on one task.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Score {
    /// An answer was committed.
    pub answered: bool,
    /// The answer matched the target on every row.
    pub correct: bool,
    /// Oracle probes spent.
    pub probes: u64,
    /// Work units spent.
    pub work: u64,
    /// Operator calls spent.
    pub calls: u64,
    /// External service calls spent.
    pub external: u64,
}

/// Turns a score into a scalar cost. Pre-registered in the manifest.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CostModel {
    /// Weight of one probe.
    #[serde(default = "one")]
    pub probe_weight: f64,
    /// Weight of one work unit.
    #[serde(default = "zero")]
    pub work_weight: f64,
    /// Weight of one external call.
    #[serde(default = "zero")]
    pub external_weight: f64,
    /// Cost charged instead of the spent resources when the task was not
    /// solved (unanswered or incorrect), in the manner of PAR10.
    pub failure_cost: f64,
}

const fn one() -> f64 {
    1.0
}
const fn zero() -> f64 {
    0.0
}

impl CostModel {
    /// A probe-counting model with a PAR-style penalty of `failure_cost`.
    #[must_use]
    pub const fn probes_with_penalty(failure_cost: f64) -> Self {
        Self {
            probe_weight: 1.0,
            work_weight: 0.0,
            external_weight: 0.0,
            failure_cost,
        }
    }

    /// Scalar cost of a score.
    #[must_use]
    pub fn cost(&self, score: &Score) -> f64 {
        if !score.correct {
            return self.failure_cost;
        }
        self.probe_weight * score.probes as f64
            + self.work_weight * score.work as f64
            + self.external_weight * score.external as f64
    }
}

/// Identifies a task in a cost table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskKey {
    /// Task identifier.
    pub id: String,
    /// Family label.
    pub family: String,
    /// Split label.
    pub split: String,
    /// Task-set seed.
    pub seed: u64,
}

/// Scores of every strategy on every task.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CostTable {
    /// Strategy names, in column order.
    pub strategies: Vec<String>,
    /// Tasks, in row order.
    pub tasks: Vec<TaskKey>,
    /// `scores[task][strategy]`.
    pub scores: Vec<Vec<Score>>,
}

impl CostTable {
    /// An empty table with the given strategies.
    #[must_use]
    pub fn new(strategies: Vec<String>) -> Self {
        Self {
            strategies,
            tasks: Vec::new(),
            scores: Vec::new(),
        }
    }

    /// Adds a task row. `scores` must have one entry per strategy.
    pub fn push(&mut self, task: TaskKey, scores: Vec<Score>) {
        assert_eq!(
            scores.len(),
            self.strategies.len(),
            "one score per strategy"
        );
        self.tasks.push(task);
        self.scores.push(scores);
    }

    /// Adds a selector column: for each task, the score of the strategy
    /// `choice[task]` names. Used to score routers in the same table.
    pub fn push_selector(&mut self, name: impl Into<String>, choices: &[usize]) {
        assert_eq!(choices.len(), self.tasks.len(), "one choice per task");
        self.strategies.push(name.into());
        for (row, &c) in self.scores.iter_mut().zip(choices) {
            let s = row[c];
            row.push(s);
        }
    }

    /// Column index of a strategy.
    #[must_use]
    pub fn column(&self, strategy: &str) -> Option<usize> {
        self.strategies.iter().position(|s| s == strategy)
    }
}

/// Per-strategy summary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrategySummary {
    /// Name.
    pub name: String,
    /// Mean cost.
    pub mean_cost: f64,
    /// Interquartile mean cost.
    pub iqm_cost: f64,
    /// Bootstrap interval on the interquartile mean.
    pub iqm_ci: (f64, f64),
    /// Fraction of tasks solved correctly.
    pub solved: f64,
    /// Mean probes on solved tasks.
    pub mean_probes_when_solved: f64,
    /// Gap closed between SBS (0) and VBS (1); `None` when the gap is zero.
    pub gap_closed: Option<f64>,
}

/// Per-family summary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FamilySummary {
    /// Family label.
    pub family: String,
    /// Tasks.
    pub tasks: usize,
    /// Mean cost per strategy.
    pub mean_cost: BTreeMap<String, f64>,
    /// Strategy with the lowest mean cost on this family.
    pub best: String,
    /// Mean of the per-task minimum.
    pub vbs_cost: f64,
}

/// The analysis.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HeadroomReport {
    /// Cost model used.
    pub cost_model: CostModel,
    /// Number of tasks.
    pub tasks: usize,
    /// Strategies.
    pub strategies: Vec<StrategySummary>,
    /// Single best solver.
    pub sbs: String,
    /// Mean cost of the SBS.
    pub sbs_cost: f64,
    /// Mean of the per-task minimum cost.
    pub vbs_cost: f64,
    /// `sbs_cost - vbs_cost`.
    pub gap: f64,
    /// Bootstrap interval on the gap, stratified by family.
    pub gap_ci: (f64, f64),
    /// `vbs_cost / sbs_cost`.
    pub vbs_over_sbs: f64,
    /// How often each strategy is the per-task best.
    pub vbs_choices: BTreeMap<String, usize>,
    /// Per-family breakdown.
    pub families: Vec<FamilySummary>,
    /// The entropy floor in this cost model's units: no strategy that is
    /// not told the target's family averages less on correct answers.
    /// `None` when not computed, or when the failure cost is too low for the
    /// floor to bind.
    #[serde(default)]
    pub entropy_floor: Option<f64>,
}

impl HeadroomReport {
    /// Attaches the entropy floor, given in probes, scaled by this report's
    /// probe weight.
    #[must_use]
    pub fn with_entropy_floor(mut self, floor_probes: f64) -> Self {
        self.entropy_floor = Some(floor_probes * self.cost_model.probe_weight);
        self
    }

    /// The most a router that is not told the label can save against the
    /// single best solver: `SBS − floor`, at least zero.
    #[must_use]
    pub fn realisable_gap_bound(&self) -> Option<f64> {
        self.entropy_floor.map(|h| (self.sbs_cost - h).max(0.0))
    }
}

/// Analyses a cost table. `base` names the strategies that count as
/// candidates for the single best solver (selectors appended with
/// [`CostTable::push_selector`] should be excluded); `None` means all.
#[must_use]
pub fn analyze(
    table: &CostTable,
    model: &CostModel,
    base: Option<&[String]>,
    resamples: usize,
    seed: u64,
) -> HeadroomReport {
    let n_tasks = table.tasks.len();
    let n_strat = table.strategies.len();
    let costs: Vec<Vec<f64>> = table
        .scores
        .iter()
        .map(|row| row.iter().map(|s| model.cost(s)).collect())
        .collect();
    let base_columns: Vec<usize> = match base {
        Some(names) => table
            .strategies
            .iter()
            .enumerate()
            .filter(|(_, s)| names.contains(s))
            .map(|(i, _)| i)
            .collect(),
        None => (0..n_strat).collect(),
    };
    let mean_cost: Vec<f64> = (0..n_strat)
        .map(|j| mean(&costs.iter().map(|row| row[j]).collect::<Vec<_>>()))
        .collect();
    let sbs_index = base_columns
        .iter()
        .copied()
        .min_by(|&a, &b| mean_cost[a].partial_cmp(&mean_cost[b]).expect("finite"))
        .unwrap_or(0);
    let sbs_cost = mean_cost[sbs_index];
    let per_task_min: Vec<f64> = costs
        .iter()
        .map(|row| {
            base_columns
                .iter()
                .map(|&j| row[j])
                .fold(f64::INFINITY, f64::min)
        })
        .collect();
    let vbs_cost = mean(&per_task_min);
    let gap = sbs_cost - vbs_cost;

    // Families as strata.
    let families: Vec<String> = {
        let mut f: Vec<String> = table.tasks.iter().map(|t| t.family.clone()).collect();
        f.sort();
        f.dedup();
        f
    };
    let strata = |values: &dyn Fn(usize) -> f64| -> Vec<Vec<f64>> {
        families
            .iter()
            .map(|fam| {
                table
                    .tasks
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| &t.family == fam)
                    .map(|(i, _)| values(i))
                    .collect()
            })
            .collect()
    };
    let gap_groups = strata(&|i| costs[i][sbs_index] - per_task_min[i]);
    let (_, gap_lo, gap_hi) = stratified_bootstrap(&gap_groups, mean, resamples, 0.05, seed);

    let mut vbs_choices: BTreeMap<String, usize> = BTreeMap::new();
    for row in &costs {
        let best = base_columns
            .iter()
            .copied()
            .min_by(|&a, &b| row[a].partial_cmp(&row[b]).expect("finite"))
            .unwrap_or(0);
        *vbs_choices
            .entry(table.strategies[best].clone())
            .or_insert(0) += 1;
    }

    let strategies: Vec<StrategySummary> = (0..n_strat)
        .map(|j| {
            let column: Vec<f64> = costs.iter().map(|row| row[j]).collect();
            let groups = strata(&|i| costs[i][j]);
            let (iqm_cost, lo, hi) =
                stratified_bootstrap(&groups, iqm, resamples, 0.05, seed ^ (j as u64 + 1));
            let solved: Vec<&Score> = table
                .scores
                .iter()
                .map(|row| &row[j])
                .filter(|s| s.correct)
                .collect();
            let gap_closed = if gap.abs() < f64::EPSILON {
                None
            } else {
                Some((sbs_cost - mean_cost[j]) / gap)
            };
            StrategySummary {
                name: table.strategies[j].clone(),
                mean_cost: mean(&column),
                iqm_cost,
                iqm_ci: (lo, hi),
                solved: if n_tasks == 0 {
                    0.0
                } else {
                    solved.len() as f64 / n_tasks as f64
                },
                mean_probes_when_solved: mean(
                    &solved.iter().map(|s| s.probes as f64).collect::<Vec<_>>(),
                ),
                gap_closed,
            }
        })
        .collect();

    let family_summaries: Vec<FamilySummary> = families
        .iter()
        .map(|fam| {
            let rows: Vec<usize> = table
                .tasks
                .iter()
                .enumerate()
                .filter(|(_, t)| &t.family == fam)
                .map(|(i, _)| i)
                .collect();
            let mean_by: BTreeMap<String, f64> = (0..n_strat)
                .map(|j| {
                    (
                        table.strategies[j].clone(),
                        mean(&rows.iter().map(|&i| costs[i][j]).collect::<Vec<_>>()),
                    )
                })
                .collect();
            let best = base_columns
                .iter()
                .copied()
                .min_by(|&a, &b| {
                    mean_by[&table.strategies[a]]
                        .partial_cmp(&mean_by[&table.strategies[b]])
                        .expect("finite")
                })
                .map(|j| table.strategies[j].clone())
                .unwrap_or_default();
            FamilySummary {
                family: fam.clone(),
                tasks: rows.len(),
                mean_cost: mean_by,
                best,
                vbs_cost: mean(&rows.iter().map(|&i| per_task_min[i]).collect::<Vec<_>>()),
            }
        })
        .collect();

    HeadroomReport {
        cost_model: *model,
        tasks: n_tasks,
        strategies,
        sbs: table.strategies[sbs_index].clone(),
        sbs_cost,
        vbs_cost,
        gap,
        gap_ci: (gap_lo, gap_hi),
        vbs_over_sbs: if sbs_cost > 0.0 {
            vbs_cost / sbs_cost
        } else {
            1.0
        },
        vbs_choices,
        families: family_summaries,
        entropy_floor: None,
    }
}

/// Renders a report as Markdown.
#[must_use]
pub fn render_markdown(report: &HeadroomReport, title: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# {title}\n");
    let _ = writeln!(
        out,
        "Tasks: {}. Cost model: probe weight {}, work weight {}, external weight {}, failure cost {}.\n",
        report.tasks,
        report.cost_model.probe_weight,
        report.cost_model.work_weight,
        report.cost_model.external_weight,
        report.cost_model.failure_cost
    );
    let _ = writeln!(
        out,
        "**Single best solver:** `{}` at mean cost {:.3}. **Virtual best solver:** mean cost {:.3}. **Gap:** {:.3} (95% CI {:.3} to {:.3}); VBS/SBS = {:.3}.\n",
        report.sbs,
        report.sbs_cost,
        report.vbs_cost,
        report.gap,
        report.gap_ci.0,
        report.gap_ci.1,
        report.vbs_over_sbs
    );
    if let (Some(floor), Some(bound)) = (report.entropy_floor, report.realisable_gap_bound()) {
        let share = if report.gap > 0.0 {
            format!(", {:.1}% of the gap", 100.0 * bound / report.gap)
        } else {
            String::new()
        };
        let _ = writeln!(
            out,
            "**Entropy floor:** {floor:.3}. A strategy that is not told the target's family averages at least this much on correct answers, so a router can save at most {bound:.3} against the single best solver{share}.\n"
        );
    }
    let _ = writeln!(
        out,
        "| Strategy | Mean cost | IQM cost | IQM 95% CI | Solved | Mean probes (solved) | Gap closed |"
    );
    let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|---:|");
    for s in &report.strategies {
        let _ = writeln!(
            out,
            "| `{}` | {:.3} | {:.3} | {:.3} to {:.3} | {:.1}% | {:.2} | {} |",
            s.name,
            s.mean_cost,
            s.iqm_cost,
            s.iqm_ci.0,
            s.iqm_ci.1,
            s.solved * 100.0,
            s.mean_probes_when_solved,
            s.gap_closed.map_or("n/a".to_owned(), |g| format!("{g:.3}"))
        );
    }
    let _ = writeln!(out, "\n## Per-task best (VBS choices)\n");
    let _ = writeln!(out, "| Strategy | Tasks where best |");
    let _ = writeln!(out, "|---|---:|");
    for (s, n) in &report.vbs_choices {
        let _ = writeln!(out, "| `{s}` | {n} |");
    }
    let _ = writeln!(out, "\n## Per family\n");
    let names: Vec<&str> = report.strategies.iter().map(|s| s.name.as_str()).collect();
    let _ = writeln!(
        out,
        "| Family | Tasks | {} | Best | VBS |",
        names
            .iter()
            .map(|n| format!("`{n}`"))
            .collect::<Vec<_>>()
            .join(" | ")
    );
    let _ = writeln!(out, "|---|---:|{}---:|---:|", "---:|".repeat(names.len()));
    for f in &report.families {
        let cells: Vec<String> = names
            .iter()
            .map(|n| format!("{:.2}", f.mean_cost.get(*n).copied().unwrap_or(f64::NAN)))
            .collect();
        let _ = writeln!(
            out,
            "| {} | {} | {} | `{}` | {:.2} |",
            f.family,
            f.tasks,
            cells.join(" | "),
            f.best,
            f.vbs_cost
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(correct: bool, probes: u64) -> Score {
        Score {
            answered: true,
            correct,
            probes,
            work: 0,
            calls: 0,
            external: 0,
        }
    }

    #[test]
    fn sbs_vbs_and_gap_closed() {
        let mut table = CostTable::new(vec!["a".into(), "b".into()]);
        for i in 0..4 {
            let family = if i % 2 == 0 { "even" } else { "odd" };
            // `a` is cheap on even tasks, `b` on odd tasks.
            let (sa, sb) = if i % 2 == 0 {
                (score(true, 2), score(true, 6))
            } else {
                (score(true, 6), score(true, 2))
            };
            table.push(
                TaskKey {
                    id: format!("t{i}"),
                    family: family.into(),
                    split: "test".into(),
                    seed: 0,
                },
                vec![sa, sb],
            );
        }
        // A perfect selector picks the cheaper strategy on each task.
        table.push_selector("oracle-selector", &[0, 1, 0, 1]);
        let base = vec!["a".to_owned(), "b".to_owned()];
        let report = analyze(
            &table,
            &CostModel::probes_with_penalty(100.0),
            Some(&base),
            200,
            1,
        );
        assert_eq!(report.sbs_cost, 4.0);
        assert_eq!(report.vbs_cost, 2.0);
        assert_eq!(report.gap, 2.0);
        let selector = report
            .strategies
            .iter()
            .find(|s| s.name == "oracle-selector")
            .unwrap();
        assert_eq!(selector.gap_closed, Some(1.0));
        assert_eq!(report.strategies[0].gap_closed, Some(0.0));
        assert_eq!(report.vbs_choices["a"], 2);
        assert_eq!(report.families.len(), 2);
        let md = render_markdown(&report, "test");
        assert!(md.contains("oracle-selector"));
    }

    #[test]
    fn failures_are_penalised() {
        let model = CostModel::probes_with_penalty(50.0);
        assert_eq!(model.cost(&score(false, 3)), 50.0);
        assert_eq!(model.cost(&score(true, 3)), 3.0);
    }
}
