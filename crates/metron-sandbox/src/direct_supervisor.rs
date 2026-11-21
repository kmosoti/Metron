//! Hardened direct process supervisor (for environments without `systemd-run --user`)
//! and shared bounded stream capture / scratch disk-space reserve enforcement.

use crate::{
    error::SandboxError,
    jail_hardening::SandboxReceipt,
    systemd_cg::{CgroupCompleted, CgroupLimits},
};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

/// Resolves a host manager binary (`systemd-run`, `systemctl`, `timeout`) via an explicit
/// environment override, then `PATH`, then standard Linux / NixOS system directories.
#[must_use]
pub fn resolve_manager_tool(env_override: &str, name: &str) -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var(env_override)
        && !explicit.is_empty()
    {
        let p = PathBuf::from(explicit);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    for dir in ["/run/current-system/sw/bin", "/usr/bin", "/bin"] {
        let candidate = Path::new(dir).join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Verifies `scratch` has strictly more than `required_bytes` free (when non-zero).
///
/// Returns `SandboxError::InsufficientScratchSpace` (never a timeout) when exhausted.
pub fn check_scratch_space(scratch: &Path, required_bytes: u64) -> Result<(), SandboxError> {
    if required_bytes == 0 {
        return Ok(());
    }
    let stat = rustix::fs::statvfs(scratch)
        .map_err(|_| SandboxError::Denied("scratch statvfs unavailable".into()))?;
    let available_bytes = stat.f_bavail.saturating_mul(stat.f_frsize);
    if available_bytes <= required_bytes {
        return Err(SandboxError::InsufficientScratchSpace {
            available_bytes,
            required_bytes,
        });
    }
    Ok(())
}

/// Spawns `bin` directly with bounded stdout/stderr capture, stdin streaming,
/// real wall-clock deadline + `SIGKILL` (`timed_out: true` when killed), and scratch
/// disk-space reserve monitoring (`SandboxError::InsufficientScratchSpace`).
pub fn run_direct_supervised(
    limits: &CgroupLimits,
    scratch: &Path,
    bin: &Path,
    args: &[String],
    stdin_file: Option<&File>,
    receipt_path: Option<&Path>,
) -> Result<CgroupCompleted, SandboxError> {
    run_direct_supervised_with_prefix(limits, scratch, bin, args, None, stdin_file, receipt_path)
}

/// Variant of [`run_direct_supervised`] that prepends an in-memory byte prefix
/// (for example a 32-byte HMAC key) ahead of `stdin_file` on the anonymous stdin pipe
/// so no secrets ever touch scratch disk.
pub fn run_direct_supervised_with_prefix(
    limits: &CgroupLimits,
    scratch: &Path,
    bin: &Path,
    args: &[String],
    stdin_prefix: Option<Vec<u8>>,
    stdin_file: Option<&File>,
    receipt_path: Option<&Path>,
) -> Result<CgroupCompleted, SandboxError> {
    run_direct_supervised_cancellable(
        limits,
        scratch,
        bin,
        args,
        stdin_prefix,
        stdin_file,
        receipt_path,
        None,
    )
}

/// Cancellable variant of [`run_direct_supervised_with_prefix`]: when `cancel` flips `true`,
/// immediately sends `SIGKILL` and reaps the child.
#[allow(clippy::too_many_arguments)]
pub fn run_direct_supervised_cancellable(
    limits: &CgroupLimits,
    scratch: &Path,
    bin: &Path,
    args: &[String],
    stdin_prefix: Option<Vec<u8>>,
    stdin_file: Option<&File>,
    receipt_path: Option<&Path>,
    cancel: Option<&AtomicBool>,
) -> Result<CgroupCompleted, SandboxError> {
    check_scratch_space(scratch, limits.scratch_reserve_bytes)?;

    let mut cmd = Command::new(bin);
    cmd.args(args)
        .env_clear()
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if stdin_prefix.is_some() || stdin_file.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| SandboxError::Denied(format!("jail launch failed: {e}")))?;

    let stdin_writer =
        spawn_stdin_writer_with_prefix(stdin_prefix, stdin_file, child.stdin.take())?;
    let stdout = capture_stream(
        child
            .stdout
            .take()
            .ok_or_else(|| SandboxError::Uncertain("stdout unavailable".into()))?,
        limits.output_limit,
    );
    let stderr = capture_stream(
        child
            .stderr
            .take()
            .ok_or_else(|| SandboxError::Uncertain("stderr unavailable".into()))?,
        limits.output_limit,
    );

    let start = Instant::now();
    let deadline = Duration::from_millis(u64::from(limits.timeout_ms));
    let mut timed_out = false;
    let mut exit_code = None;
    let mut disk_err: Option<SandboxError> = None;

    loop {
        if cancel.is_some_and(|c| c.load(Ordering::Acquire)) {
            timed_out = true;
            kill_child_tree(&mut child);
            break;
        }
        if let Err(err) = check_scratch_space(scratch, limits.scratch_reserve_bytes) {
            kill_child_tree(&mut child);
            disk_err = Some(err);
            break;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                exit_code = status.code();
                break;
            }
            Ok(None) if start.elapsed() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => {
                timed_out = true;
                kill_child_tree(&mut child);
                break;
            }
        }
    }

    if let Some(writer) = stdin_writer {
        let _ = writer.join();
    }

    if let Some(err) = disk_err {
        return Err(err);
    }

    let (out, err, truncated) = collect_streams(stdout, stderr)?;
    let receipt = receipt_path.and_then(read_receipt);
    let success = !timed_out && exit_code == Some(0);
    Ok(CgroupCompleted {
        stdout: out,
        stderr: err,
        truncated,
        exit_code: if timed_out { None } else { exit_code },
        timed_out,
        success,
        receipt,
    })
}

fn kill_child_tree(child: &mut std::process::Child) {
    if let Ok(raw) = i32::try_from(child.id())
        && let Some(pid) = rustix::process::Pid::from_raw(raw)
    {
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Spawns a background thread streaming `stdin_file` (rewound to offset 0) into the child's stdin pipe.
pub fn spawn_stdin_writer(
    stdin_file: Option<&File>,
    pipe: Option<std::process::ChildStdin>,
) -> Result<Option<thread::JoinHandle<std::io::Result<()>>>, SandboxError> {
    spawn_stdin_writer_with_prefix(None, stdin_file, pipe)
}

/// Spawns a background thread streaming optional in-memory `prefix` followed by `stdin_file`
/// (rewound to offset 0) into the child's anonymous stdin pipe.
pub fn spawn_stdin_writer_with_prefix(
    prefix: Option<Vec<u8>>,
    stdin_file: Option<&File>,
    pipe: Option<std::process::ChildStdin>,
) -> Result<Option<thread::JoinHandle<std::io::Result<()>>>, SandboxError> {
    if prefix.is_none() && stdin_file.is_none() {
        return Ok(None);
    }
    let cloned = match stdin_file {
        Some(src) => {
            let mut f = src
                .try_clone()
                .map_err(|_| SandboxError::Uncertain("stdin memfd clone failed".into()))?;
            f.seek(SeekFrom::Start(0))
                .map_err(|_| SandboxError::Uncertain("stdin memfd seek failed".into()))?;
            Some(f)
        }
        None => None,
    };
    let mut pipe =
        pipe.ok_or_else(|| SandboxError::Uncertain("child stdin pipe unavailable".into()))?;
    Ok(Some(thread::spawn(move || -> std::io::Result<()> {
        if let Some(bytes) = prefix {
            pipe.write_all(&bytes)?;
        }
        if let Some(mut f) = cloned {
            std::io::copy(&mut f, &mut pipe)?;
        }
        pipe.flush()
    })))
}

/// Joins bounded stdout/stderr reader threads with a 500ms close deadline.
pub fn collect_streams(
    stdout: thread::JoinHandle<std::io::Result<(Vec<u8>, bool)>>,
    stderr: thread::JoinHandle<std::io::Result<(Vec<u8>, bool)>>,
) -> Result<(Vec<u8>, Vec<u8>, bool), SandboxError> {
    let finish = Instant::now();
    while !(stdout.is_finished() && stderr.is_finished()) {
        if finish.elapsed() > Duration::from_millis(500) {
            return Err(SandboxError::Uncertain("output pipes remain open".into()));
        }
        thread::sleep(Duration::from_millis(5));
    }
    let (out, trunc_out) = stdout
        .join()
        .map_err(|_| SandboxError::Uncertain("output capture failed".into()))?
        .map_err(|_| SandboxError::Uncertain("output capture failed".into()))?;
    let (err, trunc_err) = stderr
        .join()
        .map_err(|_| SandboxError::Uncertain("output capture failed".into()))?
        .map_err(|_| SandboxError::Uncertain("output capture failed".into()))?;
    Ok((out, err, trunc_out || trunc_err))
}

/// Reads a `SandboxReceipt` JSON file emitted by `metron-jail`, returning `None` if absent.
#[must_use]
pub fn read_receipt(path: &Path) -> Option<SandboxReceipt> {
    let raw = std::fs::read(path).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// Spawns a background reader thread capturing up to `limit` bytes from `input`.
pub fn capture_stream(
    mut input: impl Read + Send + 'static,
    limit: usize,
) -> thread::JoinHandle<std::io::Result<(Vec<u8>, bool)>> {
    thread::spawn(move || {
        let mut output = Vec::new();
        let mut truncated = false;
        let mut buf = [0u8; 8192];
        loop {
            let n = input.read(&mut buf)?;
            if n == 0 {
                break;
            }
            let take = (limit - output.len()).min(n);
            output.extend_from_slice(&buf[..take]);
            truncated |= take < n;
        }
        Ok((output, truncated))
    })
}
