//! Routing analysis (ADR 0015): routers fitted on the train split, chosen
//! on validation, judged once on the test split against the single best
//! fixed strategy chosen on train and the virtual best on test.
//!
//! A router's column is the cost of the candidate it chose on each task,
//! run after the router's prefix, so the prefix is paid. Intervals are
//! percentile bootstraps that resample test tasks within each class.

use crate::headroom::{CostModel, Score};
use crate::stats::mean;
use metron_core::rng::Rng;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// One task's measurements.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RouteTask {
    /// Task identifier, unique across seeds.
    pub id: String,
    /// Class label (laboratory side, for strata and reports only).
    pub class: String,
    /// Split label.
    pub split: String,
    /// Scores of the label-free fixed strategies, in `RouteTable::fixed` order.
    pub fixed: Vec<Score>,
    /// Scores of the candidates run after the prefix, in candidate order.
    pub prefixed: Vec<Score>,
}

/// Everything a routing analysis needs for one prefix.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RouteTable {
    /// Prefix name.
    pub prefix: String,
    /// Label-free fixed strategies.
    pub fixed: Vec<String>,
    /// Router candidates.
    pub candidates: Vec<String>,
    /// Tasks.
    pub tasks: Vec<RouteTask>,
}

/// A router's choices, one candidate index per task, in task order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RouterChoices {
    /// Router name.
    pub name: String,
    /// Whether it was fitted on the train split.
    pub learned: bool,
    /// Candidate index per task.
    pub choices: Vec<usize>,
}

/// One router's results.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RouterResult {
    /// Router name.
    pub name: String,
    /// Whether it was fitted on the train split.
    pub learned: bool,
    /// Mean cost per split.
    pub cost: BTreeMap<String, f64>,
    /// Fraction of test tasks solved.
    pub test_solved: f64,
    /// Gap closed on test, against the train-chosen single best and the
    /// test virtual best.
    pub gap_closed: Option<f64>,
    /// 95% interval on the gap closed.
    pub gap_closed_ci: Option<(f64, f64)>,
    /// Mean test cost minus the control's, with its 95% interval.
    pub minus_control: Option<(f64, f64, f64)>,
    /// Mean test cost per class.
    pub test_cost_by_class: BTreeMap<String, f64>,
    /// Test choices: class to candidate to count.
    pub test_choices: BTreeMap<String, BTreeMap<String, usize>>,
    /// Mean test cost minus that of the best fixed order run after the
    /// same prefix (chosen on train), with its 95% interval: the share of
    /// the ordering itself, net of what the prefix contributes. A post-hoc
    /// analysis (ADR 0017), absent from reports written before it.
    #[serde(default)]
    pub minus_prefixed_sbs: Option<(f64, f64, f64)>,
}

/// The analysis of one prefix.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RouteReport {
    /// Prefix name.
    pub prefix: String,
    /// Cost model.
    pub cost_model: CostModel,
    /// Tasks per split.
    pub tasks: BTreeMap<String, usize>,
    /// The single best fixed strategy on train.
    pub sbs: String,
    /// Its mean cost per split.
    pub sbs_cost: BTreeMap<String, f64>,
    /// Mean per-task minimum over the fixed strategies, on test.
    pub vbs_test: f64,
    /// `sbs` test cost minus `vbs_test`.
    pub gap_test: f64,
    /// Mean per-task minimum over the router's candidates after the
    /// prefix, on test: the best any router with this prefix can do.
    pub oracle_router_test: f64,
    /// Mean test cost of each fixed strategy.
    pub fixed_test: BTreeMap<String, f64>,
    /// The entropy floor, when known.
    pub entropy_floor: Option<f64>,
    /// Routers, in the order given.
    pub routers: Vec<RouterResult>,
    /// The router chosen on validation among the control and the learned
    /// routers.
    pub selected: String,
    /// The best fixed order run after the same prefix, chosen on train
    /// (post hoc, ADR 0017).
    #[serde(default)]
    pub prefixed_sbs: Option<String>,
    /// Its mean cost per split.
    #[serde(default)]
    pub prefixed_sbs_cost: BTreeMap<String, f64>,
    /// Whether the comparison with the best fixed order after the prefix
    /// was pre-registered as the primary comparison (ADR 0017) rather than
    /// run post hoc.
    #[serde(default)]
    pub ordering_is_primary: bool,
}

fn split_rows(table: &RouteTable, split: &str) -> Vec<usize> {
    table
        .tasks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.split == split)
        .map(|(i, _)| i)
        .collect()
}

/// Mean cost over `rows` of a per-task cost function.
fn mean_over(rows: &[usize], cost: &dyn Fn(usize) -> f64) -> f64 {
    mean(&rows.iter().map(|&i| cost(i)).collect::<Vec<_>>())
}

/// Percentile interval of `statistic` over resamples of `rows` drawn
/// within each stratum.
fn bootstrap(
    strata: &[Vec<usize>],
    statistic: &dyn Fn(&[usize]) -> Option<f64>,
    resamples: usize,
    seed: u64,
) -> Option<(f64, f64)> {
    let mut rng = Rng::seed_from_u64(seed);
    let mut values = Vec::with_capacity(resamples);
    let mut sample = Vec::new();
    for _ in 0..resamples {
        sample.clear();
        for group in strata {
            for _ in 0..group.len() {
                sample.push(group[rng.below_usize(group.len())]);
            }
        }
        if let Some(v) = statistic(&sample) {
            values.push(v);
        }
    }
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let at =
        |q: f64| values[((q * (values.len() - 1) as f64).round() as usize).min(values.len() - 1)];
    Some((at(0.025), at(0.975)))
}

/// Analyses one prefix's routers. `control` names the hand-authored router
/// the learned ones are compared with; `selection` names the routers the
/// validation split chooses among.
#[must_use]
pub fn analyze_routes(
    table: &RouteTable,
    model: &CostModel,
    routers: &[RouterChoices],
    control: &str,
    selection: &[String],
    resamples: usize,
    seed: u64,
) -> RouteReport {
    let fixed_cost = |i: usize, j: usize| model.cost(&table.tasks[i].fixed[j]);
    let prefixed_cost = |i: usize, c: usize| model.cost(&table.tasks[i].prefixed[c]);
    let splits = ["train", "validation", "test"];
    let rows: BTreeMap<&str, Vec<usize>> =
        splits.iter().map(|s| (*s, split_rows(table, s))).collect();
    let train = &rows["train"];
    let test = &rows["test"];
    let sbs_index = (0..table.fixed.len())
        .min_by(|&a, &b| {
            mean_over(train, &|i| fixed_cost(i, a))
                .total_cmp(&mean_over(train, &|i| fixed_cost(i, b)))
        })
        .unwrap_or(0);
    let vbs_cost = |i: usize| {
        (0..table.fixed.len())
            .map(|j| fixed_cost(i, j))
            .fold(f64::INFINITY, f64::min)
    };
    let oracle_cost = |i: usize| {
        (0..table.candidates.len())
            .map(|c| prefixed_cost(i, c))
            .fold(f64::INFINITY, f64::min)
    };
    let sbs_test = mean_over(test, &|i| fixed_cost(i, sbs_index));
    let vbs_test = mean_over(test, &vbs_cost);
    let gap_test = sbs_test - vbs_test;
    let classes: Vec<String> = {
        let mut c: Vec<String> = table.tasks.iter().map(|t| t.class.clone()).collect();
        c.sort();
        c.dedup();
        c
    };
    let strata: Vec<Vec<usize>> = classes
        .iter()
        .map(|c| {
            test.iter()
                .copied()
                .filter(|&i| &table.tasks[i].class == c)
                .collect()
        })
        .filter(|g: &Vec<usize>| !g.is_empty())
        .collect();
    let control_choices = routers
        .iter()
        .find(|r| r.name == control)
        .map(|r| r.choices.clone());
    let prefixed_sbs_index = (0..table.candidates.len())
        .min_by(|&a, &b| {
            mean_over(train, &|i| prefixed_cost(i, a))
                .total_cmp(&mean_over(train, &|i| prefixed_cost(i, b)))
        })
        .unwrap_or(0);

    let results: Vec<RouterResult> = routers
        .iter()
        .enumerate()
        .map(|(k, r)| {
            let router_cost = |i: usize| prefixed_cost(i, r.choices[i]);
            let cost: BTreeMap<String, f64> = splits
                .iter()
                .map(|s| ((*s).to_owned(), mean_over(&rows[s], &router_cost)))
                .collect();
            let test_cost = cost["test"];
            let gap_closed = (gap_test > 0.0).then(|| (sbs_test - test_cost) / gap_test);
            let gap_closed_ci = bootstrap(
                &strata,
                &|sample| {
                    let sbs = mean_over(sample, &|i| fixed_cost(i, sbs_index));
                    let vbs = mean_over(sample, &vbs_cost);
                    let r = mean_over(sample, &router_cost);
                    (sbs - vbs > 0.0).then(|| (sbs - r) / (sbs - vbs))
                },
                resamples,
                seed ^ (k as u64 + 1),
            );
            let minus_control = control_choices.as_ref().and_then(|cc| {
                if r.name == control {
                    return None;
                }
                let control_cost = |i: usize| prefixed_cost(i, cc[i]);
                let diff = |sample: &[usize]| -> Option<f64> {
                    Some(mean_over(sample, &router_cost) - mean_over(sample, &control_cost))
                };
                let point = diff(test)?;
                let (lo, hi) = bootstrap(&strata, &diff, resamples, seed ^ (k as u64 + 101))?;
                Some((point, lo, hi))
            });
            let minus_prefixed_sbs = {
                let fixed_order = |i: usize| prefixed_cost(i, prefixed_sbs_index);
                let diff = |sample: &[usize]| -> Option<f64> {
                    Some(mean_over(sample, &router_cost) - mean_over(sample, &fixed_order))
                };
                diff(test).and_then(|point| {
                    bootstrap(&strata, &diff, resamples, seed ^ (k as u64 + 201))
                        .map(|(lo, hi)| (point, lo, hi))
                })
            };
            let test_solved = mean(
                &test
                    .iter()
                    .map(|&i| f64::from(u8::from(table.tasks[i].prefixed[r.choices[i]].correct)))
                    .collect::<Vec<_>>(),
            );
            let test_cost_by_class = classes
                .iter()
                .map(|c| {
                    let rows_c: Vec<usize> = test
                        .iter()
                        .copied()
                        .filter(|&i| &table.tasks[i].class == c)
                        .collect();
                    (c.clone(), mean_over(&rows_c, &router_cost))
                })
                .collect();
            let mut test_choices: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
            for &i in test {
                *test_choices
                    .entry(table.tasks[i].class.clone())
                    .or_default()
                    .entry(table.candidates[r.choices[i]].clone())
                    .or_insert(0) += 1;
            }
            RouterResult {
                name: r.name.clone(),
                learned: r.learned,
                cost,
                test_solved,
                gap_closed,
                gap_closed_ci,
                minus_control,
                test_cost_by_class,
                test_choices,
                minus_prefixed_sbs,
            }
        })
        .collect();

    let selected = results
        .iter()
        .filter(|r| selection.contains(&r.name))
        .min_by(|a, b| a.cost["validation"].total_cmp(&b.cost["validation"]))
        .map(|r| r.name.clone())
        .unwrap_or_default();

    RouteReport {
        prefix: table.prefix.clone(),
        cost_model: *model,
        tasks: rows
            .iter()
            .map(|(s, r)| ((*s).to_owned(), r.len()))
            .collect(),
        sbs: table.fixed[sbs_index].clone(),
        sbs_cost: splits
            .iter()
            .map(|s| {
                (
                    (*s).to_owned(),
                    mean_over(&rows[s], &|i| fixed_cost(i, sbs_index)),
                )
            })
            .collect(),
        vbs_test,
        gap_test,
        oracle_router_test: mean_over(test, &oracle_cost),
        fixed_test: table
            .fixed
            .iter()
            .enumerate()
            .map(|(j, name)| (name.clone(), mean_over(test, &|i| fixed_cost(i, j))))
            .collect(),
        entropy_floor: None,
        routers: results,
        selected,
        ordering_is_primary: false,
        prefixed_sbs: table.candidates.get(prefixed_sbs_index).cloned(),
        prefixed_sbs_cost: splits
            .iter()
            .map(|s| {
                (
                    (*s).to_owned(),
                    mean_over(&rows[s], &|i| prefixed_cost(i, prefixed_sbs_index)),
                )
            })
            .collect(),
    }
}

fn interval(ci: Option<(f64, f64)>) -> String {
    ci.map_or("n/a".to_owned(), |(lo, hi)| format!("{lo:.3} to {hi:.3}"))
}

/// Renders a routing report as Markdown.
#[must_use]
pub fn render_route_markdown(report: &RouteReport) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "## Prefix `{}`\n", report.prefix);
    let _ = writeln!(
        out,
        "Single best fixed strategy, chosen on train: `{}` (train {:.3}, validation {:.3}, test {:.3}). Virtual best on test over the fixed strategies: {:.3}. Gap on test: {:.3}. Best any router with this prefix could do on test: {:.3}.{}\n",
        report.sbs,
        report.sbs_cost["train"],
        report.sbs_cost["validation"],
        report.sbs_cost["test"],
        report.vbs_test,
        report.gap_test,
        report.oracle_router_test,
        report
            .entropy_floor
            .map_or(String::new(), |h| format!(" Entropy floor: {h:.3}."))
    );
    let _ = writeln!(
        out,
        "| Router | Learned | Train | Validation | Test | Solved (test) | Gap closed (test) | 95% CI | Test minus control | 95% CI |"
    );
    let _ = writeln!(out, "|---|---|---:|---:|---:|---:|---:|---:|---:|---:|");
    for r in &report.routers {
        let (diff, diff_ci) = r
            .minus_control
            .map_or(("n/a".to_owned(), "n/a".to_owned()), |(d, lo, hi)| {
                (format!("{d:.3}"), format!("{lo:.3} to {hi:.3}"))
            });
        let _ = writeln!(
            out,
            "| `{}`{} | {} | {:.3} | {:.3} | {:.3} | {:.1}% | {} | {} | {} | {} |",
            r.name,
            if r.name == report.selected {
                " (selected)"
            } else {
                ""
            },
            if r.learned { "yes" } else { "no" },
            r.cost["train"],
            r.cost["validation"],
            r.cost["test"],
            r.test_solved * 100.0,
            r.gap_closed.map_or("n/a".to_owned(), |g| format!("{g:.3}")),
            interval(r.gap_closed_ci),
            diff,
            diff_ci
        );
    }
    if let Some(name) = &report.prefixed_sbs {
        let _ = writeln!(
            out,
            "\n**{}** The best fixed order run after the same prefix, chosen on train, is `{name}` (train {:.3}, test {:.3}). Each router's test cost minus it:\n",
            if report.ordering_is_primary {
                "Pre-registered primary comparison (ADR 0017)."
            } else {
                "Post hoc (ADR 0017, not pre-registered)."
            },
            report
                .prefixed_sbs_cost
                .get("train")
                .copied()
                .unwrap_or(f64::NAN),
            report
                .prefixed_sbs_cost
                .get("test")
                .copied()
                .unwrap_or(f64::NAN),
        );
        let _ = writeln!(
            out,
            "| Router | Test minus best fixed order after the prefix | 95% CI |"
        );
        let _ = writeln!(out, "|---|---:|---:|");
        for r in &report.routers {
            if let Some((d, lo, hi)) = r.minus_prefixed_sbs {
                let _ = writeln!(out, "| `{}` | {d:.3} | {lo:.3} to {hi:.3} |", r.name);
            }
        }
    }
    let _ = writeln!(out, "\n### Test cost by class\n");
    let classes: Vec<&String> = report
        .routers
        .first()
        .map(|r| r.test_cost_by_class.keys().collect())
        .unwrap_or_default();
    let _ = writeln!(
        out,
        "| Router | {} |",
        classes
            .iter()
            .map(|c| c.as_str())
            .collect::<Vec<_>>()
            .join(" | ")
    );
    let _ = writeln!(out, "|---|{}", "---:|".repeat(classes.len()));
    for r in &report.routers {
        let cells: Vec<String> = classes
            .iter()
            .map(|c| format!("{:.2}", r.test_cost_by_class[*c]))
            .collect();
        let _ = writeln!(out, "| `{}` | {} |", r.name, cells.join(" | "));
    }
    let _ = writeln!(out, "\n### Test choices (class: candidate × tasks)\n");
    for r in &report.routers {
        let parts: Vec<String> = r
            .test_choices
            .iter()
            .map(|(class, counts)| {
                let inner: Vec<String> =
                    counts.iter().map(|(c, n)| format!("`{c}` × {n}")).collect();
                format!("{class}: {}", inner.join(", "))
            })
            .collect();
        let _ = writeln!(out, "- `{}`: {}", r.name, parts.join("; "));
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

    fn table() -> RouteTable {
        // Two classes; candidate 0 is right for class a, candidate 1 for b.
        // The fixed strategies are the candidates without the 2-probe prefix.
        let mut tasks = Vec::new();
        for (n, split) in ["train", "validation", "test"].iter().enumerate() {
            for k in 0..(4 + n) {
                for (class, good) in [("a", 0usize), ("b", 1)] {
                    let fixed = (0..2)
                        .map(|c| score(true, if c == good { 10 } else { 30 }))
                        .collect();
                    let prefixed = (0..2)
                        .map(|c| score(true, if c == good { 12 } else { 32 }))
                        .collect();
                    tasks.push(RouteTask {
                        id: format!("{split}-{class}-{k}"),
                        class: class.to_owned(),
                        split: (*split).to_owned(),
                        fixed,
                        prefixed,
                    });
                }
            }
        }
        RouteTable {
            prefix: "p".into(),
            fixed: vec!["s0".into(), "s1".into()],
            candidates: vec!["s0".into(), "s1".into()],
            tasks,
        }
    }

    #[test]
    fn a_perfect_router_closes_most_of_the_gap_and_a_constant_one_none() {
        let t = table();
        let perfect: Vec<usize> = t
            .tasks
            .iter()
            .map(|x| usize::from(x.class == "b"))
            .collect();
        let constant = vec![0; t.tasks.len()];
        let routers = vec![
            RouterChoices {
                name: "control".into(),
                learned: false,
                choices: constant,
            },
            RouterChoices {
                name: "perfect".into(),
                learned: true,
                choices: perfect,
            },
        ];
        let model = CostModel::probes_with_penalty(100.0);
        let r = analyze_routes(
            &t,
            &model,
            &routers,
            "control",
            &["control".to_owned(), "perfect".to_owned()],
            500,
            7,
        );
        assert_eq!(r.sbs, "s0");
        assert!((r.vbs_test - 10.0).abs() < 1e-9);
        assert!((r.gap_test - 10.0).abs() < 1e-9);
        assert!((r.oracle_router_test - 12.0).abs() < 1e-9);
        let control = &r.routers[0];
        let perfect = &r.routers[1];
        assert!(
            (control.gap_closed.unwrap() - (-0.2)).abs() < 1e-9,
            "the prefix costs 2"
        );
        assert!((perfect.gap_closed.unwrap() - 0.8).abs() < 1e-9);
        let (lo, hi) = perfect.gap_closed_ci.unwrap();
        assert!(lo > 0.0 && hi <= 0.8 + 1e-9);
        let (d, lo, hi) = perfect.minus_control.unwrap();
        assert!((d - (-10.0)).abs() < 1e-9 && hi < 0.0 && lo <= d);
        assert_eq!(r.selected, "perfect");
        assert_eq!(perfect.test_choices["a"]["s0"], 6);
        assert!(render_route_markdown(&r).contains("`perfect` (selected)"));
    }
}
