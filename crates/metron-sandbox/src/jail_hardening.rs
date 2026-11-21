//! Safe pre-exec hardening primitives for `metron-jail`:
//! rootless namespace isolation (`unshare`), `/proc/self/fd` FD hygiene verification,
//! `PR_SET_PDEATHSIG(SIGKILL)`, `setrlimit`, and `SandboxReceipt`.

use crate::{error::SandboxError, landlock_fs::LandlockEnforcement, systemd_cg::CgroupLimits};
use rustix::{
    fd::AsRawFd,
    fs::{Mode, OFlags, RawDir, open},
    process::{
        Resource, Rlimit, Signal, getegid, geteuid, getrlimit, set_parent_process_death_signal,
        setrlimit,
    },
    thread::UnshareFlags,
};
use serde::{Deserialize, Serialize};
use std::{fs, io::ErrorKind, mem::MaybeUninit};

/// Policy governing rootless user/net/ipc/uts namespace isolation inside `metron-jail`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum NamespacePolicy {
    /// Fail closed if unprivileged `unshare(NEWUSER|NEWNET|NEWIPC|NEWUTS)` fails.
    #[default]
    Require,
    /// Attempt namespace isolation and explicitly record degraded status ONLY when
    /// the host container forbids unprivileged user namespaces.
    AllowDegraded,
}

/// Process resource limits enforced inside `metron-jail` via safe `rustix::process::setrlimit`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DirectRlimits {
    /// Virtual address space ceiling (`RLIMIT_AS`, bytes).
    pub address_space_bytes: Option<u64>,
    /// CPU time ceiling (`RLIMIT_CPU`, seconds).
    pub cpu_seconds: Option<u64>,
    /// Maximum file size writable (`RLIMIT_FSIZE`, bytes).
    pub fsize_bytes: Option<u64>,
    /// Maximum processes/threads (`RLIMIT_NPROC`).
    pub nproc: Option<u64>,
    /// Maximum open file descriptors (`RLIMIT_NOFILE`).
    pub nofile: Option<u64>,
    /// Set `PR_SET_PDEATHSIG(SIGKILL)` so the child dies if its supervisor exits.
    pub pdeathsig_kill: bool,
}

impl DirectRlimits {
    /// Derives full direct-mode `rlimit` bounds from `CgroupLimits` (used when `systemd-run` is absent).
    #[must_use]
    pub fn from_cgroup_limits(limits: &CgroupLimits) -> Self {
        let cpu_secs = u64::from(limits.timeout_ms).div_ceil(1000).max(1);
        Self {
            address_space_bytes: Some(limits.memory_max_bytes),
            cpu_seconds: Some(cpu_secs),
            fsize_bytes: Some(limits.limit_fsize_bytes),
            nproc: Some(u64::from(limits.tasks_max.max(1))),
            nofile: Some(256),
            pdeathsig_kill: true,
        }
    }

    /// Supplemental limits applied alongside `systemd-run` cgroup v2 enforcement.
    #[must_use]
    pub fn systemd_supplement(limits: &CgroupLimits) -> Self {
        Self {
            address_space_bytes: None,
            cpu_seconds: None,
            fsize_bytes: Some(limits.limit_fsize_bytes),
            nproc: None,
            nofile: Some(512),
            pdeathsig_kill: true,
        }
    }
}

/// Machine-readable attestation emitted by `metron-jail` immediately prior to `exec`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SandboxReceipt {
    /// Exact Landlock ABI and FS/net/scope enforcement status.
    pub landlock: LandlockEnforcement,
    /// True once `seccomp-bpf` has been installed across all threads.
    pub seccomp_active: bool,
    /// True when `CLONE_NEWUSER | CLONE_NEWNET | CLONE_NEWIPC | CLONE_NEWUTS` succeeded.
    pub namespaces_active: bool,
    /// Explicit explanation if and only if `NamespacePolicy::AllowDegraded` fell back.
    pub namespaces_degraded_reason: Option<String>,
    /// True when `setrlimit` / `PR_SET_PDEATHSIG` bounds were applied.
    pub rlimits_applied: bool,
    /// True when pre-exec `/proc/self/fd` inspection passed with zero unexpected FDs.
    pub fd_hygiene_verified: bool,
}

/// Non-mutating probe indicating whether unprivileged user namespaces are enabled on this host.
#[must_use]
pub fn userns_supported() -> bool {
    let max_ok = fs::read_to_string("/proc/sys/user/max_user_namespaces")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .is_none_or(|n| n > 0);
    let clone_ok = fs::read_to_string("/proc/sys/kernel/unprivileged_userns_clone")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .is_none_or(|n| n != 0);
    let apparmor_restricted =
        fs::read_to_string("/proc/sys/kernel/apparmor_restrict_unprivileged_userns")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .is_some_and(|n| n != 0);
    max_ok && clone_ok && !apparmor_restricted
}

/// Enters rootless `NEWUSER | NEWNET | NEWIPC | NEWUTS` namespaces and maps the calling UID/GID.
///
/// Fails closed under `NamespacePolicy::Require`; degrades ONLY under explicit
/// `NamespacePolicy::AllowDegraded` and returns the concrete reason for the receipt.
pub fn apply_rootless_namespaces(
    policy: NamespacePolicy,
) -> Result<(bool, Option<String>), SandboxError> {
    if policy == NamespacePolicy::AllowDegraded && !userns_supported() {
        return Ok((
            false,
            Some("unprivileged userns restricted by host kernel/AppArmor policy".into()),
        ));
    }

    let uid = geteuid().as_raw();
    let gid = getegid().as_raw();
    let flags =
        UnshareFlags::NEWUSER | UnshareFlags::NEWNET | UnshareFlags::NEWIPC | UnshareFlags::NEWUTS;

    #[allow(deprecated)]
    let unshare_res = rustix::thread::unshare(flags);
    match unshare_res {
        Ok(()) => {
            if let Err(err) = fs::write("/proc/self/setgroups", b"deny")
                && err.kind() != ErrorKind::NotFound
            {
                return match policy {
                    NamespacePolicy::Require => Err(SandboxError::Denied(format!(
                        "userns setgroups deny write failed: {err}"
                    ))),
                    NamespacePolicy::AllowDegraded => {
                        Ok((false, Some(format!("userns setgroups restricted: {err}"))))
                    }
                };
            }
            if let Err(err) = fs::write("/proc/self/uid_map", format!("{uid} {uid} 1\n")) {
                return match policy {
                    NamespacePolicy::Require => Err(SandboxError::Denied(format!(
                        "userns uid_map write failed: {err}"
                    ))),
                    NamespacePolicy::AllowDegraded => {
                        Ok((false, Some(format!("userns uid_map restricted: {err}"))))
                    }
                };
            }
            if let Err(err) = fs::write("/proc/self/gid_map", format!("{gid} {gid} 1\n")) {
                return match policy {
                    NamespacePolicy::Require => Err(SandboxError::Denied(format!(
                        "userns gid_map write failed: {err}"
                    ))),
                    NamespacePolicy::AllowDegraded => {
                        Ok((false, Some(format!("userns gid_map restricted: {err}"))))
                    }
                };
            }
            Ok((true, None))
        }
        Err(err) => match policy {
            NamespacePolicy::Require => Err(SandboxError::Denied(format!(
                "rootless namespace isolation failed: {err}"
            ))),
            NamespacePolicy::AllowDegraded => Ok((false, Some(format!("unshare failed: {err}")))),
        },
    }
}

/// Inspects `/proc/self/fd` in safe Rust and fails closed if any open FD exists outside
/// `{0, 1, 2, dir_fd}` and `allowed_fds`.
pub fn verify_fd_hygiene(allowed_fds: &[i32]) -> Result<(), SandboxError> {
    let dir_fd = open(
        "/proc/self/fd",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| SandboxError::Denied(format!("fd hygiene: open /proc/self/fd failed: {e}")))?;
    let dir_raw = dir_fd.as_raw_fd();

    let mut buf = [MaybeUninit::uninit(); 4096];
    let mut entries = RawDir::new(&dir_fd, &mut buf);
    while let Some(entry_res) = entries.next() {
        let entry = entry_res
            .map_err(|e| SandboxError::Denied(format!("fd hygiene: read /proc/self/fd: {e}")))?;
        let name = entry.file_name().to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        let Ok(name_str) = std::str::from_utf8(name) else {
            return Err(SandboxError::Denied(
                "fd hygiene: non-utf8 entry in /proc/self/fd".into(),
            ));
        };
        let Ok(fd_num) = name_str.parse::<i32>() else {
            return Err(SandboxError::Denied(format!(
                "fd hygiene: non-numeric entry in /proc/self/fd: {name_str}"
            )));
        };
        if (0..=2).contains(&fd_num) || fd_num == dir_raw || allowed_fds.contains(&fd_num) {
            continue;
        }
        return Err(SandboxError::Denied(format!(
            "unexpected open file descriptor inherited by jail: fd {fd_num}"
        )));
    }
    Ok(())
}

/// Requests `SIGKILL` if the parent supervisor dies unexpectedly.
pub fn apply_pdeathsig_kill() -> Result<(), SandboxError> {
    set_parent_process_death_signal(Some(Signal::KILL))
        .map_err(|e| SandboxError::Denied(format!("PR_SET_PDEATHSIG(SIGKILL) failed: {e}")))
}

/// Enforces all configured `RLIMIT_*` bounds via safe `rustix::process::setrlimit`.
///
/// Strictly clamps against existing hard limits so unprivileged tightening never fails
/// with `EPERM` inside already-constrained containers.
pub fn apply_rlimits(limits: &DirectRlimits) -> Result<(), SandboxError> {
    if limits.pdeathsig_kill {
        apply_pdeathsig_kill()?;
    }
    if let Some(v) = limits.nofile {
        tighten_rlimit(Resource::Nofile, v, "RLIMIT_NOFILE")?;
    }
    if let Some(v) = limits.fsize_bytes {
        tighten_rlimit(Resource::Fsize, v, "RLIMIT_FSIZE")?;
    }
    if let Some(v) = limits.cpu_seconds {
        tighten_rlimit(Resource::Cpu, v, "RLIMIT_CPU")?;
    }
    if let Some(v) = limits.nproc {
        tighten_rlimit(Resource::Nproc, v, "RLIMIT_NPROC")?;
    }
    // Apply RLIMIT_AS last so earlier setup allocations cannot trigger ENOMEM.
    if let Some(v) = limits.address_space_bytes {
        tighten_rlimit(Resource::As, v, "RLIMIT_AS")?;
    }
    Ok(())
}

fn tighten_rlimit(resource: Resource, requested: u64, label: &str) -> Result<(), SandboxError> {
    let cur = getrlimit(resource);
    let capped = match cur.maximum {
        Some(max) => requested.min(max),
        None => requested,
    };
    let next = Rlimit {
        current: Some(capped),
        maximum: Some(capped),
    };
    setrlimit(resource, next)
        .map_err(|e| SandboxError::Denied(format!("setrlimit({label}={capped}) failed: {e}")))
}
