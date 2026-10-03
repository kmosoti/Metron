//! Figures for the repository's front page, rendered as SVG from the
//! committed reports.
//!
//! Every figure is a pure function of files under `experiments/reports/`
//! and of the priors ledger, so the same inputs give byte-identical SVG.
//! `metron figures` writes them to `docs/figures/`; `metron figures --check`
//! and the architecture suite fail when a report changes and its picture
//! does not.

use metron_lab::HeadroomReport;
use metron_lab::headroom::StrategySummary;
use metron_lab::promotion::PromotionReport;
use metron_lab::retrieval::RetrievalReport;
use metron_lab::routing::RouteReport;
use serde::de::DeserializeOwned;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// Committed reports, relative to the repository root.
pub const REPORTS_DIR: &str = "experiments/reports";
/// The priors ledger, relative to the repository root.
pub const PRIORS_LEDGER: &str = "docs/research/priors.md";
/// Where the figures go, relative to the repository root.
pub const FIGURES_DIR: &str = "docs/figures";

/// One rendered figure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Figure {
    /// File name under [`FIGURES_DIR`].
    pub file: &'static str,
    /// The SVG document.
    pub svg: String,
}

/// Renders every figure from the inputs under `root`.
///
/// # Errors
/// A missing, ambiguous or unreadable input, or a report without the
/// columns a figure draws.
pub fn render_figures(root: &Path) -> Result<Vec<Figure>, String> {
    let reports = root.join(REPORTS_DIR);
    let headroom5: HeadroomReport = load(&find_report(&reports, "headroom-arity5")?)?;
    let headroom6: HeadroomReport = load(&find_report(&reports, "headroom-arity6")?)?;
    let selectors5: HeadroomReport = load(&find_report(&reports, "selectors-arity5")?)?;
    let selectors6: HeadroomReport = load(&find_report(&reports, "selectors-arity6")?)?;
    let retrieval: RetrievalReport = load(&find_report(&reports, "retrieval-arity6")?)?;
    let promotion: Vec<PromotionReport> = load(&find_report(&reports, "promotion-arity5")?)?;
    let routing: Vec<RouteReport> =
        load(&find_report_kind(&reports, "structure-arity8", "-route")?)?;
    let ledger_path = root.join(PRIORS_LEDGER);
    let ledger =
        fs::read_to_string(&ledger_path).map_err(|e| format!("{}: {e}", ledger_path.display()))?;
    Ok(vec![
        Figure {
            file: "headroom.svg",
            svg: headroom_figure(&headroom5, &headroom6)?,
        },
        Figure {
            file: "selectors.svg",
            svg: selectors_figure(&selectors5, &selectors6)?,
        },
        Figure {
            file: "retrieval.svg",
            svg: retrieval_figure(&retrieval)?,
        },
        Figure {
            file: "promotion.svg",
            svg: promotion_figure(&promotion)?,
        },
        Figure {
            file: "routing.svg",
            svg: routing_figure(&routing)?,
        },
        Figure {
            file: "priors.svg",
            svg: priors_figure(&ledger)?,
        },
    ])
}

/// Writes every figure under `out`, creating the directory.
///
/// # Errors
/// As [`render_figures`], or a file that cannot be written.
pub fn write_figures(root: &Path, out: &Path) -> Result<Vec<PathBuf>, String> {
    fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut written = Vec::new();
    for figure in render_figures(root)? {
        let path = out.join(figure.file);
        fs::write(&path, figure.svg).map_err(|e| format!("{}: {e}", path.display()))?;
        written.push(path);
    }
    Ok(written)
}

/// The figures under `out` that are missing or differ from what the inputs
/// under `root` render.
///
/// # Errors
/// As [`render_figures`].
pub fn stale_figures(root: &Path, out: &Path) -> Result<Vec<String>, String> {
    let mut stale = Vec::new();
    for figure in render_figures(root)? {
        match fs::read_to_string(out.join(figure.file)) {
            Ok(text) if text == figure.svg => {}
            _ => stale.push(figure.file.to_owned()),
        }
    }
    Ok(stale)
}

fn find_report(dir: &Path, prefix: &str) -> Result<PathBuf, String> {
    find_report_kind(dir, prefix, "")
}

/// A report named `<prefix>-<hash>[-<kind>].json`.
fn find_report_kind(dir: &Path, prefix: &str, kind: &str) -> Result<PathBuf, String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(rest) = name.strip_prefix(prefix)
            && let Some(rest) = rest.strip_prefix('-')
            && let Some(rest) = rest.strip_suffix(".json")
            && let Some(hash) = rest.strip_suffix(kind)
            && hash.len() == 12
            && hash.chars().all(|c| c.is_ascii_hexdigit())
        {
            found.push(entry.path());
        }
    }
    found.sort();
    match found.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(format!(
            "no report named {prefix}-<hash>{kind}.json in {}",
            dir.display()
        )),
        _ => Err(format!(
            "more than one report named {prefix}-<hash>{kind}.json: {found:?}"
        )),
    }
}

fn load<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// A very small SVG writer.

const FONT: &str = "-apple-system, BlinkMacSystemFont, 'Segoe UI', Helvetica, Arial, sans-serif";
const INK: &str = "#24292f";
const MUTED: &str = "#57606a";
const GRID: &str = "#d8dee4";
const BLUE: &str = "#0072B2";
const ORANGE: &str = "#E69F00";
const GREEN: &str = "#009E73";
const RED: &str = "#D55E00";
const SKY: &str = "#56B4E9";
const PURPLE: &str = "#CC79A7";
const GREY: &str = "#8c959f";
const WIDTH: f64 = 760.0;

#[derive(Clone, Copy)]
enum Anchor {
    Start,
    Middle,
    End,
}

#[derive(Clone, Copy)]
struct Style<'a> {
    size: f64,
    fill: &'a str,
    anchor: Anchor,
    bold: bool,
}

impl<'a> Style<'a> {
    fn new(size: f64, fill: &'a str) -> Self {
        Self {
            size,
            fill,
            anchor: Anchor::Start,
            bold: false,
        }
    }

    fn middle(mut self) -> Self {
        self.anchor = Anchor::Middle;
        self
    }

    fn end(mut self) -> Self {
        self.anchor = Anchor::End;
        self
    }

    fn bold(mut self) -> Self {
        self.bold = true;
        self
    }
}

struct Svg {
    width: f64,
    height: f64,
    title: String,
    body: String,
}

/// Coordinates with at most one decimal, no trailing `.0`.
fn num(v: f64) -> String {
    let r = (v * 10.0).round() / 10.0;
    if r.fract().abs() < 1e-9 {
        format!("{}", r as i64)
    } else {
        format!("{r:.1}")
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Rough advance width of `s` at `size` pixels in a sans-serif face.
fn text_width(s: &str, size: f64) -> f64 {
    s.chars().count() as f64 * size * 0.56
}

fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl Svg {
    fn new(height: f64, title: &str, subtitle: &str) -> Self {
        let mut svg = Self {
            width: WIDTH,
            height,
            title: title.to_owned(),
            body: String::new(),
        };
        svg.text((24.0, 36.0), Style::new(18.0, INK).bold(), title);
        svg.text((24.0, 58.0), Style::new(12.5, MUTED), subtitle);
        svg
    }

    fn rect(&mut self, at: (f64, f64), size: (f64, f64), fill: &str) {
        let _ = writeln!(
            self.body,
            r##"  <rect x="{}" y="{}" width="{}" height="{}" rx="2" fill="{fill}"/>"##,
            num(at.0),
            num(at.1),
            num(size.0.max(0.0)),
            num(size.1),
        );
    }

    fn line(&mut self, from: (f64, f64), to: (f64, f64), stroke: &str, width: f64, dashed: bool) {
        let dash = if dashed {
            r##" stroke-dasharray="6 4""##
        } else {
            ""
        };
        let _ = writeln!(
            self.body,
            r##"  <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{stroke}" stroke-width="{}"{dash}/>"##,
            num(from.0),
            num(from.1),
            num(to.0),
            num(to.1),
            num(width),
        );
    }

    fn polyline(&mut self, points: &[(f64, f64)], stroke: &str, width: f64) {
        let pts: Vec<String> = points
            .iter()
            .map(|(x, y)| format!("{},{}", num(*x), num(*y)))
            .collect();
        let _ = writeln!(
            self.body,
            r##"  <polyline points="{}" fill="none" stroke="{stroke}" stroke-width="{}" stroke-linejoin="round" stroke-linecap="round"/>"##,
            pts.join(" "),
            num(width),
        );
    }

    fn circle(&mut self, at: (f64, f64), r: f64, fill: &str) {
        let _ = writeln!(
            self.body,
            r##"  <circle cx="{}" cy="{}" r="{}" fill="{fill}" stroke="#ffffff" stroke-width="1.5"/>"##,
            num(at.0),
            num(at.1),
            num(r),
        );
    }

    fn cross(&mut self, at: (f64, f64), r: f64, stroke: &str) {
        self.line(
            (at.0 - r, at.1 - r),
            (at.0 + r, at.1 + r),
            stroke,
            2.0,
            false,
        );
        self.line(
            (at.0 - r, at.1 + r),
            (at.0 + r, at.1 - r),
            stroke,
            2.0,
            false,
        );
    }

    fn text(&mut self, at: (f64, f64), style: Style<'_>, content: &str) {
        let anchor = match style.anchor {
            Anchor::Start => "",
            Anchor::Middle => r##" text-anchor="middle""##,
            Anchor::End => r##" text-anchor="end""##,
        };
        let weight = if style.bold {
            r##" font-weight="600""##
        } else {
            ""
        };
        let _ = writeln!(
            self.body,
            r##"  <text x="{}" y="{}" font-size="{}" fill="{}"{anchor}{weight}>{}</text>"##,
            num(at.0),
            num(at.1),
            num(style.size),
            style.fill,
            escape(content),
        );
    }

    /// Colour swatches with labels, laid out leftwards from `right`.
    fn legend(&mut self, items: &[(&str, &str)], right: f64, y: f64) {
        let mut x = right;
        for (color, label) in items.iter().rev() {
            x -= text_width(label, 12.0);
            self.text((x, y + 4.0), Style::new(12.0, INK), label);
            x -= 18.0;
            self.rect((x, y - 6.0), (12.0, 12.0), color);
            x -= 18.0;
        }
    }

    fn finish(self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" font-family="{FONT}" role="img">"##,
            w = num(self.width),
            h = num(self.height),
        );
        let _ = writeln!(out, "  <title>{}</title>", escape(&self.title));
        let _ = writeln!(
            out,
            r##"  <rect x="0.5" y="0.5" width="{}" height="{}" rx="10" fill="#ffffff" stroke="#d0d7de"/>"##,
            num(self.width - 1.0),
            num(self.height - 1.0),
        );
        out.push_str(&self.body);
        out.push_str("</svg>\n");
        out
    }
}

fn strategy<'a>(r: &'a HeadroomReport, name: &str) -> Result<&'a StrategySummary, String> {
    r.strategies
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| format!("strategy {name} is missing from the report"))
}

// ---------------------------------------------------------------------------
// Headroom: the cost ladder.

fn headroom_figure(a5: &HeadroomReport, a6: &HeadroomReport) -> Result<String, String> {
    let rows = [
        (
            "Probe every row",
            "the exhaustive truth table".to_owned(),
            "exhaustive",
        ),
        (
            "Affine shortcut, verified, then greedy",
            "guess a structure, check it, fall back".to_owned(),
            "affine-verified-then-greedy",
        ),
        (
            "Greedy search over the whole pool",
            format!("single best fixed strategy ({})", a5.sbs),
            "sbs",
        ),
        (
            "A perfect per-task router",
            "virtual best: told each target's family".to_owned(),
            "vbs",
        ),
    ];
    let cost = |r: &HeadroomReport, which: &str| -> Result<f64, String> {
        match which {
            "sbs" => Ok(r.sbs_cost),
            "vbs" => Ok(r.vbs_cost),
            name => strategy(r, name).map(|s| s.mean_cost),
        }
    };
    let (left, right, top, row_h) = (310.0, 716.0, 100.0, 48.0);
    let bottom = top + row_h * rows.len() as f64;
    let families = a5.families.len();
    let floors: Vec<(f64, &str)> = [(a5, BLUE), (a6, ORANGE)]
        .into_iter()
        .filter_map(|(r, c)| r.entropy_floor.map(|h| (h, c)))
        .collect();
    let mut svg = Svg::new(
        bottom + if floors.is_empty() { 86.0 } else { 106.0 },
        "What would a perfect router save?",
        &format!(
            "Mean cost per task: {} tasks from {families} structural families, ten task-set seeds",
            a5.tasks
        ),
    );
    svg.legend(&[(BLUE, "arity 5"), (ORANGE, "arity 6")], right, 84.0);
    let (lo, hi) = (2.0_f64, 7.0_f64);
    let x = |v: f64| left + (v.log2() - lo) / (hi - lo) * (right - left);
    for tick in [4_u32, 8, 16, 32, 64, 128] {
        let tx = x(f64::from(tick));
        svg.line((tx, top - 4.0), (tx, bottom), GRID, 1.0, false);
        svg.text(
            (tx, bottom + 16.0),
            Style::new(11.0, MUTED).middle(),
            &tick.to_string(),
        );
    }
    for (i, (label, sub, which)) in rows.iter().enumerate() {
        let y0 = top + row_h * i as f64;
        svg.text(
            (left - 14.0, y0 + 19.0),
            Style::new(13.0, INK).end().bold(),
            label,
        );
        svg.text((left - 14.0, y0 + 35.0), Style::new(11.0, MUTED).end(), sub);
        for (j, (report, color)) in [(a5, BLUE), (a6, ORANGE)].into_iter().enumerate() {
            let v = cost(report, which)?;
            let by = y0 + 8.0 + 17.0 * j as f64;
            svg.rect((left, by), (x(v) - left, 14.0), color);
            svg.text(
                (x(v) + 6.0, by + 11.0),
                Style::new(11.0, INK),
                &format!("{v:.2}"),
            );
        }
    }
    let same_floor =
        floors.len() == 2 && format!("{:.2}", floors[0].0) == format!("{:.2}", floors[1].0);
    for (i, &(h, color)) in floors.iter().enumerate() {
        let fx = x(h);
        let stroke = if same_floor { INK } else { color };
        svg.line((fx, top - 8.0), (fx, bottom), stroke, 1.5, true);
        if i == 0 || !same_floor {
            svg.text(
                (fx + 5.0, top - 8.0 + 14.0 * i as f64),
                Style::new(11.0, stroke).bold(),
                &format!("entropy floor {h:.2}"),
            );
        }
    }
    svg.text(
        ((left + right) / 2.0, bottom + 36.0),
        Style::new(11.5, MUTED).middle(),
        "probes per task, log scale; a wrong or missing answer costs twice the probe cap",
    );
    svg.text(
        (24.0, bottom + 66.0),
        Style::new(12.5, INK),
        &format!(
            "The perfect router would save {:.2} probes (arity 5) and {:.2} (arity 6): about log2 {families} = {:.2} bits, the family label.",
            a5.gap,
            a6.gap,
            (families as f64).log2()
        ),
    );
    if let (Some(b5), Some(b6)) = (a5.realisable_gap_bound(), a6.realisable_gap_bound()) {
        svg.text(
            (24.0, bottom + 86.0),
            Style::new(12.5, INK),
            &format!(
                "It sits below the entropy floor. A router that is not told the family can save at most {b5:.2} and {b6:.2}."
            ),
        );
    }
    Ok(svg.finish())
}

// ---------------------------------------------------------------------------
// Selectors: the routing control.

struct SweepPoint {
    k: u32,
    solved: f64,
    probes: f64,
}

fn sweep(r: &HeadroomReport, prefix: &str) -> Vec<SweepPoint> {
    let mut points: Vec<SweepPoint> = r
        .strategies
        .iter()
        .filter_map(|s| {
            let k = s.name.strip_prefix(prefix)?.parse().ok()?;
            Some(SweepPoint {
                k,
                solved: s.solved,
                probes: s.mean_probes_when_solved,
            })
        })
        .collect();
    points.sort_by_key(|p| p.k);
    points
}

fn selectors_figure(a5: &HeadroomReport, a6: &HeadroomReport) -> Result<String, String> {
    let most5 = sweep(a5, "select-most-survivors-k");
    let most6 = sweep(a6, "select-most-survivors-k");
    let fewest5 = sweep(a5, "select-fewest-survivors-k");
    let fewest6 = sweep(a6, "select-fewest-survivors-k");
    let (Some(last5), Some(last6)) = (most5.last(), most6.last()) else {
        return Err("the selector reports have no most-survivors sweep".to_owned());
    };
    let k_max = last5.k.max(last6.k).max(1);
    let (left, right, top, bottom) = (84.0, 500.0, 104.0, 300.0);
    let mut svg = Svg::new(
        386.0,
        "Can a router learn the family from a few probes?",
        "Spend k greedy probes, commit to the family with the most surviving hypotheses, search only that family",
    );
    svg.legend(&[(BLUE, "arity 5"), (ORANGE, "arity 6")], 736.0, 84.0);
    let x = |k: u32| left + f64::from(k) / f64::from(k_max) * (right - left);
    let y = |p: f64| bottom - p * (bottom - top);
    for pct in [0_u32, 25, 50, 75, 100] {
        let ty = y(f64::from(pct) / 100.0);
        svg.line((left, ty), (right, ty), GRID, 1.0, false);
        svg.text(
            (left - 10.0, ty + 4.0),
            Style::new(11.0, MUTED).end(),
            &format!("{pct}%"),
        );
    }
    for k in 0..=k_max {
        svg.text(
            (x(k), bottom + 18.0),
            Style::new(11.0, MUTED).middle(),
            &k.to_string(),
        );
    }
    svg.text(
        ((left + right) / 2.0, bottom + 40.0),
        Style::new(11.5, MUTED).middle(),
        "greedy probes spent before committing to a family (k)",
    );
    svg.text((left, top - 14.0), Style::new(11.5, MUTED), "tasks solved");

    let sbs = strategy(a5, &a5.sbs)?;
    let sbs6 = strategy(a6, &a6.sbs)?;
    svg.line(
        (left, y(sbs.solved)),
        (right, y(sbs.solved)),
        GREEN,
        2.0,
        true,
    );
    svg.text(
        (right + 14.0, y(sbs.solved) + 4.0),
        Style::new(12.0, GREEN).bold(),
        &format!("single best ({})", a5.sbs),
    );
    let (p5, p6) = (
        format!("{:.1}", sbs.mean_probes_when_solved),
        format!("{:.1}", sbs6.mean_probes_when_solved),
    );
    let sbs_note = if p5 == p6 {
        format!("solves {:.0}% at {p5} probes", sbs.solved * 100.0)
    } else {
        format!("solves {:.0}% at {p5} and {p6} probes", sbs.solved * 100.0)
    };
    svg.text(
        (right + 14.0, y(sbs.solved) + 20.0),
        Style::new(11.5, MUTED),
        &sbs_note,
    );

    for (points, color) in [(&most5, BLUE), (&most6, ORANGE)] {
        let pts: Vec<(f64, f64)> = points.iter().map(|p| (x(p.k), y(p.solved))).collect();
        svg.polyline(&pts, color, 2.5);
        for &pt in &pts {
            svg.circle(pt, 4.5, color);
        }
    }
    let (mut ly5, mut ly6) = (y(last5.solved) + 4.0, y(last6.solved) + 4.0);
    if (ly5 - ly6).abs() < 15.0 {
        let mid = f64::midpoint(ly5, ly6);
        let up = if ly5 <= ly6 { -7.5 } else { 7.5 };
        ly5 = mid + up;
        ly6 = mid - up;
    }
    svg.text(
        (x(last5.k) + 10.0, ly5),
        Style::new(11.5, BLUE).bold(),
        &format!("{:.0}%", last5.solved * 100.0),
    );
    svg.text(
        (x(last6.k) + 10.0, ly6),
        Style::new(11.5, ORANGE).bold(),
        &format!("{:.0}%", last6.solved * 100.0),
    );

    let mut control_label = None;
    for (points, color) in [(&fewest5, BLUE), (&fewest6, ORANGE)] {
        for p in points.iter().filter(|p| p.k > 0) {
            svg.cross((x(p.k), y(p.solved)), 4.5, color);
            control_label = Some((x(p.k), y(p.solved)));
        }
    }
    if let Some((cx, cy)) = control_label {
        svg.text(
            (cx + 10.0, cy - 8.0),
            Style::new(11.0, MUTED),
            "fewest-survivors control",
        );
    }

    let note_x = right + 14.0;
    let lines = [
        format!("At k = {} a solved task costs", last5.k),
        format!(
            "{:.1} probes (arity 5) and {:.1} (arity 6).",
            last5.probes, last6.probes
        ),
        "More prefix only walks the router".to_owned(),
        "back to the single best strategy:".to_owned(),
        "the family label is not in the probes.".to_owned(),
    ];
    for (i, line) in lines.iter().enumerate() {
        svg.text(
            (note_x, y(0.30) + 18.0 * i as f64),
            Style::new(12.0, INK),
            line,
        );
    }
    Ok(svg.finish())
}

// ---------------------------------------------------------------------------
// Retrieval: recall per byte.

struct MethodPoint {
    name: String,
    bytes: f64,
    recalls: Vec<f64>,
    nanos: Vec<f64>,
}

fn method_label(name: &str) -> String {
    if let Some(inner) = name
        .strip_prefix("hdc-bundle(")
        .and_then(|s| s.strip_suffix(" bits)"))
    {
        let bits: usize = inner.parse().unwrap_or(0);
        return format!("HDC, {} bits", grouped(bits));
    }
    if let Some(inner) = name
        .strip_prefix("bloom-per-table(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let bits = inner.split(',').next().unwrap_or(inner);
        return format!("Bloom, {bits}");
    }
    name.replace('-', " ")
}

fn duration_label(nanos: f64) -> String {
    if nanos >= 1e6 {
        format!("{:.1} ms", nanos / 1e6)
    } else {
        format!("{:.0} µs", nanos / 1e3)
    }
}

fn retrieval_figure(r: &RetrievalReport) -> Result<String, String> {
    let mut methods: Vec<MethodPoint> = Vec::new();
    for row in &r.rows_measured {
        if let Some(m) = methods.iter_mut().find(|m| m.name == row.method) {
            m.recalls.push(row.recall_at_answer);
            m.nanos.push(row.mean_query_nanos);
        } else {
            methods.push(MethodPoint {
                name: row.method.clone(),
                bytes: row.bytes_per_entry,
                recalls: vec![row.recall_at_answer],
                nanos: vec![row.mean_query_nanos],
            });
        }
    }
    if methods.is_empty() {
        return Err("the retrieval report has no rows".to_owned());
    }
    let (left, right, top, bottom) = (84.0, 716.0, 104.0, 304.0);
    let mut svg = Svg::new(
        396.0,
        "Retrieval: recall per byte",
        &format!(
            "Find every stored function consistent with a few observed rows; {} six-input functions in the store",
            grouped(r.store_size)
        ),
    );
    let (lo, hi) = (2.0_f64, 11.0_f64);
    let x = |bytes: f64| left + (bytes.log2() - lo) / (hi - lo) * (right - left);
    let (r_lo, r_hi) = (0.4_f64, 1.0_f64);
    let y = |recall: f64| bottom - (recall - r_lo) / (r_hi - r_lo) * (bottom - top);
    for pct in [40_u32, 50, 60, 70, 80, 90, 100] {
        let ty = y(f64::from(pct) / 100.0);
        svg.line((left, ty), (right, ty), GRID, 1.0, false);
        svg.text(
            (left - 10.0, ty + 4.0),
            Style::new(11.0, MUTED).end(),
            &format!("{pct}%"),
        );
    }
    for p in 3..=10 {
        let bytes = f64::from(1_u32 << p);
        svg.text(
            (x(bytes), bottom + 18.0),
            Style::new(11.0, MUTED).middle(),
            &format!("{} B", 1_u32 << p),
        );
    }
    svg.text(
        ((left + right) / 2.0, bottom + 40.0),
        Style::new(11.5, MUTED).middle(),
        "index bytes per stored function, log scale (the raw table is 8 bytes)",
    );
    svg.text(
        (24.0, 76.0),
        Style::new(12.5, MUTED),
        "Recall of that set; dot: mean over 4 to 32 observed rows, bar: range; HDC: hyperdimensional codes",
    );
    for m in &methods {
        let color = if m.name.starts_with("bitmask") {
            GREEN
        } else if m.name.starts_with("bloom") {
            ORANGE
        } else if m.name.starts_with("hdc") {
            PURPLE
        } else {
            GREY
        };
        let mean = m.recalls.iter().sum::<f64>() / m.recalls.len() as f64;
        let lo_r = m.recalls.iter().copied().fold(f64::INFINITY, f64::min);
        let hi_r = m.recalls.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let nanos = m.nanos.iter().sum::<f64>() / m.nanos.len() as f64;
        let px = x(m.bytes);
        svg.line((px, y(lo_r)), (px, y(hi_r)), color, 3.0, false);
        svg.circle((px, y(mean)), 6.0, color);
        let label = format!("{} · {}", method_label(&m.name), duration_label(nanos));
        let below = y(mean) + 22.0;
        let (at, style) = if m.name.starts_with("bitmask") {
            ((px + 10.0, y(mean) - 10.0), Style::new(12.0, color).bold())
        } else if m.name.starts_with("exact") {
            ((px + 10.0, below), Style::new(11.5, INK))
        } else if m.name.starts_with("bloom") {
            ((px - 10.0, below), Style::new(11.5, INK).end())
        } else if px > 600.0 {
            ((px - 12.0, y(mean) - 8.0), Style::new(11.5, INK).end())
        } else if m.bytes >= 256.0 {
            ((px + 10.0, below), Style::new(11.5, INK))
        } else {
            ((px + 12.0, y(mean) + 4.0), Style::new(11.5, INK))
        };
        svg.text(at, style, &label);
    }
    let mean_recall = |m: &MethodPoint| m.recalls.iter().sum::<f64>() / m.recalls.len() as f64;
    let bitmask = methods.iter().find(|m| m.name.starts_with("bitmask"));
    let best_hdc = methods
        .iter()
        .filter(|m| m.name.starts_with("hdc"))
        .max_by(|a, b| mean_recall(a).total_cmp(&mean_recall(b)));
    if let (Some(b), Some(h)) = (bitmask, best_hdc) {
        svg.text(
            (24.0, bottom + 72.0),
            Style::new(12.5, INK),
            &format!(
                "The bitmask index finds {:.0}% at {} bytes; the best hyperdimensional code uses {:.0} times the bytes and misses {:.1}%.",
                mean_recall(b) * 100.0,
                num(b.bytes),
                h.bytes / b.bytes,
                (1.0 - mean_recall(h)) * 100.0
            ),
        );
    }
    Ok(svg.finish())
}

// ---------------------------------------------------------------------------
// Promotion: candidates judged on held-out targets.

fn wrap_composition(ops: &[String], width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for (i, op) in ops.iter().enumerate() {
        let piece = if i + 1 < ops.len() {
            format!("{op} →")
        } else {
            op.clone()
        };
        if !current.is_empty() && current.chars().count() + 1 + piece.chars().count() > width {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(&piece);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn promotion_figure(reports: &[PromotionReport]) -> Result<String, String> {
    let cases: Vec<(&str, &metron_lab::promotion::PromotionCase)> = reports
        .iter()
        .flat_map(|r| r.cases.iter().map(move |c| (r.strategy.as_str(), c)))
        .collect();
    if cases.is_empty() {
        return Err("the promotion report has no candidates".to_owned());
    }
    let block = 64.0;
    let top = 112.0;
    let bottom = top + block * cases.len() as f64;
    let mut svg = Svg::new(
        bottom + 44.0,
        "Candidates are judged, not trusted",
        "The system proposes a composition from train-split episodes; the laboratory runs it on held-out targets",
    );
    svg.legend(
        &[
            (GREEN, "promoted"),
            (RED, "rejected"),
            (GREY, "the strategy that proposed it"),
        ],
        736.0,
        84.0,
    );
    let (bar_left, bar_w) = (420.0, 200.0);
    for (i, (strategy_name, case)) in cases.iter().enumerate() {
        let y0 = top + block * i as f64;
        if i > 0 {
            svg.line((24.0, y0), (736.0, y0), GRID, 1.0, false);
        }
        svg.text(
            (24.0, y0 + 20.0),
            Style::new(12.5, INK).bold(),
            &format!("from {strategy_name}"),
        );
        let ops: Vec<String> = case
            .candidate
            .composition
            .iter()
            .map(|op| op.as_str().to_owned())
            .collect();
        for (j, line) in wrap_composition(&ops, 60).iter().enumerate() {
            svg.text(
                (24.0, y0 + 37.0 + 14.0 * j as f64),
                Style::new(11.0, MUTED),
                line,
            );
        }
        let promoted = case.decision.is_promoted();
        let color = if promoted { GREEN } else { RED };
        let held = case.held_out.solved;
        let reference = case.reference.solved;
        svg.rect((bar_left, y0 + 12.0), (bar_w * held, 16.0), color);
        svg.text(
            (bar_left + bar_w * held + 6.0, y0 + 25.0),
            Style::new(12.0, INK).bold(),
            &format!("{:.0}%", held * 100.0),
        );
        svg.rect((bar_left, y0 + 34.0), (bar_w * reference, 8.0), GREY);
        svg.text(
            (bar_left + bar_w * reference + 6.0, y0 + 42.0),
            Style::new(11.0, MUTED),
            &format!("{:.0}%", reference * 100.0),
        );
        svg.text(
            (736.0, y0 + 25.0),
            Style::new(12.0, color).end().bold(),
            if promoted { "promoted" } else { "rejected" },
        );
    }
    svg.text(
        (bar_left, top - 4.0),
        Style::new(11.0, MUTED),
        "held-out tasks solved",
    );
    let promoted = cases
        .iter()
        .filter(|(_, c)| c.decision.is_promoted())
        .count();
    let beats_parent = cases
        .iter()
        .filter(|(_, c)| c.decision.is_promoted() && c.held_out.solved > c.reference.solved)
        .count();
    svg.text(
        (24.0, bottom + 26.0),
        Style::new(12.5, INK),
        &format!(
            "{promoted} of {} candidates promoted; {beats_parent} solves more held-out targets than the strategy it came from.",
            cases.len()
        ),
    );
    Ok(svg.finish())
}

// ---------------------------------------------------------------------------
// Routing: the structure-keyed lab's test split.

fn routing_figure(reports: &[RouteReport]) -> Result<String, String> {
    let r = reports
        .first()
        .ok_or("the routing report has no prefixes")?;
    let router = |name: &str| r.routers.iter().find(|x| x.name.starts_with(name));
    let gap_note = |x: &metron_lab::routing::RouterResult| -> String {
        match (x.gap_closed, x.gap_closed_ci) {
            (Some(g), Some((lo, hi))) => {
                format!("closes {g:.2} of the gap (95% CI {lo:.2} to {hi:.2})")
            }
            (Some(g), None) => format!("closes {g:.2} of the gap"),
            _ => String::new(),
        }
    };
    struct Row {
        label: String,
        sub: String,
        value: f64,
        color: &'static str,
        note: String,
    }
    let mut rows = vec![Row {
        label: "Single best cascade".into(),
        sub: format!(
            "{}, chosen on train",
            r.sbs.trim_start_matches("cascade-").replace('-', " → ")
        ),
        value: r.sbs_cost["test"],
        color: GREY,
        note: String::new(),
    }];
    for (prefix, label, sub, color) in [
        (
            "posterior-order",
            "Bayes ranking, hand-authored",
            "try classes in posterior order",
            BLUE,
        ),
        (
            "tabular",
            "Tabular router, learned",
            "cheapest cascade per posterior bucket",
            ORANGE,
        ),
        (
            "ridge",
            "Ridge router, learned",
            "lowest predicted cost",
            ORANGE,
        ),
        (
            "posterior-reverse",
            "Reverse ranking",
            "negative control",
            RED,
        ),
    ] {
        if let Some(x) = router(prefix) {
            let selected = if x.name == r.selected {
                "; selected on validation"
            } else {
                ""
            };
            rows.push(Row {
                label: label.into(),
                sub: format!("{sub}{selected}"),
                value: x.cost["test"],
                color,
                note: gap_note(x),
            });
        }
    }
    rows.push(Row {
        label: "Best router possible".into(),
        sub: "per-task best cascade after the anchors".into(),
        value: r.oracle_router_test,
        color: SKY,
        note: String::new(),
    });
    rows.push(Row {
        label: "Virtual best".into(),
        sub: "told each target's class".into(),
        value: r.vbs_test,
        color: SKY,
        note: String::new(),
    });
    let (left, right, top, row_h) = (290.0, 716.0, 100.0, 44.0);
    let bottom = top + row_h * rows.len() as f64;
    let mut svg = Svg::new(
        bottom + 78.0,
        "Routing among structural learners",
        &format!(
            "Structure-keyed lab at arity 8 (ADR 0015): mean cost on the {} test tasks; routers first spend nine anchor probes",
            r.tasks["test"]
        ),
    );
    let max = rows
        .iter()
        .map(|row| row.value)
        .fold(0.0, f64::max)
        .max(1.0);
    let scale = (max * 1.15 / 10.0).ceil() * 10.0;
    let x = |v: f64| left + v / scale * (right - left);
    let step = if scale > 60.0 { 20.0 } else { 10.0 };
    let mut tick = 0.0;
    while tick <= scale + 1e-9 {
        svg.line((x(tick), top - 4.0), (x(tick), bottom), GRID, 1.0, false);
        svg.text(
            (x(tick), bottom + 16.0),
            Style::new(11.0, MUTED).middle(),
            &num(tick),
        );
        tick += step;
    }
    for (i, row) in rows.iter().enumerate() {
        let y0 = top + row_h * i as f64;
        svg.text(
            (left - 14.0, y0 + 17.0),
            Style::new(12.5, INK).end().bold(),
            &row.label,
        );
        svg.text(
            (left - 14.0, y0 + 32.0),
            Style::new(11.0, MUTED).end(),
            &row.sub,
        );
        svg.rect((left, y0 + 8.0), (x(row.value) - left, 16.0), row.color);
        svg.text(
            (x(row.value) + 6.0, y0 + 20.5),
            Style::new(11.5, INK).bold(),
            &format!("{:.1}", row.value),
        );
        if !row.note.is_empty() {
            svg.text((left + 2.0, y0 + 38.0), Style::new(10.5, MUTED), &row.note);
        }
    }
    if let Some(h) = r.entropy_floor {
        let fx = x(h);
        svg.line((fx, top - 8.0), (fx, bottom), INK, 1.5, true);
        svg.text(
            (fx + 5.0, top - 8.0),
            Style::new(11.0, INK).bold(),
            &format!("entropy floor {h:.1}"),
        );
    }
    svg.text(
        ((left + right) / 2.0, bottom + 36.0),
        Style::new(11.5, MUTED).middle(),
        "probes per task; a wrong answer costs twice the probe cap",
    );
    if let Some(control) = router("posterior-order") {
        let paying: Vec<&str> = r
            .routers
            .iter()
            .filter(|x| x.name != "posterior-reverse")
            .filter(|x| x.gap_closed_ci.is_some_and(|(lo, _)| lo > 0.0))
            .map(|x| x.name.as_str())
            .collect();
        let failures = ((1.0 - control.test_solved) * r.tasks["test"] as f64).round();
        let caption = if paying.is_empty() {
            format!(
                "No router's interval clears zero. {failures:.0} wrong answers at {:.0} each add {:.1} to the Bayes ranking's mean.",
                r.cost_model.failure_cost,
                failures * r.cost_model.failure_cost / r.tasks["test"].max(1) as f64
            )
        } else {
            format!(
                "Routing pays: {} closes the gap with an interval that clears zero, on tasks no router saw in training.",
                paying.join(", ")
            )
        };
        svg.text((24.0, bottom + 62.0), Style::new(12.5, INK), &caption);
    }
    Ok(svg.finish())
}

// ---------------------------------------------------------------------------
// Priors: the ledger as a picture.

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Validated,
    Refined,
    Falsified,
    Untested,
}

impl Status {
    fn parse(text: &str) -> Option<Self> {
        let t = text.trim().to_lowercase();
        [
            ("validated", Status::Validated),
            ("refined", Status::Refined),
            ("falsified", Status::Falsified),
            ("untested", Status::Untested),
        ]
        .into_iter()
        .find(|(word, _)| t.starts_with(word))
        .map(|(_, s)| s)
    }
}

/// Splits a Markdown table row into cells, honouring `\|` escapes.
fn table_cells(row: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut current = String::new();
    let mut chars = row.trim().trim_start_matches('|').chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                current.push('|');
                chars.next();
            }
            '|' => cells.push(std::mem::take(&mut current).trim().to_owned()),
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        cells.push(current.trim().to_owned());
    }
    cells
}

/// `(entry number, status)` for every row of the ledger table.
fn ledger_entries(ledger: &str) -> Vec<(u32, Status)> {
    let mut entries = Vec::new();
    let mut in_ledger = false;
    for line in ledger.lines() {
        if line.starts_with("## ") {
            in_ledger = line.trim() == "## Ledger";
            continue;
        }
        if !in_ledger || !line.starts_with('|') {
            continue;
        }
        let cells = table_cells(line);
        if let (Some(first), Some(last)) = (cells.first(), cells.last())
            && let Ok(n) = first.parse::<u32>()
            && let Some(status) = Status::parse(last)
        {
            entries.push((n, status));
        }
    }
    entries
}

fn priors_figure(ledger: &str) -> Result<String, String> {
    let entries = ledger_entries(ledger);
    if entries.is_empty() {
        return Err("no ledger rows found in the priors ledger".to_owned());
    }
    let groups = [
        (Status::Validated, "validated", GREEN),
        (Status::Refined, "refined", SKY),
        (Status::Falsified, "falsified", RED),
        (Status::Untested, "untested", GREY),
    ];
    let per_row = 14_usize;
    let cell = 34.0;
    let gap = 6.0;
    let mut layout = Vec::new();
    let mut y = 96.0;
    for (status, label, color) in groups {
        let numbers: Vec<u32> = entries
            .iter()
            .filter(|(_, s)| *s == status)
            .map(|(n, _)| *n)
            .collect();
        let lines = numbers.len().div_ceil(per_row).max(1);
        layout.push((y, label, color, numbers));
        y += lines as f64 * (cell + gap) + 12.0;
    }
    let mut svg = Svg::new(
        y + 40.0,
        "Every prior is a hypothesis",
        &format!(
            "{} priors the design relies on, each with a test or a primary source (docs/research/priors.md)",
            entries.len()
        ),
    );
    for (y0, label, color, numbers) in layout {
        svg.text((24.0, y0 + 16.0), Style::new(13.0, color).bold(), label);
        svg.text(
            (24.0, y0 + 32.0),
            Style::new(11.0, MUTED),
            &format!(
                "{} {}",
                numbers.len(),
                if numbers.len() == 1 {
                    "prior"
                } else {
                    "priors"
                }
            ),
        );
        if numbers.is_empty() {
            svg.text((150.0, y0 + 22.0), Style::new(12.0, MUTED), "none");
        }
        for (i, n) in numbers.iter().enumerate() {
            let cx = 150.0 + (i % per_row) as f64 * (cell + gap);
            let cy = y0 + (i / per_row) as f64 * (cell + gap);
            svg.rect((cx, cy), (cell, cell), color);
            svg.text(
                (cx + cell / 2.0, cy + cell / 2.0 + 4.5),
                Style::new(12.5, "#ffffff").middle().bold(),
                &n.to_string(),
            );
        }
    }
    svg.text(
        (24.0, y + 18.0),
        Style::new(12.5, INK),
        "Priors about established mathematics held; priors about the design's own glue failed most often.",
    );
    Ok(svg.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_text_are_formatted_for_svg() {
        assert_eq!(num(3.0), "3");
        assert_eq!(num(3.04), "3");
        assert_eq!(num(3.06), "3.1");
        assert_eq!(escape("a < b & c"), "a &lt; b &amp; c");
        assert_eq!(grouped(5126), "5,126");
        assert_eq!(grouped(512), "512");
        assert_eq!(grouped(1_000_000), "1,000,000");
    }

    #[test]
    fn ledger_rows_are_parsed_with_escaped_pipes() {
        let ledger = "# Priors\n\n## Ledger\n\n| # | Prior | Status |\n|---|---|---|\n\
| 1 | sizes \\|C\\| | validated |\n| 2 | b | falsified, then fixed |\n| 3 | c | refined: more |\n\n\
## Other\n\n| 4 | d | untested |\n";
        let entries = ledger_entries(ledger);
        let numbers: Vec<u32> = entries.iter().map(|(n, _)| *n).collect();
        assert_eq!(numbers, vec![1, 2, 3]);
        assert!(entries[0].1 == Status::Validated);
        assert!(entries[1].1 == Status::Falsified);
        assert!(entries[2].1 == Status::Refined);
        assert_eq!(table_cells("| a \\| b | c |"), vec!["a | b", "c"]);
    }

    #[test]
    fn compositions_wrap_at_arrows() {
        let ops: Vec<String> = ["greedy-split-probe", "version-space-filter", "commit"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        assert_eq!(
            wrap_composition(&ops, 40),
            vec!["greedy-split-probe →", "version-space-filter → commit"]
        );
    }
}
