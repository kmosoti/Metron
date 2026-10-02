//! Helpers for the executable architecture invariants.
//!
//! These functions read the workspace itself (manifests and sources) so the
//! tests in `tests/` can assert facts about dependency direction and about
//! what each crate is allowed to touch.

use std::fs;
use std::path::{Path, PathBuf};

/// The workspace root.
#[must_use]
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root exists")
}

/// Directory of a workspace crate under `crates/`.
#[must_use]
pub fn crate_dir(name: &str) -> PathBuf {
    workspace_root().join("crates").join(name)
}

/// Names declared in one section of a `Cargo.toml` (for example
/// `dependencies` or `dev-dependencies`). Only the simple, one-line forms
/// used in this workspace are recognised.
#[must_use]
pub fn manifest_deps(manifest: &Path, section: &str) -> Vec<String> {
    let text =
        fs::read_to_string(manifest).unwrap_or_else(|e| panic!("{}: {e}", manifest.display()));
    let header = format!("[{section}]");
    let mut in_section = false;
    let mut names = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            in_section = line == header;
            continue;
        }
        if !in_section || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let name: String = line
            .chars()
            .take_while(|c| !matches!(c, '=' | '.' | ' '))
            .collect();
        if !name.is_empty() {
            names.push(name);
        }
    }
    names
}

/// All `*.rs` files under `dir`, with their contents.
#[must_use]
pub fn rust_sources(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text =
                    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                out.push((path, text));
            }
        }
    }
    out.sort();
    out
}

/// Lines of code (comments excluded) containing any of `tokens`, as
/// `path:line: token` findings.
#[must_use]
pub fn find_tokens(sources: &[(PathBuf, String)], tokens: &[&str]) -> Vec<String> {
    let mut findings = Vec::new();
    for (path, text) in sources {
        for (i, raw) in text.lines().enumerate() {
            let line = raw.trim_start();
            if line.starts_with("//") {
                continue;
            }
            for token in tokens {
                if line.contains(token) {
                    findings.push(format!("{}:{}: {token}", path.display(), i + 1));
                }
            }
        }
    }
    findings
}

/// The text of a `struct` or `enum` item named `name`, from its first line
/// through its closing brace, including the attribute lines directly above.
#[must_use]
pub fn item_text(source: &str, name: &str) -> Option<String> {
    let lines: Vec<&str> = source.lines().collect();
    let start = lines.iter().position(|l| {
        let t = l.trim_start();
        (t.starts_with("pub struct ")
            || t.starts_with("struct ")
            || t.starts_with("pub enum ")
            || t.starts_with("enum "))
            && t.split_whitespace()
                .nth(if t.starts_with("pub") { 2 } else { 1 })
                .is_some_and(|n| n.trim_end_matches(['{', '(', ';']) == name)
    })?;
    let mut first = start;
    while first > 0 && lines[first - 1].trim_start().starts_with("#[") {
        first -= 1;
    }
    let mut depth = 0i32;
    let mut end = start;
    for (i, line) in lines.iter().enumerate().skip(start) {
        depth += line.matches('{').count() as i32;
        depth -= line.matches('}').count() as i32;
        if depth <= 0 && (line.contains('}') || line.trim_end().ends_with(';')) {
            end = i;
            break;
        }
    }
    Some(lines[first..=end].join("\n"))
}
