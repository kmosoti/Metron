//! Durable SQLite adapter for the runtime.
#![forbid(unsafe_code)]

mod engine;
pub mod memory;

use std::path::Path;
use std::time::Duration;

use metron_protocol::store::StoreError;
use metron_protocol::types::{BoundedText, MAX_TEXT, TaskId};
use rusqlite::{Connection, OpenFlags, backup::Backup};

const SCHEMA: &str = include_str!("schema.sql");

/// Probe the linked SQLite build, not a separately installed sqlite3 command.
/// This establishes FTS5 availability only, not runtime memory authorization.
pub fn search_capabilities() -> Result<(String, bool), StoreError> {
    let connection = Connection::open_in_memory().map_err(operational)?;
    let version: String = connection
        .query_row("SELECT sqlite_version()", [], |r| r.get(0))
        .map_err(operational)?;
    connection
        .execute_batch(
            "CREATE VIRTUAL TABLE probe USING fts5(scope UNINDEXED, content);
        INSERT INTO probe(scope,content) VALUES ('authorized','needle');",
        )
        .map_err(operational)?;
    let visible: i64 = connection
        .query_row(
            "SELECT count(*) FROM probe WHERE probe MATCH 'needle' AND scope='authorized'",
            [],
            |r| r.get(0),
        )
        .map_err(operational)?;
    let hidden: i64 = connection
        .query_row(
            "SELECT count(*) FROM probe WHERE probe MATCH 'needle' AND scope='other'",
            [],
            |r| r.get(0),
        )
        .map_err(operational)?;
    Ok((version, visible == 1 && hidden == 0))
}

fn operational(error: impl std::fmt::Display) -> StoreError {
    let mut text = error.to_string();
    text.truncate(text.floor_char_boundary(MAX_TEXT));
    StoreError::Operational(BoundedText::new(text).expect("bounded error"))
}

/// A single SQLite connection. Transactions never span external effects.
pub struct SqliteStore {
    connection: Connection,
    read_only: bool,
}

impl SqliteStore {
    /// Create a new store, failing if the destination already exists.
    pub fn create(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref();
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(operational)?;
        let result = (|| {
            let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
                .map_err(operational)?;
            configure(&connection, false)?;
            connection.execute_batch(SCHEMA).map_err(operational)?;
            Ok(Self {
                connection,
                read_only: false,
            })
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(path);
        }
        result
    }

    /// Open an existing store without creating or migrating it.
    pub fn open_existing(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::open(path.as_ref(), false)
    }

    /// Open an existing store without domain writes, migration or checkpoint.
    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::open(path.as_ref(), true)
    }

    fn open(path: &Path, read_only: bool) -> Result<Self, StoreError> {
        let flags = if read_only {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        } else {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        };
        let connection = Connection::open_with_flags(path, flags).map_err(operational)?;
        configure(&connection, read_only)?;
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(operational)?;
        if version != 1 {
            return Err(StoreError::Integrity);
        }
        Ok(Self {
            connection,
            read_only,
        })
    }

    /// Whether this connection was opened read-only.
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Return task identifiers in a stable order.
    pub fn task_ids(&self) -> Result<Vec<TaskId>, StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT task_id FROM tasks ORDER BY task_id")
            .map_err(operational)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(operational)?;
        rows.map(|value| TaskId::new(value.map_err(operational)?).map_err(StoreError::Validation))
            .collect()
    }

    /// Use SQLite's online backup API and validate the new copy.
    pub fn backup_to_new(&self, path: impl AsRef<Path>) -> Result<(), StoreError> {
        let path = path.as_ref();
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(operational)?;
        let result = (|| {
            let mut destination =
                Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
                    .map_err(operational)?;
            let backup = Backup::new(&self.connection, &mut destination).map_err(operational)?;
            backup
                .run_to_completion(128, Duration::from_millis(10), None)
                .map_err(operational)?;
            drop(backup);
            let check: String = destination
                .query_row("PRAGMA integrity_check", [], |row| row.get(0))
                .map_err(operational)?;
            if check != "ok" {
                return Err(StoreError::Integrity);
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(path);
        }
        result
    }
}

fn configure(connection: &Connection, read_only: bool) -> Result<(), StoreError> {
    connection
        .busy_timeout(Duration::from_millis(1000))
        .map_err(operational)?;
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(operational)?;
    if !read_only {
        let mode: String = connection
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .map_err(operational)?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(StoreError::Integrity);
        }
        connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(operational)?;
    }
    let mode: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .map_err(operational)?;
    let sync: i64 = connection
        .pragma_query_value(None, "synchronous", |row| row.get(0))
        .map_err(operational)?;
    let foreign: i64 = connection
        .pragma_query_value(None, "foreign_keys", |row| row.get(0))
        .map_err(operational)?;
    let busy: i64 = connection
        .pragma_query_value(None, "busy_timeout", |row| row.get(0))
        .map_err(operational)?;
    if !mode.eq_ignore_ascii_case("wal") || sync != 2 || foreign != 1 || busy != 1000 {
        return Err(StoreError::Integrity);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn temporary_dir() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "metron-sqlite-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn create_open_and_backup_preserve_database() {
        let directory = temporary_dir();
        let path = directory.join("run.sqlite");
        let copy = directory.join("copy.sqlite");
        let store = SqliteStore::create(&path).unwrap();
        assert!(SqliteStore::create(&path).is_err());
        store
            .connection
            .execute(
                "INSERT INTO metadata(key,value) VALUES ('test', x'0102')",
                [],
            )
            .unwrap();
        store.backup_to_new(&copy).unwrap();
        assert!(store.backup_to_new(&copy).is_err());
        let restored = SqliteStore::open_read_only(&copy).unwrap();
        assert!(restored.is_read_only());
        let bytes: Vec<u8> = restored
            .connection
            .query_row("SELECT value FROM metadata WHERE key='test'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(bytes, [1, 2]);
        assert!(
            restored
                .connection
                .execute("DELETE FROM metadata", [])
                .is_err()
        );
        drop(restored);
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn existing_open_does_not_create_missing_database() {
        let directory = temporary_dir();
        let path = directory.join("missing.sqlite");
        assert!(SqliteStore::open_existing(&path).is_err());
        assert!(SqliteStore::open_read_only(&path).is_err());
        assert!(!path.exists());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
