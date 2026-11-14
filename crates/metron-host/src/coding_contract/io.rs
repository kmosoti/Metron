//! Descriptor-pinned external I/O (`openat2` NO_SYMLINKS | BENEATH), bounded
//! descriptor workspace snapshots, and typed-enum read-only SQLite predecessor inspection.

use super::types::MAX_BASELINE_TOTAL_BYTES;
use crate::coding_tools;
use metron_protocol::{
    AuditSnapshot, Digest, MAX_INTERNAL, RuntimeSnapshot, StorePort, TaskId, canonical_bytes,
    content_digest, parse_internal,
    store::{DispatchStatus, ReservationStatus},
};
use metron_store_sqlite::SqliteStore;
use rustix::{
    fs::{AtFlags, Mode, OFlags, ResolveFlags, mkdirat, open, openat, openat2, unlinkat},
    io::Errno,
};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};

fn normalize_no_dot_path(path: &Path) -> Result<(PathBuf, PathBuf), String> {
    if path.as_os_str().is_empty() {
        return Err("external path must not be empty".into());
    }
    if path.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::CurDir | Component::Prefix(_)
        )
    }) {
        return Err(format!(
            "external path must not contain '.' or '..': {}",
            path.display()
        ));
    }
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| format!("current working directory: {e}"))?
            .join(path)
    };
    if abs.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::CurDir | Component::Prefix(_)
        )
    }) {
        return Err(format!(
            "normalized path must not contain '.' or '..': {}",
            abs.display()
        ));
    }
    let rel: PathBuf = abs
        .components()
        .filter_map(|c| match c {
            Component::Normal(part) => Some(part),
            _ => None,
        })
        .collect();
    if rel.as_os_str().is_empty() {
        return Err("external path cannot target filesystem root directly".into());
    }
    Ok((abs, rel))
}

fn open_slash() -> Result<File, String> {
    open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|e| format!("open '/': {e}"))
}

/// Pin an existing regular file or directory from `/` via `openat2(..., BENEATH | NO_SYMLINKS)`.
/// Rejects symlinks or `..` across every path component without TOCTOU path re-resolution.
pub(crate) fn open_external_nofollow(path: &Path, dir: bool) -> Result<(File, PathBuf), String> {
    let (abs, rel) = normalize_no_dot_path(path)?;
    let slash = open_slash()?;
    let flags = if dir {
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC
    } else {
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC
    };
    let fd = openat2(
        &slash,
        &rel,
        flags,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )
    .map_err(|e| {
        format!(
            "unsafe or inaccessible external path {}: {e}",
            abs.display()
        )
    })?;
    Ok((File::from(fd), abs))
}

/// Verify via pinned file descriptors (`..` chain comparing `(st_dev, st_ino)`) that
/// `target_dir` does not equal or descend from `forbidden_dir`.
pub(crate) fn ensure_dir_outside(
    target_dir: &File,
    forbidden_dir: &File,
    label: &str,
) -> Result<(), String> {
    let forbidden_meta = forbidden_dir
        .metadata()
        .map_err(|e| format!("forbidden dir metadata: {e}"))?;
    if !forbidden_meta.is_dir() {
        return Err("forbidden confinement root must be a directory".into());
    }
    let forbidden_id = (forbidden_meta.dev(), forbidden_meta.ino());
    let mut cur = target_dir
        .try_clone()
        .map_err(|e| format!("clone descriptor: {e}"))?;
    for _depth in 0..512 {
        let meta = cur.metadata().map_err(|e| format!("dir metadata: {e}"))?;
        if !meta.is_dir() {
            return Err("descriptor confinement expected directory".into());
        }
        let id = (meta.dev(), meta.ino());
        if id == forbidden_id {
            return Err(format!("external path resolves inside {label}"));
        }
        let parent = File::from(
            openat(
                &cur,
                "..",
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|e| format!("walk parent descriptor: {e}"))?,
        );
        let parent_meta = parent
            .metadata()
            .map_err(|e| format!("parent metadata: {e}"))?;
        let parent_id = (parent_meta.dev(), parent_meta.ino());
        if parent_id == id {
            return Ok(());
        }
        cur = parent;
    }
    Err("directory hierarchy exceeded descriptor traversal depth limit".into())
}

fn ensure_file_outside_workspace(
    file: &File,
    abs_path: &Path,
    workspace_dir: Option<&File>,
) -> Result<(), String> {
    let Some(ws) = workspace_dir else {
        return Ok(());
    };
    let parent_path = abs_path
        .parent()
        .ok_or_else(|| "external file has no parent".to_string())?;
    let (parent_dir, _) = if parent_path == Path::new("/") {
        (open_slash()?, PathBuf::from("/"))
    } else {
        open_external_nofollow(parent_path, true)?
    };
    ensure_dir_outside(&parent_dir, ws, "the candidate workspace")?;
    let leaf_name = abs_path
        .file_name()
        .ok_or_else(|| "external file missing leaf name".to_string())?;
    let verified_leaf = File::from(
        openat(
            &parent_dir,
            leaf_name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| format!("re-verify leaf under parent: {e}"))?,
    );
    let f_meta = file.metadata().map_err(|e| format!("file metadata: {e}"))?;
    let v_meta = verified_leaf
        .metadata()
        .map_err(|e| format!("verified metadata: {e}"))?;
    if (f_meta.dev(), f_meta.ino()) != (v_meta.dev(), v_meta.ino()) {
        return Err("external file changed during descriptor verification".into());
    }
    Ok(())
}

/// Read exact bounded bytes from an already-pinned `File` descriptor using `take(max + 1)`
/// so concurrent growth never exhausts memory or silently truncates. Rejects non-regular files
/// and any hardlinked file (`nlink != 1`) to block outside sensitive-file hardlink aliasing.
pub(crate) fn read_open_file_bounded(file: File, max_bytes: usize) -> Result<Vec<u8>, String> {
    let meta = file.metadata().map_err(|e| format!("file metadata: {e}"))?;
    if !meta.is_file() {
        return Err("external input must be a regular file".into());
    }
    if meta.nlink() != 1 {
        return Err(format!(
            "external input must not have multiple hardlinks (nlink={})",
            meta.nlink()
        ));
    }
    if meta.len() > max_bytes as u64 {
        return Err(format!("external input exceeds {max_bytes} byte limit"));
    }
    let mut bytes = Vec::new();
    file.take((max_bytes as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read bounded external file: {e}"))?;
    if bytes.len() > max_bytes {
        return Err(format!(
            "external input exceeded {max_bytes} byte limit while reading"
        ));
    }
    Ok(bytes)
}

pub(crate) fn read_external_bounded_bytes(
    path: &Path,
    workspace_dir: Option<&File>,
    max_bytes: usize,
) -> Result<(PathBuf, Vec<u8>, Digest), String> {
    let (file, abs) = open_external_nofollow(path, false)?;
    ensure_file_outside_workspace(&file, &abs, workspace_dir)?;
    let bytes = read_open_file_bounded(file, max_bytes)?;
    let digest = content_digest(&bytes);
    Ok((abs, bytes, digest))
}

pub(crate) fn read_external_json<T: DeserializeOwned>(
    path: &Path,
    workspace_dir: Option<&File>,
) -> Result<(PathBuf, T, Digest), String> {
    let (resolved, bytes, digest) = read_external_bounded_bytes(path, workspace_dir, MAX_INTERNAL)?;
    let value: T = parse_internal(&bytes)
        .map_err(|e| format!("invalid bounded JSON in {}: {e:?}", resolved.display()))?;
    Ok((resolved, value, digest))
}

/// Create an immutable external artifact without following any ancestor/leaf symlink or
/// replacing existing files: pins parent via `openat2` from `/`, verifies outside forbidden
/// dirs, creates leaf with `O_CREAT | O_EXCL | O_NOFOLLOW`, syncs file, and syncs parent.
pub(crate) fn write_external_new_json<T: Serialize>(
    path: &Path,
    workspace_dir: Option<&File>,
    forbidden_dirs: &[(&File, &str)],
    value: &T,
) -> Result<(PathBuf, Digest), String> {
    let bytes = canonical_bytes(value, MAX_INTERNAL)
        .map_err(|e| format!("canonical contract serialization failed: {e:?}"))?;
    let (abs, _) = normalize_no_dot_path(path)?;
    let parent_path = abs
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| format!("external output missing parent: {}", abs.display()))?;
    let leaf_name = abs
        .file_name()
        .ok_or_else(|| format!("external output missing file name: {}", abs.display()))?;
    let (parent_dir, _) = if parent_path == Path::new("/") {
        (open_slash()?, PathBuf::from("/"))
    } else {
        open_external_nofollow(parent_path, true)?
    };
    if let Some(ws) = workspace_dir {
        ensure_dir_outside(&parent_dir, ws, "the candidate workspace")?;
    }
    for (dir, label) in forbidden_dirs {
        ensure_dir_outside(&parent_dir, dir, label)?;
    }

    let mut leaf = File::from(
        openat(
            &parent_dir,
            leaf_name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|e| {
            format!(
                "immutable external create failed for {}: {e}",
                abs.display()
            )
        })?,
    );
    leaf.write_all(&bytes)
        .and_then(|()| leaf.sync_all())
        .map_err(|e| format!("write {}: {e}", abs.display()))?;
    parent_dir
        .sync_all()
        .map_err(|e| format!("sync parent of {}: {e}", abs.display()))?;
    Ok((abs, content_digest(&bytes)))
}

/// Create a fresh `0o700` external directory under a symlink-free parent outside forbidden dirs.
pub(crate) fn create_external_private_dir(
    path: &Path,
    workspace_dir: Option<&File>,
    forbidden_dirs: &[(&File, &str)],
) -> Result<(File, PathBuf), String> {
    let (abs, _) = normalize_no_dot_path(path)?;
    let parent_path = abs
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| format!("external dir missing parent: {}", abs.display()))?;
    let leaf_name = abs
        .file_name()
        .ok_or_else(|| format!("external dir missing leaf name: {}", abs.display()))?;
    let (parent_dir, _) = if parent_path == Path::new("/") {
        (open_slash()?, PathBuf::from("/"))
    } else {
        open_external_nofollow(parent_path, true)?
    };
    if let Some(ws) = workspace_dir {
        ensure_dir_outside(&parent_dir, ws, "the candidate workspace")?;
    }
    for (dir, label) in forbidden_dirs {
        ensure_dir_outside(&parent_dir, dir, label)?;
    }
    mkdirat(&parent_dir, leaf_name, Mode::from_raw_mode(0o700))
        .map_err(|e| format!("create private external dir {}: {e}", abs.display()))?;
    parent_dir
        .sync_all()
        .map_err(|e| format!("sync parent of {}: {e}", abs.display()))?;
    let dir_fd = File::from(
        openat(
            &parent_dir,
            leaf_name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| format!("open private external dir {}: {e}", abs.display()))?,
    );
    Ok((dir_fd, abs))
}

/// Create a fresh `0o700` isolated workspace directory outside forbidden paths and
/// populate relative files safely via descriptor-relative `mkdirat`/`openat(..., O_CREAT | O_EXCL | O_NOFOLLOW)`.
pub(crate) fn seed_candidate_workspace(
    dir_path: &Path,
    forbidden_dirs: &[(&File, &str)],
    files: &BTreeMap<String, String>,
) -> Result<(File, PathBuf), String> {
    let (root_fd, root_abs) = create_external_private_dir(dir_path, None, forbidden_dirs)?;
    for (rel_path, content) in files {
        if rel_path.contains('\\') || rel_path.starts_with('/') {
            return Err(format!("invalid baseline file path '{rel_path}'"));
        }
        let segments: Vec<&str> = rel_path.split('/').collect();
        if segments.is_empty()
            || segments
                .iter()
                .any(|s| s.is_empty() || *s == "." || *s == "..")
        {
            return Err(format!("invalid baseline file path '{rel_path}'"));
        }
        let mut cur_fd = root_fd
            .try_clone()
            .map_err(|e| format!("clone workspace root fd: {e}"))?;
        for dir_seg in &segments[..segments.len() - 1] {
            match mkdirat(&cur_fd, *dir_seg, Mode::from_raw_mode(0o700)) {
                Ok(()) => {}
                Err(Errno::EXIST) => {}
                Err(e) => {
                    return Err(format!(
                        "create workspace subdir '{dir_seg}' in {}: {e}",
                        root_abs.display()
                    ));
                }
            }
            cur_fd = File::from(
                openat(
                    &cur_fd,
                    *dir_seg,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|e| {
                    format!(
                        "open workspace subdir '{dir_seg}' in {}: {e}",
                        root_abs.display()
                    )
                })?,
            );
        }
        let leaf = segments[segments.len() - 1];
        let mut file = File::from(
            openat(
                &cur_fd,
                leaf,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o600),
            )
            .map_err(|e| format!("create workspace file '{rel_path}': {e}"))?,
        );
        file.write_all(content.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|e| format!("write workspace file '{rel_path}': {e}"))?;
    }
    root_fd
        .sync_all()
        .map_err(|e| format!("sync seeded workspace {}: {e}", root_abs.display()))?;
    Ok((root_fd, root_abs))
}

pub(crate) fn unlink_pinned_leaf(dir_fd: &File, leaf_name: &str) -> Result<(), String> {
    unlinkat(dir_fd, leaf_name, AtFlags::empty())
        .map_err(|e| format!("unlink pinned file '{leaf_name}': {e}"))
}

/// Snapshot workspace files via the descriptor-relative `openat2` walker in `coding_tools`
/// (enforces `<= 1024` entries, `<= 128` files, `<= 4 MiB`, `nlink == 1`, `NO_SYMLINKS`, `NO_XDEV`).
pub(crate) fn collect_workspace_snapshot(
    workspace: &Path,
) -> Result<(File, BTreeMap<String, Digest>), String> {
    let (ws_dir, _) = open_external_nofollow(workspace, true)?;
    let files = coding_tools::snapshot_workspace_files(&ws_dir, MAX_BASELINE_TOTAL_BYTES as u32)
        .map_err(|e| e.to_string())?;
    let mut map = BTreeMap::new();
    for file in files {
        map.insert(file.path.as_str().to_string(), file.digest);
    }
    Ok((ws_dir, map))
}

#[derive(Clone, Debug)]
pub(crate) struct PredecessorInspection {
    pub task_id: TaskId,
    pub replay_head: Digest,
    pub unknown_hold: bool,
    pub open_reservations: u32,
    pub unadmitted_pending_intents: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PinnedSqliteIdentity {
    pub dir_dev: u64,
    pub dir_ino: u64,
    pub db_dev: u64,
    pub db_ino: u64,
}

/// Pin `runtime.sqlite` (and any companion `-wal`/`-shm` files) inside an already-pinned
/// directory descriptor using `O_NOFOLLOW` and `nlink == 1` checks so symlink swaps or
/// hardlink redirects cannot redirect SQLite read-only WAL reads.
pub(crate) fn verify_pinned_sqlite_file(
    dir_fd: &File,
    dir_path: &Path,
) -> Result<PinnedSqliteIdentity, String> {
    let dir_meta = dir_fd
        .metadata()
        .map_err(|e| format!("sqlite dir metadata {}: {e}", dir_path.display()))?;
    if !dir_meta.is_dir() {
        return Err(format!(
            "sqlite path {} must be a directory",
            dir_path.display()
        ));
    }
    let db_fd = File::from(
        openat(
            dir_fd,
            "runtime.sqlite",
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| format!("open pinned runtime.sqlite in {}: {e}", dir_path.display()))?,
    );
    let db_meta = db_fd
        .metadata()
        .map_err(|e| format!("runtime.sqlite metadata {}: {e}", dir_path.display()))?;
    if !db_meta.is_file() {
        return Err("runtime.sqlite must be a regular file".into());
    }
    if db_meta.nlink() != 1 {
        return Err(format!(
            "runtime.sqlite must have nlink == 1 (got {})",
            db_meta.nlink()
        ));
    }
    for companion in ["runtime.sqlite-wal", "runtime.sqlite-shm"] {
        match openat(
            dir_fd,
            companion,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => {
                let comp_fd = File::from(fd);
                let comp_meta = comp_fd
                    .metadata()
                    .map_err(|e| format!("{companion} metadata: {e}"))?;
                if !comp_meta.is_file() || comp_meta.nlink() != 1 {
                    return Err(format!(
                        "SQLite companion '{companion}' must be a regular unaliased file"
                    ));
                }
            }
            Err(Errno::NOENT) => {}
            Err(e) => {
                return Err(format!(
                    "SQLite companion '{companion}' unsafe or inaccessible: {e}"
                ));
            }
        }
    }
    Ok(PinnedSqliteIdentity {
        dir_dev: dir_meta.dev(),
        dir_ino: dir_meta.ino(),
        db_dev: db_meta.dev(),
        db_ino: db_meta.ino(),
    })
}

/// Inspect predecessor run SQLite journal read-only via coherent SQLite WAL/audit/replay
/// and typed `DispatchStatus` / `ReservationStatus` enums without marker files, `immutable=1`,
/// or symlink-swap exposure. Explicitly separates unadmitted pending intents
/// (`Ready` / `AwaitingApproval` / `Leased`) from executed unknown/held dispatches.
pub(crate) fn inspect_predecessor_journal_read_only(
    run_dir: &Path,
    expected_task_id: Option<&TaskId>,
) -> Result<(File, PredecessorInspection), String> {
    let (dir_fd, dir_abs) = open_external_nofollow(run_dir, true)?;
    let before_id = verify_pinned_sqlite_file(&dir_fd, &dir_abs)?;
    let sqlite_path = dir_abs.join("runtime.sqlite");

    // Preserve read-only WAL visibility; never open with `immutable=1`.
    let mut store = SqliteStore::open_read_only(&sqlite_path)
        .map_err(|e| format!("predecessor store: {e:?}"))?;
    let ids = store
        .task_ids()
        .map_err(|e| format!("predecessor task ids: {e:?}"))?;
    if ids.len() != 1 {
        return Err("predecessor store must contain exactly one coding task".into());
    }
    let task_id = ids[0].clone();
    if let Some(expected) = expected_task_id
        && expected != &task_id
    {
        return Err(format!(
            "predecessor task_id mismatch: expected {}, found {}",
            expected.as_str(),
            task_id.as_str()
        ));
    }

    let mut snapshot = store
        .audit()
        .map_err(|e| format!("predecessor audit: {e:?}"))?;
    let projection = snapshot
        .projection(&task_id)
        .map_err(|e| format!("predecessor projection: {e:?}"))?;
    let engine =
        crate::cognition(&projection.genesis, &projection.task).map_err(|(_, e)| e.to_string())?;
    let checks = metron_harness::audit::verify(&mut snapshot, &task_id)
        .map_err(|e| format!("predecessor verify: {e:?}"))?;
    let behavior_ok = metron_harness::replay::behavior(&mut snapshot, &task_id, engine.as_ref())
        .map_err(|e| format!("predecessor replay: {e:?}"))?;
    if !checks.chain_valid || !checks.accounting_valid || !checks.projection_valid || !behavior_ok {
        return Err("predecessor journal failed read-only audit/replay verification".into());
    }

    let mut open_reservations = 0u32;
    let mut unadmitted_pending_intents = 0u32;
    let mut unknown_hold = projection.task_account.integrity_hold
        || projection.task_account.incomplete
        || projection.principal_account.integrity_hold
        || projection.principal_account.incomplete;
    let mut cursor = None;
    loop {
        let page = snapshot
            .intents(&task_id, cursor)
            .map_err(|e| format!("predecessor intents: {e:?}"))?;
        for intent in page.records {
            let reservation_held = intent
                .reservation
                .as_ref()
                .is_some_and(|r| r.status == ReservationStatus::Held);
            if reservation_held {
                open_reservations = open_reservations.saturating_add(1);
            }
            if matches!(
                intent.status,
                DispatchStatus::Ready | DispatchStatus::AwaitingApproval | DispatchStatus::Leased
            ) {
                unadmitted_pending_intents = unadmitted_pending_intents.saturating_add(1);
            }
            let executed_unknown = matches!(
                intent.status,
                DispatchStatus::Dispatched | DispatchStatus::Uncertain
            ) || intent.reconciliation_lease.is_some()
                || reservation_held;
            if executed_unknown {
                unknown_hold = true;
            }
        }
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    if projection.task_account.reserved.model_requests.get() > 0
        || projection.task_account.reserved.tool_calls.get() > 0
        || projection.task_account.reserved.spend_micros_usd.get() > 0
    {
        unknown_hold = true;
        if open_reservations == 0 {
            open_reservations = 1;
        }
    }

    let (after_dir_fd, _) = open_external_nofollow(run_dir, true)?;
    let after_id = verify_pinned_sqlite_file(&after_dir_fd, &dir_abs)?;
    if after_id != before_id {
        return Err(
            "predecessor SQLite directory or file identity changed during inspection".into(),
        );
    }

    Ok((
        dir_fd,
        PredecessorInspection {
            task_id,
            replay_head: projection.replay_head,
            unknown_hold,
            open_reservations,
            unadmitted_pending_intents,
        },
    ))
}
