//! Scoped, unverified cross-task notes. Scope and grants are controlled by the host.
use std::{
    fmt,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    time::Duration,
};

use metron_protocol::types::{Digest, TaskId, content_digest};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};

const SCHEMA: &str = "
PRAGMA user_version=1001;
CREATE TABLE grants(principal TEXT NOT NULL, project TEXT NOT NULL, collection TEXT NOT NULL, PRIMARY KEY(principal,project,collection));
CREATE TABLE entries(id INTEGER PRIMARY KEY, dispatch TEXT NOT NULL UNIQUE, principal TEXT NOT NULL, project TEXT NOT NULL, collection TEXT NOT NULL, source_task TEXT NOT NULL, source_evidence TEXT, body TEXT NOT NULL, content_hash TEXT NOT NULL, trust TEXT NOT NULL CHECK(trust='model_authored_unverified'));
CREATE TRIGGER immutable_update BEFORE UPDATE ON entries BEGIN SELECT RAISE(ABORT, 'immutable'); END;
CREATE TRIGGER immutable_delete BEFORE DELETE ON entries BEGIN SELECT RAISE(ABORT, 'immutable'); END;
CREATE VIRTUAL TABLE entry_fts USING fts5(body, content='entries', content_rowid='id');
CREATE TRIGGER entries_index AFTER INSERT ON entries BEGIN INSERT INTO entry_fts(rowid,body) VALUES(new.id,new.body); END;
";

/// Deliberately excludes SQLite's messages and stored data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryError {
    InvalidInput,
    Denied,
    DispatchConflict,
    Integrity,
    Storage,
    PrivateFileRequired,
}
impl fmt::Display for MemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "memory error: {self:?}")
    }
}
impl std::error::Error for MemoryError {}
fn storage(_: rusqlite::Error) -> MemoryError {
    MemoryError::Storage
}
fn valid(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && !s.contains('\0')
}

/// Construct only from trusted task/policy data, never from model tool arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    principal: String,
    project: String,
    collection: String,
}
impl Scope {
    pub fn new(
        principal: impl Into<String>,
        project: impl Into<String>,
        collection: impl Into<String>,
    ) -> Result<Self, MemoryError> {
        let s = Self {
            principal: principal.into(),
            project: project.into(),
            collection: collection.into(),
        };
        if !valid(&s.principal, 256) || !valid(&s.project, 256) || !valid(&s.collection, 256) {
            return Err(MemoryError::InvalidInput);
        }
        Ok(s)
    }
}

/// Model-authored input; evidence is provenance, not validation of the claim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryInput {
    pub dispatch: Digest,
    pub source_task: TaskId,
    pub source_evidence: Option<Digest>,
    pub body: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryEntry {
    pub dispatch: Digest,
    pub source_task: TaskId,
    pub source_evidence: Option<Digest>,
    pub body: String,
    pub content_hash: Digest,
    pub trust: &'static str,
}

type StoredInput = (
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    String,
);

fn require_private_files(path: &Path) -> Result<(), MemoryError> {
    for suffix in ["", "-wal", "-shm"] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        let metadata = match std::fs::symlink_metadata(Path::new(&name)) {
            Ok(metadata) => metadata,
            Err(e) if !suffix.is_empty() && e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(MemoryError::Storage),
        };
        if !metadata.is_file() || metadata.nlink() != 1 || metadata.mode() & 0o077 != 0 {
            return Err(MemoryError::PrivateFileRequired);
        }
    }
    Ok(())
}

pub struct MemoryStore {
    connection: Connection,
}
impl MemoryStore {
    /// Fresh scope check used before any model request that may contain notes.
    pub fn has_grant(&mut self, scope: &Scope) -> Result<bool, MemoryError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let allowed:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM grants WHERE principal=?1 AND project=?2 AND collection=?3)",params![scope.principal,scope.project,scope.collection],|row|row.get(0)).map_err(storage)?;
        tx.commit().map_err(storage)?;
        Ok(allowed)
    }
    pub fn create(path: impl AsRef<Path>) -> Result<Self, MemoryError> {
        let path = path.as_ref();
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|_| MemoryError::Storage)?;
        let result = (|| {
            require_private_files(path)?;
            let connection = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            )
            .map_err(storage)?;
            configure(&connection)?;
            connection.execute_batch(SCHEMA).map_err(storage)?;
            Ok(Self { connection })
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(path);
        }
        result
    }
    pub fn open_existing(path: impl AsRef<Path>) -> Result<Self, MemoryError> {
        let path = path.as_ref();
        require_private_files(path)?;
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(storage)?;
        configure(&connection)?;
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(storage)?;
        if version != 1001 {
            return Err(MemoryError::Integrity);
        }
        Ok(Self { connection })
    }
    /// Trusted administrator action; serialized with every put and search.
    pub fn grant(&mut self, scope: &Scope) -> Result<(), MemoryError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        tx.execute(
            "INSERT OR IGNORE INTO grants VALUES (?1,?2,?3)",
            params![scope.principal, scope.project, scope.collection],
        )
        .map_err(storage)?;
        tx.commit().map_err(storage)
    }
    pub fn revoke(&mut self, scope: &Scope) -> Result<(), MemoryError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        tx.execute(
            "DELETE FROM grants WHERE principal=?1 AND project=?2 AND collection=?3",
            params![scope.principal, scope.project, scope.collection],
        )
        .map_err(storage)?;
        tx.commit().map_err(storage)
    }
    pub fn put(&mut self, scope: &Scope, input: &MemoryInput) -> Result<MemoryEntry, MemoryError> {
        if !valid(&input.body, 4096) {
            return Err(MemoryError::InvalidInput);
        }
        let hash = content_digest(input.body.as_bytes());
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        authorized(&tx, scope)?;
        let existing: Option<StoredInput> = tx.query_row("SELECT principal,project,collection,source_task,source_evidence,body,content_hash FROM entries WHERE dispatch=?1", [input.dispatch.as_str()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional().map_err(storage)?;
        let evidence = input.source_evidence.as_ref().map(Digest::as_str);
        if let Some((p, j, c, t, e, b, h)) = existing {
            if p != scope.principal
                || j != scope.project
                || c != scope.collection
                || t != input.source_task.as_str()
                || e.as_deref() != evidence
                || b != input.body
                || h != hash.as_str()
            {
                return Err(MemoryError::DispatchConflict);
            }
        } else {
            tx.execute("INSERT INTO entries(dispatch,principal,project,collection,source_task,source_evidence,body,content_hash,trust) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'model_authored_unverified')", params![input.dispatch.as_str(),scope.principal,scope.project,scope.collection,input.source_task.as_str(),evidence,input.body,hash.as_str()]).map_err(storage)?;
        }
        tx.commit().map_err(storage)?;
        Ok(MemoryEntry {
            dispatch: input.dispatch.clone(),
            source_task: input.source_task.clone(),
            source_evidence: input.source_evidence.clone(),
            body: input.body.clone(),
            content_hash: hash,
            trust: "model_authored_unverified",
        })
    }
    /// Literal FTS phrase, ordered by insertion ID; at most eight notes and 4096 bytes across returned text fields.
    pub fn search(
        &mut self,
        scope: &Scope,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        if !valid(query, 256) || limit == 0 || limit > 8 {
            return Err(MemoryError::InvalidInput);
        }
        let phrase = format!("\"{}\"", query.replace('"', "\"\""));
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        authorized(&tx, scope)?;
        let entries = {
            let mut stmt = tx.prepare("SELECT e.dispatch,e.source_task,e.source_evidence,e.body,e.content_hash FROM entry_fts f JOIN entries e ON e.id=f.rowid WHERE entry_fts MATCH ?1 AND e.principal=?2 AND e.project=?3 AND e.collection=?4 ORDER BY e.id LIMIT ?5").map_err(storage)?;
            let mut rows = stmt
                .query(params![
                    phrase,
                    scope.principal,
                    scope.project,
                    scope.collection,
                    limit as i64
                ])
                .map_err(storage)?;
            let mut out = Vec::new();
            let mut bytes = 0;
            while let Some(r) = rows.next().map_err(storage)? {
                let body: String = r.get(3).map_err(storage)?;
                let dispatch: String = r.get(0).map_err(storage)?;
                let task: String = r.get(1).map_err(storage)?;
                let evidence: Option<String> = r.get(2).map_err(storage)?;
                let hash: String = r.get(4).map_err(storage)?;
                let size = body.len()
                    + dispatch.len()
                    + task.len()
                    + evidence.as_ref().map_or(0, String::len)
                    + hash.len()
                    + "model_authored_unverified".len();
                if bytes + size > 4096 {
                    break;
                }
                bytes += size;
                let content_hash = Digest::new(hash).map_err(|_| MemoryError::Integrity)?;
                if content_digest(body.as_bytes()) != content_hash {
                    return Err(MemoryError::Integrity);
                }
                out.push(MemoryEntry {
                    dispatch: Digest::new(dispatch).map_err(|_| MemoryError::Integrity)?,
                    source_task: TaskId::new(task).map_err(|_| MemoryError::Integrity)?,
                    source_evidence: evidence
                        .map(Digest::new)
                        .transpose()
                        .map_err(|_| MemoryError::Integrity)?,
                    body,
                    content_hash,
                    trust: "model_authored_unverified",
                });
            }
            out
        };
        tx.commit().map_err(storage)?;
        Ok(entries)
    }
    /// Rebuild the disposable FTS index from canonical entries atomically.
    pub fn rebuild_index(&mut self) -> Result<(), MemoryError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        tx.execute("INSERT INTO entry_fts(entry_fts) VALUES('rebuild')", [])
            .map_err(storage)?;
        tx.commit().map_err(storage)
    }
}
fn authorized(tx: &rusqlite::Transaction<'_>, scope: &Scope) -> Result<(), MemoryError> {
    let found: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM grants WHERE principal=?1 AND project=?2 AND collection=?3)", params![scope.principal,scope.project,scope.collection], |r| r.get(0)).map_err(storage)?;
    if found {
        Ok(())
    } else {
        Err(MemoryError::Denied)
    }
}
fn configure(connection: &Connection) -> Result<(), MemoryError> {
    connection
        .busy_timeout(Duration::from_millis(1000))
        .map_err(storage)?;
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(storage)?;
    Ok(())
}
