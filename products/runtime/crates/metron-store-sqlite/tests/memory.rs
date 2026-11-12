use metron_protocol::types::{TaskId, content_digest};
use metron_store_sqlite::memory::{
    MemoryError, MemoryInput, MemoryStore, Scope, SourceAnnotationInput, SourceAnnotationKind,
};
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

fn annotation(id: &str, target: &MemoryInput, kind: SourceAnnotationKind) -> SourceAnnotationInput {
    SourceAnnotationInput {
        dispatch: content_digest(id.as_bytes()),
        target_dispatch: target.dispatch.clone(),
        kind,
        source_evidence: content_digest(b"trusted evidence"),
        reason: "checked source".into(),
    }
}

#[test]
fn trusted_status_is_scoped_append_only_and_survives_reopen_and_rebuild() {
    let path = path();
    let scope = Scope::new("alice", "project", "notes").unwrap();
    let other = Scope::new("alice", "different", "notes").unwrap();
    let mut store = MemoryStore::create(&path).unwrap();
    store.grant(&scope).unwrap();
    store.grant(&other).unwrap();
    let yes = note("status-yes", "answer yes");
    let no = note("status-no", "answer no");
    let foreign = note("status-foreign", "answer foreign");
    store.put(&scope, &yes).unwrap();
    store.put(&scope, &no).unwrap();
    store.put(&other, &foreign).unwrap();
    assert!(
        store
            .search(&scope, "answer", 8)
            .unwrap()
            .iter()
            .all(|n| n.annotations.is_empty())
    );
    let stale = annotation("status-stale", &yes, SourceAnnotationKind::Stale);
    let conflict = annotation(
        "status-conflict",
        &yes,
        SourceAnnotationKind::ConflictsWith {
            related_dispatch: no.dispatch.clone(),
        },
    );
    let first = store.annotate_source(&scope, &stale).unwrap();
    assert_eq!(store.annotate_source(&scope, &stale).unwrap(), first);
    let second = store.annotate_source(&scope, &conflict).unwrap();
    assert_eq!(first.origin, "trusted_host");
    let mut changed = stale.clone();
    changed.reason = "different".into();
    assert_eq!(
        store.annotate_source(&scope, &changed),
        Err(MemoryError::DispatchConflict)
    );
    let cross = annotation(
        "cross",
        &yes,
        SourceAnnotationKind::ConflictsWith {
            related_dispatch: foreign.dispatch.clone(),
        },
    );
    assert_eq!(
        store.annotate_source(&scope, &cross),
        Err(MemoryError::InvalidInput)
    );
    let foreign_target = annotation("foreign-target", &foreign, SourceAnnotationKind::Stale);
    assert_eq!(
        store.annotate_source(&scope, &foreign_target),
        Err(MemoryError::InvalidInput)
    );
    assert_eq!(
        store.annotate_source(&other, &stale),
        Err(MemoryError::DispatchConflict)
    );
    let self_link = annotation(
        "self",
        &yes,
        SourceAnnotationKind::ConflictsWith {
            related_dispatch: yes.dispatch.clone(),
        },
    );
    assert_eq!(
        store.annotate_source(&scope, &self_link),
        Err(MemoryError::InvalidInput)
    );
    let missing = annotation(
        "missing",
        &yes,
        SourceAnnotationKind::ConflictsWith {
            related_dispatch: content_digest(b"missing"),
        },
    );
    assert_eq!(
        store.annotate_source(&scope, &missing),
        Err(MemoryError::InvalidInput)
    );
    let missing_target = annotation(
        "missing-target",
        &note("missing-entry", "x"),
        SourceAnnotationKind::Stale,
    );
    assert_eq!(
        store.annotate_source(&scope, &missing_target),
        Err(MemoryError::InvalidInput)
    );
    let mut long = annotation("long-annotation", &yes, SourceAnnotationKind::Stale);
    long.reason = "x".repeat(513);
    assert_eq!(
        store.annotate_source(&scope, &long),
        Err(MemoryError::InvalidInput)
    );
    let mut second_connection = MemoryStore::open_existing(&path).unwrap();
    second_connection.revoke(&scope).unwrap();
    assert_eq!(
        store.annotate_source(&scope, &stale),
        Err(MemoryError::Denied)
    );
    assert_eq!(store.search(&scope, "answer", 8), Err(MemoryError::Denied));
    second_connection.grant(&scope).unwrap();
    drop(second_connection);
    drop(store);
    let mut store = MemoryStore::open_existing(&path).unwrap();
    let results = store.search(&scope, "answer", 8).unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].body, "answer yes");
    assert_eq!(results[1].body, "answer no");
    let mut expected = vec![first.clone(), second.clone()];
    expected.sort_by(|a, b| a.dispatch.as_str().cmp(b.dispatch.as_str()));
    assert_eq!(results[0].annotations, expected);
    assert_eq!(results[1].annotations, vec![second]);
    assert!(
        results
            .iter()
            .all(|e| e.trust == "model_authored_unverified")
    );
    assert_eq!(
        store.put(&scope, &yes).unwrap().annotations,
        results[0].annotations
    );
    store.rebuild_index().unwrap();
    assert_eq!(store.search(&scope, "answer", 8).unwrap(), results);
    drop(store);
    let raw = rusqlite::Connection::open(&path).unwrap();
    assert!(
        raw.execute(
            "UPDATE source_annotations SET reason='changed' WHERE dispatch=?1",
            [stale.dispatch.as_str()]
        )
        .is_err()
    );
    assert!(
        raw.execute(
            "DELETE FROM source_annotations WHERE dispatch=?1",
            [stale.dispatch.as_str()]
        )
        .is_err()
    );
    drop(raw);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn annotation_limit_rejects_new_entries_but_not_idempotent_retries() {
    let path = path();
    let scope = Scope::new("a", "p", "c").unwrap();
    let mut store = MemoryStore::create(&path).unwrap();
    store.grant(&scope).unwrap();
    let input = note("capped", "needle");
    store.put(&scope, &input).unwrap();
    for i in 0..8 {
        store
            .annotate_source(
                &scope,
                &annotation(
                    &format!("annotation-{i}"),
                    &input,
                    SourceAnnotationKind::Stale,
                ),
            )
            .unwrap();
    }
    assert_eq!(
        store.search(&scope, "needle", 8).unwrap()[0]
            .annotations
            .len(),
        8
    );
    assert!(
        store
            .annotate_source(
                &scope,
                &annotation("annotation-0", &input, SourceAnnotationKind::Stale)
            )
            .is_ok()
    );
    assert_eq!(
        store.annotate_source(
            &scope,
            &annotation("annotation-9", &input, SourceAnnotationKind::Stale)
        ),
        Err(MemoryError::InvalidInput)
    );
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn migration_is_atomic_and_preserves_entries_index_and_provenance() {
    let path = path();
    let scope = Scope::new("a", "p", "c").unwrap();
    let mut store = MemoryStore::create(&path).unwrap();
    store.grant(&scope).unwrap();
    let input = note("legacy", "legacy needle");
    let expected = store.put(&scope, &input).unwrap();
    drop(store);
    let raw = rusqlite::Connection::open(&path).unwrap();
    raw.execute_batch("DROP TABLE source_annotations; PRAGMA user_version=1001;")
        .unwrap();
    drop(raw);
    let mut migrated = MemoryStore::open_existing(&path).unwrap();
    assert_eq!(
        migrated.search(&scope, "needle", 8).unwrap(),
        vec![expected.clone()]
    );
    migrated.rebuild_index().unwrap();
    assert_eq!(
        migrated.search(&scope, "needle", 8).unwrap(),
        vec![expected]
    );
    drop(migrated);
    let raw = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = raw
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, 1002);
    raw.execute_batch("DROP TABLE source_annotations; CREATE TABLE source_annotations(blocker INTEGER); PRAGMA user_version=1001;").unwrap();
    drop(raw);
    assert!(matches!(
        MemoryStore::open_existing(&path),
        Err(MemoryError::Storage)
    ));
    let raw = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = raw
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, 1001);
    assert_eq!(
        raw.query_row("SELECT count(*) FROM entries", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    raw.execute_batch("PRAGMA user_version=9999;").unwrap();
    drop(raw);
    assert!(matches!(
        MemoryStore::open_existing(&path),
        Err(MemoryError::Integrity)
    ));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn conflicts_are_visible_at_both_endpoints_and_share_incident_limits() {
    let path = path();
    let scope = Scope::new("a", "p", "c").unwrap();
    let mut store = MemoryStore::create(&path).unwrap();
    store.grant(&scope).unwrap();
    let a = note("edge-a", "edge alpha");
    let b = note("edge-b", "edge beta");
    store.put(&scope, &a).unwrap();
    store.put(&scope, &b).unwrap();
    let conflict = annotation(
        "edge-0",
        &a,
        SourceAnnotationKind::ConflictsWith {
            related_dispatch: b.dispatch.clone(),
        },
    );
    let saved = store.annotate_source(&scope, &conflict).unwrap();
    let entries = store.search_candidates(&scope, "edge", 8).unwrap().entries;
    assert_eq!(entries[0].annotations, vec![saved.clone()]);
    assert_eq!(entries[1].annotations, vec![saved]);
    for i in 0..7 {
        let input = annotation(
            &format!("edge-b-stale-{i}"),
            &b,
            SourceAnnotationKind::Stale,
        );
        store.annotate_source(&scope, &input).unwrap();
    }
    assert_eq!(
        store.annotate_source(
            &scope,
            &annotation("edge-b-over", &b, SourceAnnotationKind::Stale)
        ),
        Err(MemoryError::InvalidInput)
    );
    let c = note("edge-c", "edge gamma");
    store.put(&scope, &c).unwrap();
    let new_link = annotation(
        "edge-c-to-b",
        &c,
        SourceAnnotationKind::ConflictsWith {
            related_dispatch: b.dispatch.clone(),
        },
    );
    assert_eq!(
        store.annotate_source(&scope, &new_link),
        Err(MemoryError::InvalidInput)
    );
    let page = store.search_candidates(&scope, "gamma", 8).unwrap();
    assert!(page.entries[0].annotations.is_empty());
    assert!(store.annotate_source(&scope, &conflict).is_ok());
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn candidate_pages_return_complete_large_entries_and_truthful_has_more() {
    let path = path();
    let scope = Scope::new("a", "p", "c").unwrap();
    let other = Scope::new("a", "other", "c").unwrap();
    let mut store = MemoryStore::create(&path).unwrap();
    store.grant(&scope).unwrap();
    let large = note("large-candidate", &format!("needle {}", "x".repeat(4089)));
    assert_eq!(large.body.len(), 4096);
    store.put(&scope, &large).unwrap();
    let mut status = annotation("large-status", &large, SourceAnnotationKind::Stale);
    status.reason = "r".repeat(512);
    store.annotate_source(&scope, &status).unwrap();
    store
        .put(&scope, &note("next-candidate", "needle next"))
        .unwrap();
    assert!(store.search(&scope, "needle", 8).unwrap().is_empty());
    let one = store.search_candidates(&scope, "needle", 1).unwrap();
    assert_eq!(one.requested_limit, 1);
    assert!(one.has_more);
    assert_eq!(one.entries.len(), 1);
    assert_eq!(one.entries[0].body, large.body);
    assert_eq!(one.entries[0].annotations[0].reason.len(), 512);
    let all = store.search_candidates(&scope, "needle", 8).unwrap();
    assert!(!all.has_more);
    assert_eq!(all.entries.len(), 2);
    assert_eq!(
        store.search_candidates(&other, "needle", 1),
        Err(MemoryError::Denied)
    );
    store.grant(&other).unwrap();
    assert!(
        store
            .search_candidates(&other, "needle", 1)
            .unwrap()
            .entries
            .is_empty()
    );
    assert_eq!(
        store.search_candidates(&scope, "needle", 0),
        Err(MemoryError::InvalidInput)
    );
    assert_eq!(
        store.search_candidates(&scope, "needle", 9),
        Err(MemoryError::InvalidInput)
    );
    store.revoke(&scope).unwrap();
    assert_eq!(
        store.search_candidates(&scope, "needle", 1),
        Err(MemoryError::Denied)
    );
    store.grant(&scope).unwrap();
    drop(store);
    let mut store = MemoryStore::open_existing(&path).unwrap();
    assert_eq!(store.search_candidates(&scope, "needle", 8).unwrap(), all);
    drop(store);
    std::fs::remove_file(path).unwrap();
}
