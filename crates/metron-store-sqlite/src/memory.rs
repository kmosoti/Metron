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

const ANNOTATION_SCHEMA: &str = "
CREATE TABLE source_annotations(
 id INTEGER PRIMARY KEY, dispatch TEXT NOT NULL UNIQUE,
 principal TEXT NOT NULL, project TEXT NOT NULL, collection TEXT NOT NULL,
 target_dispatch TEXT NOT NULL REFERENCES entries(dispatch),
 kind TEXT NOT NULL CHECK(kind IN ('stale', 'conflicts_with')),
 related_dispatch TEXT REFERENCES entries(dispatch),
 source_evidence TEXT NOT NULL, reason TEXT NOT NULL CHECK(length(CAST(reason AS BLOB)) BETWEEN 1 AND 512),
 origin TEXT NOT NULL CHECK(origin='trusted_host'),
 CHECK((kind='stale' AND related_dispatch IS NULL) OR
       (kind='conflicts_with' AND related_dispatch IS NOT NULL AND related_dispatch<>target_dispatch))
);
CREATE INDEX source_annotations_target ON source_annotations(target_dispatch, id);
CREATE INDEX source_annotations_related ON source_annotations(related_dispatch, id);
CREATE TRIGGER source_annotations_immutable_update BEFORE UPDATE ON source_annotations BEGIN SELECT RAISE(ABORT, 'immutable'); END;
CREATE TRIGGER source_annotations_immutable_delete BEFORE DELETE ON source_annotations BEGIN SELECT RAISE(ABORT, 'immutable'); END;
PRAGMA user_version=1002;
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
/// Only trusted host code may construct status annotations; model tools cannot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceAnnotationKind {
    Stale,
    ConflictsWith { related_dispatch: Digest },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceAnnotationInput {
    pub dispatch: Digest,
    pub target_dispatch: Digest,
    pub kind: SourceAnnotationKind,
    pub source_evidence: Digest,
    pub reason: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceAnnotation {
    pub dispatch: Digest,
    pub target_dispatch: Digest,
    pub kind: SourceAnnotationKind,
    pub source_evidence: Digest,
    pub reason: String,
    pub origin: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryEntry {
    pub dispatch: Digest,
    pub source_task: TaskId,
    pub source_evidence: Option<Digest>,
    pub body: String,
    pub content_hash: Digest,
    pub trust: &'static str,
    /// Empty means unannotated; absence does not imply freshness or truth.
    pub annotations: Vec<SourceAnnotation>,
}

/// Bounded search candidates before host-side excerpt and context selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemorySearchPage {
    pub entries: Vec<MemoryEntry>,
    pub has_more: bool,
    pub requested_limit: usize,
}

type StoredAnnotation = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    String,
);
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
            let mut connection = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            )
            .map_err(storage)?;
            configure(&connection)?;
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            tx.execute_batch(SCHEMA).map_err(storage)?;
            tx.execute_batch(ANNOTATION_SCHEMA).map_err(storage)?;
            tx.commit().map_err(storage)?;
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
        let mut connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(storage)?;
        configure(&connection)?;
        {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let version: i64 = tx
                .pragma_query_value(None, "user_version", |r| r.get(0))
                .map_err(storage)?;
            match version {
                1001 => tx.execute_batch(ANNOTATION_SCHEMA).map_err(storage)?,
                1002 => {}
                _ => return Err(MemoryError::Integrity),
            }
            tx.commit().map_err(storage)?;
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
        let annotations = annotations_for(&tx, &input.dispatch)?;
        tx.commit().map_err(storage)?;
        Ok(MemoryEntry {
            dispatch: input.dispatch.clone(),
            source_task: input.source_task.clone(),
            source_evidence: input.source_evidence.clone(),
            body: input.body.clone(),
            content_hash: hash,
            trust: "model_authored_unverified",
            annotations,
        })
    }
    /// Trusted host-only append. Status never changes the claim's trust level.
    pub fn annotate_source(
        &mut self,
        scope: &Scope,
        input: &SourceAnnotationInput,
    ) -> Result<SourceAnnotation, MemoryError> {
        if !valid(&input.reason, 512) {
            return Err(MemoryError::InvalidInput);
        }
        let (kind, related) = match &input.kind {
            SourceAnnotationKind::Stale => ("stale", None),
            SourceAnnotationKind::ConflictsWith { related_dispatch } => {
                if related_dispatch == &input.target_dispatch {
                    return Err(MemoryError::InvalidInput);
                }
                ("conflicts_with", Some(related_dispatch.as_str()))
            }
        };
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        authorized(&tx, scope)?;
        let existing: Option<StoredAnnotation> = tx.query_row(
            "SELECT principal,project,collection,target_dispatch,kind,related_dispatch,source_evidence,reason FROM source_annotations WHERE dispatch=?1",
            [input.dispatch.as_str()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?)))
            .optional().map_err(storage)?;
        if let Some((p, j, c, target, stored_kind, stored_related, evidence, reason)) = existing {
            if p != scope.principal
                || j != scope.project
                || c != scope.collection
                || target != input.target_dispatch.as_str()
                || stored_kind != kind
                || stored_related.as_deref() != related
                || evidence != input.source_evidence.as_str()
                || reason != input.reason
            {
                return Err(MemoryError::DispatchConflict);
            }
        } else {
            if !entry_in_scope(&tx, scope, &input.target_dispatch)? {
                return Err(MemoryError::InvalidInput);
            }
            if let SourceAnnotationKind::ConflictsWith { related_dispatch } = &input.kind
                && !entry_in_scope(&tx, scope, related_dispatch)?
            {
                return Err(MemoryError::InvalidInput);
            }
            for endpoint in [Some(input.target_dispatch.as_str()), related]
                .into_iter()
                .flatten()
            {
                let count: i64 = tx.query_row(
                    "SELECT count(*) FROM source_annotations WHERE target_dispatch=?1 OR related_dispatch=?1",
                    [endpoint], |r| r.get(0)).map_err(storage)?;
                if count >= 8 {
                    return Err(MemoryError::InvalidInput);
                }
            }
            tx.execute("INSERT INTO source_annotations(dispatch,principal,project,collection,target_dispatch,kind,related_dispatch,source_evidence,reason,origin) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'trusted_host')",
                params![input.dispatch.as_str(),scope.principal,scope.project,scope.collection,input.target_dispatch.as_str(),kind,related,input.source_evidence.as_str(),input.reason]).map_err(storage)?;
        }
        tx.commit().map_err(storage)?;
        Ok(SourceAnnotation {
            dispatch: input.dispatch.clone(),
            target_dispatch: input.target_dispatch.clone(),
            kind: input.kind.clone(),
            source_evidence: input.source_evidence.clone(),
            reason: input.reason.clone(),
            origin: "trusted_host",
        })
    }
    /// Legacy result bounded to 4096 bytes across note and annotation fields.
    pub fn search(
        &mut self,
        scope: &Scope,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        let page = self.search_candidates(scope, query, limit)?;
        let mut bytes = 0;
        let mut out = Vec::new();
        for entry in page.entries {
            let size = entry_size(&entry);
            if bytes + size > 4096 {
                break;
            }
            bytes += size;
            out.push(entry);
        }
        Ok(out)
    }
    /// Literal FTS phrase with complete bounded entries and a truthful overflow bit.
    pub fn search_candidates(
        &mut self,
        scope: &Scope,
        query: &str,
        limit: usize,
    ) -> Result<MemorySearchPage, MemoryError> {
        if !valid(query, 256) || limit == 0 || limit > 8 {
            return Err(MemoryError::InvalidInput);
        }
        let phrase = format!("\"{}\"", query.replace('"', "\"\""));
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        authorized(&tx, scope)?;
        let (entries, has_more) = {
            let mut stmt = tx.prepare("SELECT e.dispatch,e.source_task,e.source_evidence,e.body,e.content_hash FROM entry_fts f JOIN entries e ON e.id=f.rowid WHERE entry_fts MATCH ?1 AND e.principal=?2 AND e.project=?3 AND e.collection=?4 ORDER BY e.id LIMIT ?5").map_err(storage)?;
            let mut rows = stmt
                .query(params![
                    phrase,
                    scope.principal,
                    scope.project,
                    scope.collection,
                    (limit + 1) as i64
                ])
                .map_err(storage)?;
            let mut out = Vec::new();
            let mut has_more = false;
            while let Some(r) = rows.next().map_err(storage)? {
                if out.len() == limit {
                    has_more = true;
                    break;
                }
                let body: String = r.get(3).map_err(storage)?;
                let dispatch = Digest::new(r.get::<_, String>(0).map_err(storage)?)
                    .map_err(|_| MemoryError::Integrity)?;
                let task: String = r.get(1).map_err(storage)?;
                let evidence: Option<String> = r.get(2).map_err(storage)?;
                let content_hash = Digest::new(r.get::<_, String>(4).map_err(storage)?)
                    .map_err(|_| MemoryError::Integrity)?;
                if !valid(&body, 4096) || content_digest(body.as_bytes()) != content_hash {
                    return Err(MemoryError::Integrity);
                }
                let annotations = annotations_for(&tx, &dispatch)?;
                out.push(MemoryEntry {
                    dispatch,
                    source_task: TaskId::new(task).map_err(|_| MemoryError::Integrity)?,
                    source_evidence: evidence
                        .map(Digest::new)
                        .transpose()
                        .map_err(|_| MemoryError::Integrity)?,
                    body,
                    content_hash,
                    trust: "model_authored_unverified",
                    annotations,
                });
            }
            (out, has_more)
        };
        tx.commit().map_err(storage)?;
        Ok(MemorySearchPage {
            entries,
            has_more,
            requested_limit: limit,
        })
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
fn entry_size(entry: &MemoryEntry) -> usize {
    let annotation_bytes: usize = entry
        .annotations
        .iter()
        .map(|a| {
            a.dispatch.as_str().len()
                + a.target_dispatch.as_str().len()
                + a.source_evidence.as_str().len()
                + a.reason.len()
                + a.origin.len()
                + match &a.kind {
                    SourceAnnotationKind::Stale => 5,
                    SourceAnnotationKind::ConflictsWith { related_dispatch } => {
                        14 + related_dispatch.as_str().len()
                    }
                }
        })
        .sum();
    annotation_bytes
        + entry.body.len()
        + entry.dispatch.as_str().len()
        + entry.source_task.as_str().len()
        + entry
            .source_evidence
            .as_ref()
            .map_or(0, |d| d.as_str().len())
        + entry.content_hash.as_str().len()
        + entry.trust.len()
}
fn entry_in_scope(
    tx: &rusqlite::Transaction<'_>,
    scope: &Scope,
    dispatch: &Digest,
) -> Result<bool, MemoryError> {
    tx.query_row("SELECT EXISTS(SELECT 1 FROM entries WHERE dispatch=?1 AND principal=?2 AND project=?3 AND collection=?4)", params![dispatch.as_str(),scope.principal,scope.project,scope.collection], |r| r.get(0)).map_err(storage)
}
fn annotations_for(
    tx: &rusqlite::Transaction<'_>,
    target: &Digest,
) -> Result<Vec<SourceAnnotation>, MemoryError> {
    let mut stmt = tx.prepare("SELECT dispatch,target_dispatch,kind,related_dispatch,source_evidence,reason,origin FROM source_annotations WHERE target_dispatch=?1 OR related_dispatch=?1 ORDER BY dispatch LIMIT 9").map_err(storage)?;
    let mut rows = stmt.query([target.as_str()]).map_err(storage)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(storage)? {
        if out.len() == 8 {
            return Err(MemoryError::Integrity);
        }
        let original_target = Digest::new(row.get::<_, String>(1).map_err(storage)?)
            .map_err(|_| MemoryError::Integrity)?;
        let kind: String = row.get(2).map_err(storage)?;
        let related: Option<String> = row.get(3).map_err(storage)?;
        let origin: String = row.get(6).map_err(storage)?;
        let reason: String = row.get(5).map_err(storage)?;
        if origin != "trusted_host" || !valid(&reason, 512) {
            return Err(MemoryError::Integrity);
        }
        let kind = match (kind.as_str(), related) {
            ("stale", None) if &original_target == target => SourceAnnotationKind::Stale,
            ("conflicts_with", Some(id)) => {
                let related_dispatch = Digest::new(id).map_err(|_| MemoryError::Integrity)?;
                if related_dispatch == original_target
                    || (&original_target != target && &related_dispatch != target)
                {
                    return Err(MemoryError::Integrity);
                }
                SourceAnnotationKind::ConflictsWith { related_dispatch }
            }
            _ => return Err(MemoryError::Integrity),
        };
        out.push(SourceAnnotation {
            dispatch: Digest::new(row.get::<_, String>(0).map_err(storage)?)
                .map_err(|_| MemoryError::Integrity)?,
            target_dispatch: original_target,
            kind,
            source_evidence: Digest::new(row.get::<_, String>(4).map_err(storage)?)
                .map_err(|_| MemoryError::Integrity)?,
            reason,
            origin: "trusted_host",
        });
    }
    Ok(out)
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
