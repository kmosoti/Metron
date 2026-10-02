//! Writing run results.
//!
//! A run directory holds the manifest that produced it, the episode journal
//! as JSON Lines, the full episode object, and a summary that includes the
//! laboratory's verdict. Everything the laboratory wrote is stored as plain
//! JSON values so this crate needs no laboratory types.

use crate::files::{FileError, read_jsonl, write_json_pretty, write_jsonl};
use metron_core::journal::{Episode, JournalEntry, JournalError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Everything a run produces.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    /// Run name (from the manifest).
    pub name: String,
    /// The manifest, verbatim as JSON.
    pub manifest: serde_json::Value,
    /// The episode.
    pub episode: Episode,
    /// The laboratory's verdict, as JSON.
    pub verdict: serde_json::Value,
}

/// Writes run directories under a root.
#[derive(Clone, Debug)]
pub struct ResultsWriter {
    root: PathBuf,
}

impl ResultsWriter {
    /// Creates a writer rooted at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The directory a run would be written to.
    #[must_use]
    pub fn run_dir(&self, record: &RunRecord) -> PathBuf {
        let name: String = record
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
        self.root.join(format!(
            "{name}-seed{}-{}",
            record.episode.seed,
            record.episode.head_hash().short()
        ))
    }

    /// Writes `manifest.json`, `episode.jsonl`, `episode.json` and
    /// `summary.json`, returning the run directory.
    pub fn write(&self, record: &RunRecord) -> Result<PathBuf, FileError> {
        let dir = self.run_dir(record);
        write_json_pretty(dir.join("manifest.json"), &record.manifest)?;
        write_jsonl(dir.join("episode.jsonl"), &record.episode.entries)?;
        write_json_pretty(dir.join("episode.json"), &record.episode)?;
        let summary = serde_json::json!({
            "name": record.name,
            "seed": record.episode.seed,
            "manifest_hash": record.episode.manifest_hash,
            "journal_head": record.episode.head_hash(),
            "entries": record.episode.entries.len(),
            "receipts": record.episode.receipts().count(),
            "outcome": record.episode.outcome,
            "verdict": record.verdict,
        });
        write_json_pretty(dir.join("summary.json"), &summary)?;
        Ok(dir)
    }
}

/// Errors from reading a journal back.
#[derive(Debug, thiserror::Error)]
pub enum ReadJournalError {
    /// File problem.
    #[error(transparent)]
    File(#[from] FileError),
    /// Chain problem.
    #[error(transparent)]
    Journal(#[from] JournalError),
}

/// Reads an `episode.jsonl` file and verifies its chain.
pub fn read_journal(path: impl AsRef<Path>) -> Result<Vec<JournalEntry>, ReadJournalError> {
    let entries: Vec<JournalEntry> = read_jsonl(path)?;
    JournalEntry::verify_chain(&entries)?;
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use metron_core::hash::ContentHash;
    use metron_core::id::{EpisodeId, InquiryId, OperatorId};
    use metron_core::journal::JournalEvent;

    #[test]
    fn write_then_read_back_verifies() {
        let dir = std::env::temp_dir().join(format!("metron-results-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut episode = Episode::new(EpisodeId(1), InquiryId(1), 3, ContentHash::GENESIS);
        episode.append(
            1,
            JournalEvent::OperatorSkipped {
                step: 0,
                operator: OperatorId::from("x"),
            },
        );
        let record = RunRecord {
            name: "unit test".into(),
            manifest: serde_json::json!({"name": "unit test"}),
            episode,
            verdict: serde_json::json!({"correct": false}),
        };
        let run_dir = ResultsWriter::new(&dir).write(&record).unwrap();
        assert!(
            run_dir
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("unit-test-seed3-")
        );
        let entries = read_journal(run_dir.join("episode.jsonl")).unwrap();
        assert_eq!(entries.len(), 1);
        let back: Episode = crate::files::load_json(run_dir.join("episode.json")).unwrap();
        assert_eq!(back, record.episode);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
