//! Sealed `memfd` image pinning and disk-backed scratch checks (zero disk staging).

use crate::error::SandboxError;
use rustix::{
    fs::{MemfdFlags, Mode, OFlags, SealFlags, fcntl_add_seals, memfd_create, open, statfs},
    io::{FdFlags, fcntl_setfd},
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    os::fd::{AsRawFd, BorrowedFd},
    path::{Path, PathBuf},
};

/// Maximum sealed executable size (512 MiB).
pub const MAX_EXECUTABLE_BYTES: u64 = 512 * 1024 * 1024;

const TMPFS_MAGIC: u64 = 0x0102_1994;
const RAMFS_MAGIC: u64 = 0x8584_58f6;

/// An immutable, `F_SEAL_WRITE`-protected executable kept solely in memory.
pub struct SealedMemfd {
    file: File,
    proc_path: PathBuf,
    sha256: String,
    size: u64,
}

impl SealedMemfd {
    /// Returns `/proc/self/fd/<n>` (valid inside the owning process or after `CLOEXEC` clear).
    #[must_use]
    pub fn self_fd_path(&self) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.file.as_raw_fd()))
    }

    /// Returns `/proc/<pid>/fd/<n>` for the creating process.
    #[must_use]
    pub fn proc_path(&self) -> &Path {
        &self.proc_path
    }

    /// Returns the raw file descriptor number.
    #[must_use]
    pub fn raw_fd(&self) -> i32 {
        self.file.as_raw_fd()
    }

    /// Returns the SHA-256 hex digest computed while loading and sealing the `memfd`.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// Returns the sealed byte length.
    #[must_use]
    pub fn len(&self) -> u64 {
        self.size
    }

    /// Returns `true` if the sealed `memfd` is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// Borrows the underlying file handle.
    #[must_use]
    pub fn as_file(&self) -> &File {
        &self.file
    }

    /// Clears `FD_CLOEXEC` so a child spawned via `exec` inherits this sealed descriptor.
    pub fn clear_cloexec(&self) -> Result<(), SandboxError> {
        fcntl_setfd(&self.file, FdFlags::empty())
            .map_err(|_| SandboxError::Denied("failed to clear FD_CLOEXEC on memfd".into()))
    }

    /// Rewinds and clones a read-only view of the sealed bytes.
    pub fn read_all(&self) -> Result<Vec<u8>, SandboxError> {
        let mut cloned = self
            .file
            .try_clone()
            .map_err(|_| SandboxError::Denied("memfd handle unavailable".into()))?;
        cloned
            .seek(SeekFrom::Start(0))
            .map_err(|_| SandboxError::Denied("memfd seek failed".into()))?;
        let mut out = Vec::with_capacity(self.size.min(16 * 1024 * 1024) as usize);
        cloned
            .take(MAX_EXECUTABLE_BYTES + 1)
            .read_to_end(&mut out)
            .map_err(|_| SandboxError::Denied("memfd read failed".into()))?;
        Ok(out)
    }
}

/// Rejects RAM-backed scratch directories (`tmpfs` / `ramfs`).
pub fn require_disk_backed(path: &Path) -> Result<(), SandboxError> {
    let fs =
        statfs(path).map_err(|_| SandboxError::Denied("scratch filesystem unavailable".into()))?;
    let kind = fs.f_type as u64;
    if kind == TMPFS_MAGIC || kind == RAMFS_MAGIC {
        return Err(SandboxError::Denied("RAM-backed scratch denied".into()));
    }
    Ok(())
}

/// Computes the SHA-256 hex digest of a regular file opened without following symlinks.
pub fn sha256_regular_file(path: &Path) -> Result<String, SandboxError> {
    let source = open_regular_readonly(path)?;
    let mut hasher = Sha256::new();
    let mut reader = source.take(MAX_EXECUTABLE_BYTES + 1);
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|_| SandboxError::Denied("executable read failed".into()))?;
        if n == 0 {
            break;
        }
        total = total.saturating_add(n as u64);
        if total > MAX_EXECUTABLE_BYTES {
            return Err(SandboxError::Denied("executable exceeds size bound".into()));
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

/// Opens a regular file (`O_NOFOLLOW | O_NONBLOCK`), copies into a sealed executable `memfd`,
/// and verifies an optional expected SHA-256 hex digest before adding immutable seals.
pub fn open_and_seal_regular(
    path: &Path,
    name: &str,
    expected_sha256: Option<&str>,
    cloexec: bool,
) -> Result<SealedMemfd, SandboxError> {
    let source = open_regular_readonly(path)?;
    let meta = source
        .metadata()
        .map_err(|_| SandboxError::Denied("executable metadata unavailable".into()))?;
    if !meta.is_file() || meta.len() > MAX_EXECUTABLE_BYTES {
        return Err(SandboxError::Denied("invalid executable".into()));
    }
    seal_from_reader(name, source, expected_sha256, cloexec)
}

/// Copies an in-memory byte slice into an `F_SEAL_WRITE`-sealed executable `memfd`.
pub fn seal_bytes(
    name: &str,
    bytes: &[u8],
    expected_sha256: Option<&str>,
    cloexec: bool,
) -> Result<SealedMemfd, SandboxError> {
    if bytes.len() as u64 > MAX_EXECUTABLE_BYTES {
        return Err(SandboxError::Denied("executable exceeds size bound".into()));
    }
    seal_from_reader(name, bytes, expected_sha256, cloexec)
}

/// Copies a reader into a `memfd`, verifies optional SHA-256, and seals all write/resize flags.
pub fn seal_from_reader(
    name: &str,
    reader: impl Read,
    expected_sha256: Option<&str>,
    cloexec: bool,
) -> Result<SealedMemfd, SandboxError> {
    let flags = MemfdFlags::ALLOW_SEALING
        | MemfdFlags::EXEC
        | if cloexec {
            MemfdFlags::CLOEXEC
        } else {
            MemfdFlags::empty()
        };
    let mut memfd = File::from(
        memfd_create(name, flags)
            .map_err(|_| SandboxError::Denied("sealed executable memfd unavailable".into()))?,
    );
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut limited = reader.take(MAX_EXECUTABLE_BYTES + 1);
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = limited
            .read(&mut buf)
            .map_err(|_| SandboxError::Denied("executable copy failed".into()))?;
        if n == 0 {
            break;
        }
        total = total.saturating_add(n as u64);
        if total > MAX_EXECUTABLE_BYTES {
            return Err(SandboxError::Denied("executable exceeds limit".into()));
        }
        hasher.update(&buf[..n]);
        memfd
            .write_all(&buf[..n])
            .map_err(|_| SandboxError::Denied("memfd write failed".into()))?;
    }
    let sha256 = hex_lower(&hasher.finalize());
    if let Some(expected) = expected_sha256
        && sha256 != expected
    {
        return Err(SandboxError::Denied("executable digest mismatch".into()));
    }
    memfd
        .seek(SeekFrom::Start(0))
        .map_err(|_| SandboxError::Denied("memfd rewind failed".into()))?;
    fcntl_add_seals(
        &memfd,
        SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL,
    )
    .map_err(|_| SandboxError::Denied("executable seal failed".into()))?;
    let proc_path = PathBuf::from(format!(
        "/proc/{}/fd/{}",
        std::process::id(),
        memfd.as_raw_fd()
    ));
    Ok(SealedMemfd {
        file: memfd,
        proc_path,
        sha256,
        size: total,
    })
}

/// Duplicates `source` over standard input (FD 0) safely.
pub fn replace_stdin_with(source: BorrowedFd<'_>) -> Result<(), SandboxError> {
    rustix::stdio::dup2_stdin(source)
        .map_err(|_| SandboxError::Denied("failed to bind sanitized stdin".into()))
}

/// Opens `path` without following symlinks or blocking, verifies it is a regular file,
/// and reads at most `max_bytes` bytes (failing closed if exceeded).
pub fn read_bounded_regular_file(
    path: &Path,
    max_bytes: u64,
    label: &str,
) -> Result<Vec<u8>, SandboxError> {
    let file = File::from(
        open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| SandboxError::Denied(format!("{label} unreadable")))?,
    );
    let meta = file
        .metadata()
        .map_err(|_| SandboxError::Denied(format!("{label} metadata unavailable")))?;
    if !meta.is_file() {
        return Err(SandboxError::Denied(format!(
            "{label} must be a regular file"
        )));
    }
    let cap = usize::try_from(meta.len().min(max_bytes).min(1024 * 1024)).unwrap_or(0);
    let mut buf = Vec::with_capacity(cap);
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut buf)
        .map_err(|_| SandboxError::Denied(format!("{label} read failed")))?;
    if buf.len() as u64 > max_bytes {
        return Err(SandboxError::Denied(format!("{label} exceeds size bound")));
    }
    Ok(buf)
}

fn open_regular_readonly(path: &Path) -> Result<File, SandboxError> {
    let file = File::from(
        open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| SandboxError::Denied("pinned executable unavailable".into()))?,
    );
    let meta = file
        .metadata()
        .map_err(|_| SandboxError::Denied("executable metadata unavailable".into()))?;
    if !meta.is_file() {
        return Err(SandboxError::Denied(
            "non-regular executable rejected".into(),
        ));
    }
    Ok(file)
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::read_bounded_regular_file;
    use std::path::Path;

    #[test]
    fn read_bounded_regular_file_rejects_non_regular_and_oversized() {
        assert!(read_bounded_regular_file(Path::new("/dev/null"), 64, "dev").is_err());
        assert!(read_bounded_regular_file(Path::new("."), 64, "dir").is_err());
        let cargo_toml = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(read_bounded_regular_file(&cargo_toml, 8, "small").is_err());
        assert!(read_bounded_regular_file(&cargo_toml, 64 * 1024, "ok").is_ok());
    }
}
