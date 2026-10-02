//! File helpers: text, JSON and JSON Lines.

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;

/// File errors.
#[derive(Debug, thiserror::Error)]
pub enum FileError {
    /// I/O failure, with the path.
    #[error("{path}: {source}")]
    Io {
        /// The path.
        path: String,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
    /// JSON failure, with the path.
    #[error("{path}: {source}")]
    Json {
        /// The path.
        path: String,
        /// The underlying error.
        #[source]
        source: serde_json::Error,
    },
}

fn io(path: &Path, source: std::io::Error) -> FileError {
    FileError::Io {
        path: path.display().to_string(),
        source,
    }
}

fn json(path: &Path, source: serde_json::Error) -> FileError {
    FileError::Json {
        path: path.display().to_string(),
        source,
    }
}

/// Reads a whole text file.
pub fn read_text(path: impl AsRef<Path>) -> Result<String, FileError> {
    let path = path.as_ref();
    fs::read_to_string(path).map_err(|e| io(path, e))
}

/// Reads and parses a JSON file.
pub fn load_json<T: DeserializeOwned>(path: impl AsRef<Path>) -> Result<T, FileError> {
    let path = path.as_ref();
    let text = read_text(path)?;
    serde_json::from_str(&text).map_err(|e| json(path, e))
}

/// Writes a value as pretty JSON, creating parent directories.
pub fn write_json_pretty<T: Serialize>(path: impl AsRef<Path>, value: &T) -> Result<(), FileError> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| json(path, e))?;
    fs::write(path, text).map_err(|e| io(path, e))
}

/// Writes items as JSON Lines, creating parent directories.
pub fn write_jsonl<'a, T, I>(path: impl AsRef<Path>, items: I) -> Result<(), FileError>
where
    T: Serialize + 'a,
    I: IntoIterator<Item = &'a T>,
{
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    }
    let file = fs::File::create(path).map_err(|e| io(path, e))?;
    let mut writer = BufWriter::new(file);
    for item in items {
        let line = serde_json::to_vec(item).map_err(|e| json(path, e))?;
        writer.write_all(&line).map_err(|e| io(path, e))?;
        writer.write_all(b"\n").map_err(|e| io(path, e))?;
    }
    writer.flush().map_err(|e| io(path, e))
}

/// Reads JSON Lines into values, skipping blank lines.
pub fn read_jsonl<T: DeserializeOwned>(path: impl AsRef<Path>) -> Result<Vec<T>, FileError> {
    let path = path.as_ref();
    let file = fs::File::open(path).map_err(|e| io(path, e))?;
    let reader = BufReader::new(file);
    let mut out = Vec::new();
    for line in reader.lines() {
        let line = line.map_err(|e| io(path, e))?;
        if line.trim().is_empty() {
            continue;
        }
        out.push(serde_json::from_str(&line).map_err(|e| json(path, e))?);
    }
    Ok(out)
}
