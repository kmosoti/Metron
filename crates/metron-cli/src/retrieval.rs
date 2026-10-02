//! The retrieval workload command.

use crate::compose::load_manifest;
use metron_adapters::write_json_pretty;
use metron_lab::retrieval::{RetrievalReport, render_markdown, run};
use std::fs;
use std::path::{Path, PathBuf};

/// Options for `retrieval`.
#[derive(Clone, Debug)]
pub struct RetrievalOptions {
    /// Root under which the report directory is written.
    pub results_root: PathBuf,
}

impl Default for RetrievalOptions {
    fn default() -> Self {
        Self {
            results_root: PathBuf::from("experiments/results"),
        }
    }
}

/// What `retrieval` produced.
#[derive(Clone, Debug)]
pub struct RetrievalOutcome {
    /// Where the files went.
    pub dir: PathBuf,
    /// The report.
    pub report: RetrievalReport,
    /// Markdown rendering.
    pub markdown: String,
}

/// Runs the retrieval workload described by a manifest.
pub fn run_retrieval(
    manifest_path: &Path,
    options: &RetrievalOptions,
) -> Result<RetrievalOutcome, String> {
    let manifest = load_manifest(manifest_path)?;
    let spec = manifest
        .retrieval
        .as_ref()
        .ok_or("manifest has no `retrieval` section")?;
    let report = run(spec, manifest.seed);
    let title = format!(
        "Retrieval: {} (arity {}, {} queries per setting)",
        manifest.name, spec.arity, spec.queries
    );
    let markdown = render_markdown(&report, &title);
    let dir = options.results_root.join(format!(
        "retrieval-{}-{}",
        manifest.name,
        manifest.hash().short()
    ));
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    write_json_pretty(dir.join("manifest.json"), &manifest).map_err(|e| e.to_string())?;
    write_json_pretty(dir.join("retrieval.json"), &report).map_err(|e| e.to_string())?;
    fs::write(dir.join("retrieval.md"), &markdown).map_err(|e| e.to_string())?;
    Ok(RetrievalOutcome {
        dir,
        report,
        markdown,
    })
}
