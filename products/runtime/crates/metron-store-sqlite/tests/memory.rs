use metron_protocol::types::{TaskId, content_digest};
use metron_store_sqlite::memory::{MemoryError, MemoryInput, MemoryStore, Scope};
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "metron-memory-{}-{}.sqlite",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}
fn note(id: &str, body: &str) -> MemoryInput {
    MemoryInput {
        dispatch: content_digest(id.as_bytes()),
        source_task: TaskId::new("task-one").unwrap(),
        source_evidence: Some(content_digest(b"source")),
        body: body.into(),
    }
}
#[test]
fn persistence_scope_revocation_and_rebuild() {
    let path = path();
    let scope = Scope::new("alice", "project", "notes").unwrap();
    let mut store = MemoryStore::create(&path).unwrap();
    assert_eq!(
        store.put(&scope, &note("1", "The answer is yes")),
        Err(MemoryError::Denied)
    );
    store.grant(&scope).unwrap();
    let first = store.put(&scope, &note("1", "The answer is yes")).unwrap();
    assert_eq!(
        store.put(&scope, &note("1", "The answer is yes")).unwrap(),
        first
    );
    assert_eq!(
        store.put(&scope, &note("1", "The answer is no")),
        Err(MemoryError::DispatchConflict)
    );
    store.put(&scope, &note("2", "The answer is no")).unwrap();
    drop(store);
    let mut store = MemoryStore::open_existing(&path).unwrap();
    let result = store.search(&scope, "answer", 8).unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].trust, "model_authored_unverified");
    assert_eq!(result[0].source_evidence, Some(content_digest(b"source")));
    assert_eq!(
        result[0].content_hash,
        content_digest(result[0].body.as_bytes())
    );
    store.rebuild_index().unwrap();
    assert_eq!(store.search(&scope, "answer", 8).unwrap(), result);
    for other in [
        Scope::new("bob", "project", "notes").unwrap(),
        Scope::new("alice", "other", "notes").unwrap(),
        Scope::new("alice", "project", "other").unwrap(),
    ] {
        assert_eq!(store.search(&other, "answer", 8), Err(MemoryError::Denied));
        store.grant(&other).unwrap();
        assert!(store.search(&other, "answer", 8).unwrap().is_empty());
        assert_eq!(
            store.put(&other, &note("1", "The answer is yes")),
            Err(MemoryError::DispatchConflict)
        );
    }
    store.revoke(&scope).unwrap();
    assert_eq!(store.search(&scope, "answer", 8), Err(MemoryError::Denied));
    assert_eq!(
        store.put(&scope, &note("3", "answer")),
        Err(MemoryError::Denied)
    );
    store.grant(&scope).unwrap();
    assert_eq!(store.search(&scope, "answer", 8).unwrap(), result);
    drop(store);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn limits_and_literal_queries() {
    let path = path();
    let scope = Scope::new("a", "p", "c").unwrap();
    let mut store = MemoryStore::create(&path).unwrap();
    store.grant(&scope).unwrap();
    assert_eq!(
        store.put(&scope, &note("long", &"a".repeat(4097))),
        Err(MemoryError::InvalidInput)
    );
    for i in 0..10 {
        store
            .put(
                &scope,
                &note(&i.to_string(), &format!("needle {i} {}", "x".repeat(900))),
            )
            .unwrap();
    }
    let found = store.search(&scope, "needle", 8).unwrap();
    assert_eq!(found.len(), 3);
    assert!(found.iter().map(|x| x.body.len()).sum::<usize>() <= 4096);
    assert!(
        store
            .search(&scope, "needle OR missing", 8)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.search(&scope, "needle", 9),
        Err(MemoryError::InvalidInput)
    );
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn index_failure_rolls_back_canonical_entry_and_revocation_is_shared() {
    let path = path();
    let scope = Scope::new("a", "p", "c").unwrap();
    let mut first = MemoryStore::create(&path).unwrap();
    first.grant(&scope).unwrap();
    let mut second = MemoryStore::open_existing(&path).unwrap();
    let raw = rusqlite::Connection::open(&path).unwrap();
    raw.execute_batch("CREATE TRIGGER fail_insert AFTER INSERT ON entries BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
    assert_eq!(
        first.put(&scope, &note("failure", "needle")),
        Err(MemoryError::Storage)
    );
    let count: i64 = raw
        .query_row("SELECT count(*) FROM entries", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    raw.execute_batch("DROP TRIGGER fail_insert").unwrap();
    first.put(&scope, &note("failure", "needle")).unwrap();
    second.revoke(&scope).unwrap();
    assert_eq!(first.search(&scope, "needle", 8), Err(MemoryError::Denied));
    assert_eq!(
        first.put(&scope, &note("new", "needle")),
        Err(MemoryError::Denied)
    );
    drop(raw);
    drop(first);
    drop(second);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn database_and_sidecars_are_private_and_insecure_existing_files_are_rejected() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let path = path();
    let scope = Scope::new("alice", "private-project", "notes").unwrap();
    let mut store = MemoryStore::create(&path).unwrap();
    store.grant(&scope).unwrap();
    store
        .put(&scope, &note("private-note", "private body"))
        .unwrap();
    for suffix in ["", "-wal", "-shm"] {
        let mut file = path.as_os_str().to_owned();
        file.push(suffix);
        if let Ok(metadata) = std::fs::symlink_metadata(&file) {
            assert_eq!(metadata.mode() & 0o077, 0);
        }
    }
    drop(store);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        MemoryStore::open_existing(&path),
        Err(MemoryError::PrivateFileRequired)
    ));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    std::fs::write(&wal, []).unwrap();
    std::fs::set_permissions(&wal, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        MemoryStore::open_existing(&path),
        Err(MemoryError::PrivateFileRequired)
    ));
    std::fs::remove_file(wal).unwrap();
    let alias = path.with_extension("link");
    std::os::unix::fs::symlink(&path, &alias).unwrap();
    assert!(matches!(
        MemoryStore::open_existing(&alias),
        Err(MemoryError::PrivateFileRequired)
    ));
    std::fs::remove_file(alias).unwrap();
    std::fs::remove_file(path).unwrap();
}
