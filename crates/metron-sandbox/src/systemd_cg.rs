//! Unprivileged `systemd-run --user --collect` cgroup v2 process runner and cleanup verifier.

use crate::cgroup_types::validate_unit_name;
pub use crate::cgroup_types::{CgroupCompleted, CgroupLimits};
pub use crate::direct_supervisor::resolve_manager_tool;
use crate::{
    direct_supervisor::{
        capture_stream, check_scratch_space, collect_streams, spawn_stdin_writer_with_prefix,
    },
    error::SandboxError,
};
use std::{
    fs::File,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

/// Checks whether unprivileged `systemd-run --user` cgroup management is active.
#[must_use]
pub fn systemd_user_available() -> bool {
    let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") else {
        return false;
    };
    if !runtime.starts_with('/')
        || runtime.contains('\0')
        || runtime.contains('\r')
        || runtime.contains('\n')
    {
        return false;
    }
    if resolve_manager_tool("METRON_SYSTEMD_RUN_BIN", "systemd-run").is_none() {
        return false;
    }
    systemctl(&runtime, &["show", "-p", "ControlGroup", "--value"])
        .is_some_and(|cg| cg.trim().starts_with('/'))
}

/// Constructs the sanitized `systemd-run --user --collect` command line without spawning.
pub fn build_systemd_run_command(
    runtime: &str,
    limits: &CgroupLimits,
    trampoline_bin: &Path,
    trampoline_args: &[String],
) -> Command {
    let secs = u64::from(limits.timeout_ms).div_ceil(1000).max(1);
    let systemd_run = resolve_manager_tool("METRON_SYSTEMD_RUN_BIN", "systemd-run")
        .unwrap_or_else(|| PathBuf::from("systemd-run"));
    let mut cmd = manager_command(&systemd_run, runtime);
    cmd.args([
        "--user",
        "--quiet",
        "--wait",
        "--pipe",
        "--collect",
        "--slice=app.slice",
    ])
    .arg(format!("--unit={}", limits.unit_name))
    .args([
        "--property=Type=exec",
        "--property=ExitType=cgroup",
        "--property=KillMode=control-group",
        "--property=TimeoutStopSec=2s",
        "--property=MemorySwapMax=0",
    ])
    .arg(format!(
        "--property=CPUQuota={}%",
        limits.cpu_quota_percent.max(1)
    ))
    .arg(format!(
        "--property=LimitFSIZE={}",
        limits.limit_fsize_bytes
    ))
    .arg(format!("--property=MemoryMax={}", limits.memory_max_bytes))
    .arg(format!("--property=TasksMax={}", limits.tasks_max.max(1)))
    .arg(format!(
        "--property=RuntimeMaxSec={}s",
        secs.saturating_add(1)
    ))
    .arg("--")
    .arg(trampoline_bin)
    .args(trampoline_args);
    cmd
}

/// Executes `trampoline_bin` inside an isolated `systemd-run --user --collect` cgroup v2 unit.
pub fn run_in_systemd_cgroup(
    limits: &CgroupLimits,
    scratch: &Path,
    trampoline_bin: &Path,
    trampoline_args: &[String],
    stdin_file: Option<&File>,
) -> Result<CgroupCompleted, SandboxError> {
    run_in_systemd_cgroup_with_prefix(
        limits,
        scratch,
        trampoline_bin,
        trampoline_args,
        None,
        stdin_file,
    )
}

/// Variant of [`run_in_systemd_cgroup`] prepending an in-memory byte prefix ahead of
/// `stdin_file` over the anonymous `--pipe` stdin stream (zero secrets on scratch disk).
pub fn run_in_systemd_cgroup_with_prefix(
    limits: &CgroupLimits,
    scratch: &Path,
    trampoline_bin: &Path,
    trampoline_args: &[String],
    stdin_prefix: Option<Vec<u8>>,
    stdin_file: Option<&File>,
) -> Result<CgroupCompleted, SandboxError> {
    run_in_systemd_cgroup_cancellable(
        limits,
        scratch,
        trampoline_bin,
        trampoline_args,
        stdin_prefix,
        stdin_file,
        None,
    )
}

/// Cancellable variant of [`run_in_systemd_cgroup_with_prefix`]: when `cancel` flips `true`,
/// immediately kills the transient systemd unit and child process tree.
pub fn run_in_systemd_cgroup_cancellable(
    limits: &CgroupLimits,
    scratch: &Path,
    trampoline_bin: &Path,
    trampoline_args: &[String],
    stdin_prefix: Option<Vec<u8>>,
    stdin_file: Option<&File>,
    cancel: Option<&AtomicBool>,
) -> Result<CgroupCompleted, SandboxError> {
    check_scratch_space(scratch, limits.scratch_reserve_bytes)?;

    let runtime = std::env::var("XDG_RUNTIME_DIR")
        .map_err(|_| SandboxError::Denied("user runtime unavailable".into()))?;
    if !runtime.starts_with('/')
        || runtime.contains('\0')
        || runtime.contains('\r')
        || runtime.contains('\n')
    {
        return Err(SandboxError::Denied("invalid user runtime".into()));
    }
    let manager = systemctl(&runtime, &["show", "-p", "ControlGroup", "--value"])
        .ok_or_else(|| SandboxError::Denied("user cgroup manager unavailable".into()))?;
    let manager = manager.trim();
    if !manager.starts_with('/')
        || manager.contains("..")
        || manager.contains('\0')
        || manager.contains('\r')
        || manager.contains('\n')
    {
        return Err(SandboxError::Denied("invalid manager cgroup".into()));
    }
    let cg_root = PathBuf::from("/sys/fs/cgroup").join(manager.trim_start_matches('/'));
    let events = std::fs::read_to_string(cg_root.join("cgroup.events"))
        .map_err(|_| SandboxError::Denied("cgroup v2 state unavailable".into()))?;
    if !events.lines().any(|line| line.starts_with("populated ")) {
        return Err(SandboxError::Denied("invalid cgroup v2 state".into()));
    }

    validate_unit_name(&limits.unit_name)?;
    let unit_name = &limits.unit_name;
    let cgroup = cg_root
        .join("app.slice")
        .join(format!("{unit_name}.service"));
    if cgroup.exists()
        || systemctl(&runtime, &["show", unit_name, "-p", "LoadState", "--value"])
            .is_some_and(|v| v.trim() != "not-found")
    {
        return Err(SandboxError::Denied("dispatch unit already exists".into()));
    }

    let mut cmd = build_systemd_run_command(&runtime, limits, trampoline_bin, trampoline_args);
    if stdin_prefix.is_some() || stdin_file.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|_| SandboxError::Denied("transient service launch unavailable".into()))?;

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
    let mut launcher_code = None;
    let mut disk_err: Option<SandboxError> = None;

    loop {
        if cancel.is_some_and(|c| c.load(Ordering::Acquire)) {
            timed_out = true;
            kill_unit(&runtime, unit_name);
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        if let Err(err) = check_scratch_space(scratch, limits.scratch_reserve_bytes) {
            kill_unit(&runtime, unit_name);
            let _ = child.kill();
            let _ = child.wait();
            disk_err = Some(err);
            break;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                launcher_code = status.code();
                break;
            }
            Ok(None) if start.elapsed() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => {
                timed_out = true;
                kill_unit(&runtime, unit_name);
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
        }
    }

    if let Some(writer) = stdin_writer {
        let _ = writer.join();
    }

    verify_cgroup_closed(&runtime, unit_name, &cgroup)?;

    if let Some(err) = disk_err {
        return Err(err);
    }

    let (out, err, truncated) = collect_streams(stdout, stderr)?;
    let success = !timed_out && launcher_code == Some(0);
    Ok(CgroupCompleted {
        stdout: out,
        stderr: err,
        truncated,
        exit_code: if timed_out { None } else { launcher_code },
        timed_out,
        success,
        receipt: None,
    })
}

fn verify_cgroup_closed(runtime: &str, unit_name: &str, cgroup: &Path) -> Result<(), SandboxError> {
    let info = systemctl(
        runtime,
        &["show", unit_name, "-p", "ActiveState", "-p", "LoadState"],
    )
    .ok_or_else(|| SandboxError::Uncertain("service state unavailable".into()))?;
    let inactive = info.lines().any(|s| {
        s == "ActiveState=inactive" || s == "ActiveState=failed" || s == "LoadState=not-found"
    });
    let closed = match std::fs::read_to_string(cgroup.join("cgroup.events")) {
        Ok(events) => events.lines().any(|line| line == "populated 0"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
    };
    if !inactive || !closed {
        return Err(SandboxError::Uncertain(
            "service cgroup cleanup not established".into(),
        ));
    }
    if info.lines().any(|s| s == "ActiveState=failed") {
        let _ = systemctl(runtime, &["reset-failed", unit_name]);
    }
    Ok(())
}

fn kill_unit(runtime: &str, unit_name: &str) {
    let _ = systemctl(
        runtime,
        &["kill", "--kill-whom=all", "--signal=KILL", unit_name],
    );
    let _ = systemctl(runtime, &["stop", unit_name]);
}

fn manager_command(program: &Path, runtime: &str) -> Command {
    let mut c = Command::new(program);
    let mut path_val = "/run/current-system/sw/bin:/usr/bin:/bin".to_owned();
    if let Some(parent) = program.parent().and_then(Path::to_str)
        && !parent.is_empty()
    {
        path_val = format!("{parent}:{path_val}");
    }
    c.env_clear()
        .env("XDG_RUNTIME_DIR", runtime)
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path={runtime}/bus"),
        )
        .env("PATH", path_val)
        .env("LANG", "C.UTF-8");
    c
}

fn systemctl(runtime: &str, args: &[&str]) -> Option<String> {
    let systemctl_bin = resolve_manager_tool("METRON_SYSTEMCTL_BIN", "systemctl")?;
    let out = match resolve_manager_tool("METRON_TIMEOUT_BIN", "timeout") {
        Some(timeout_bin) => manager_command(&timeout_bin, runtime)
            .args(["5s"])
            .arg(&systemctl_bin)
            .arg("--user")
            .args(args)
            .output()
            .ok()?,
        None => manager_command(&systemctl_bin, runtime)
            .arg("--user")
            .args(args)
            .output()
            .ok()?,
    };
    out.status
        .success()
        .then(|| String::from_utf8(out.stdout).ok())
        .flatten()
}
