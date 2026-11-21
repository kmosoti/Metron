//! Landlock ABI v3..v6 strict filesystem, TCP network, and IPC scope enforcement.
//!
//! Zero silent `CompatLevel::BestEffort` downgrades: probes the exact host kernel ABI
//! under `CompatLevel::HardRequirement`, fails closed with `SandboxError::LandlockInsufficient`
//! if the kernel cannot strictly enforce `min_abi` or any requested filesystem rule, and
//! reports exact FS/net/scope enforcement status in `LandlockEnforcement`.

use crate::error::SandboxError;
use landlock::{
    ABI, Access, AccessFs, AccessNet, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset,
    RulesetAttr, RulesetCreatedAttr, RulesetStatus, Scope,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Explicit Landlock kernel ABI floor.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum LandlockAbi {
    /// ABI v1 (Linux 5.13): basic filesystem read/write/execute.
    V1,
    /// ABI v2 (Linux 5.19): adds `Refer` across directories.
    V2,
    /// ABI v3 (Linux 6.2): adds `Truncate`.
    V3,
    /// ABI v4 (Linux 6.7): adds TCP `BindTcp` / `ConnectTcp` network restriction.
    V4,
    /// ABI v5 (Linux 6.10): adds `IoctlDev`.
    V5,
    /// ABI v6 (Linux 6.12): adds abstract Unix socket and signal `Scope` isolation.
    V6,
}

impl LandlockAbi {
    /// Converts to the `landlock` crate's `ABI` enum.
    #[must_use]
    pub const fn to_crate_abi(self) -> ABI {
        match self {
            Self::V1 => ABI::V1,
            Self::V2 => ABI::V2,
            Self::V3 => ABI::V3,
            Self::V4 => ABI::V4,
            Self::V5 => ABI::V5,
            Self::V6 => ABI::V6,
        }
    }
}

const fn default_min_abi() -> LandlockAbi {
    LandlockAbi::V3
}

/// Explicit filesystem and network allowlist enforced via Landlock under `HardRequirement`.
///
/// Everything not listed (`$HOME`, vaults, `/etc/shadow`, ambient `/tmp`, TCP ports)
/// is denied by default with `EACCES`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LandlockPolicy {
    /// Explicit read-only paths (files or directories).
    pub ro_paths: Vec<PathBuf>,
    /// Explicit read-write paths (scratch and optional writable workspace).
    pub rw_paths: Vec<PathBuf>,
    /// Explicit write-only paths (for example, AOT compiler output directory without read access).
    #[serde(default)]
    pub wo_paths: Vec<PathBuf>,
    /// Explicit character/pseudo-devices permitted read+write (e.g., `/dev/null`).
    pub rw_devices: Vec<PathBuf>,
    /// Whether to grant read-only access to `/proc/self` (without broad `/proc` execute).
    pub allow_proc_self: bool,
    /// Whether to enforce Landlock ABI v4+ TCP bind/connect denial (zero allowed ports).
    pub deny_tcp: bool,
    /// Whether to enforce Landlock ABI v6+ abstract Unix socket & signal scoping.
    pub isolate_scope: bool,
    /// Minimum Landlock ABI strictly required by policy (fails closed if kernel is older).
    #[serde(default = "default_min_abi")]
    pub min_abi: LandlockAbi,
    /// When `true`, fail closed if kernel lacks ABI v4+ TCP restriction.
    #[serde(default)]
    pub require_net_isolation: bool,
    /// When `true`, fail closed if kernel lacks ABI v6+ IPC scope isolation.
    #[serde(default)]
    pub require_scope_isolation: bool,
}

impl LandlockPolicy {
    /// Builds a Lane B policy allowing only explicit read-only inputs, system runtime libs,
    /// and explicit read-write scratch/workspace paths.
    #[must_use]
    pub fn lane_b(ro_inputs: Vec<PathBuf>, rw_paths: Vec<PathBuf>) -> Self {
        let mut ro_paths = baseline_system_ro_paths();
        for path in ro_inputs {
            if !ro_paths.contains(&path) {
                ro_paths.push(path);
            }
        }
        Self {
            ro_paths,
            rw_paths,
            wo_paths: Vec::new(),
            rw_devices: baseline_rw_devices(),
            allow_proc_self: true,
            deny_tcp: true,
            isolate_scope: true,
            min_abi: LandlockAbi::V3,
            require_net_isolation: false,
            require_scope_isolation: false,
        }
    }
}

/// Exact Landlock enforcement tier recorded in `SandboxReceipt`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LandlockEnforcement {
    /// Highest kernel Landlock ABI actively enforced for this ruleset.
    pub abi: LandlockAbi,
    /// All requested filesystem rules were strictly enforced under `HardRequirement`.
    pub fs_enforced: bool,
    /// ABI v4+ TCP bind/connect restriction actively enforced.
    pub net_enforced: bool,
    /// ABI v6+ abstract Unix socket and signal scope isolation actively enforced.
    pub scope_enforced: bool,
}

/// Probes the exact highest Landlock ABI supported by the running Linux kernel using
/// `CompatLevel::HardRequirement` (never silently downgrading).
pub fn probe_kernel_abi() -> Result<LandlockAbi, SandboxError> {
    for candidate in [
        LandlockAbi::V6,
        LandlockAbi::V5,
        LandlockAbi::V4,
        LandlockAbi::V3,
        LandlockAbi::V2,
        LandlockAbi::V1,
    ] {
        let abi = candidate.to_crate_abi();
        let Ok(mut rs) = Ruleset::default()
            .set_compatibility(CompatLevel::HardRequirement)
            .handle_access(AccessFs::from_all(abi))
        else {
            continue;
        };
        if candidate >= LandlockAbi::V4 {
            let Ok(next) = rs.handle_access(AccessNet::from_all(abi)) else {
                continue;
            };
            rs = next;
        }
        if candidate >= LandlockAbi::V6 {
            let Ok(next) = rs.scope(Scope::from_all(abi)) else {
                continue;
            };
            rs = next;
        }
        if rs.create().is_ok() {
            return Ok(candidate);
        }
    }
    Err(SandboxError::LandlockInsufficient(
        "host kernel does not support Landlock".into(),
    ))
}

/// Applies `LandlockPolicy` permanently to the calling thread/process under `HardRequirement`.
pub fn apply_landlock(policy: &LandlockPolicy) -> Result<LandlockEnforcement, SandboxError> {
    let detected = probe_kernel_abi()?;
    if detected < policy.min_abi {
        return Err(SandboxError::LandlockInsufficient(format!(
            "kernel Landlock {detected:?} is below policy minimum {:?}",
            policy.min_abi
        )));
    }
    if policy.require_net_isolation && detected < LandlockAbi::V4 {
        return Err(SandboxError::LandlockInsufficient(format!(
            "kernel Landlock {detected:?} lacks required ABI v4 TCP isolation"
        )));
    }
    if policy.require_scope_isolation && detected < LandlockAbi::V6 {
        return Err(SandboxError::LandlockInsufficient(format!(
            "kernel Landlock {detected:?} lacks required ABI v6 scope isolation"
        )));
    }

    let abi = detected.to_crate_abi();
    let mut ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(AccessFs::from_all(abi))
        .map_err(|e| SandboxError::LandlockInsufficient(format!("landlock fs handle: {e}")))?;

    let net_enforced = policy.deny_tcp && detected >= LandlockAbi::V4;
    if net_enforced {
        ruleset = ruleset
            .handle_access(AccessNet::from_all(abi))
            .map_err(|e| SandboxError::LandlockInsufficient(format!("landlock net handle: {e}")))?;
    }
    let scope_enforced = policy.isolate_scope && detected >= LandlockAbi::V6;
    if scope_enforced {
        ruleset = ruleset.scope(Scope::from_all(abi)).map_err(|e| {
            SandboxError::LandlockInsufficient(format!("landlock scope handle: {e}"))
        })?;
    }

    let mut created = ruleset
        .create()
        .map_err(|e| SandboxError::LandlockInsufficient(format!("landlock create: {e}")))?;

    for ro in &policy.ro_paths {
        if !ro.exists() {
            continue;
        }
        let access = if ro.is_dir() {
            AccessFs::from_read(abi)
        } else {
            AccessFs::from_read(abi) & AccessFs::from_file(abi)
        };
        let fd = PathFd::new(ro).map_err(|e| {
            SandboxError::LandlockInsufficient(format!("landlock ro open {}: {e}", ro.display()))
        })?;
        created = created
            .add_rule(PathBeneath::new(fd, access))
            .map_err(|e| {
                SandboxError::LandlockInsufficient(format!(
                    "landlock ro rule {}: {e}",
                    ro.display()
                ))
            })?;
    }

    for rw in &policy.rw_paths {
        let access = if rw.is_dir() {
            AccessFs::from_all(abi)
        } else {
            AccessFs::from_file(abi)
        };
        let fd = PathFd::new(rw).map_err(|e| {
            SandboxError::LandlockInsufficient(format!("landlock rw open {}: {e}", rw.display()))
        })?;
        created = created
            .add_rule(PathBeneath::new(fd, access))
            .map_err(|e| {
                SandboxError::LandlockInsufficient(format!(
                    "landlock rw rule {}: {e}",
                    rw.display()
                ))
            })?;
    }

    for wo in &policy.wo_paths {
        let access = if wo.is_dir() {
            AccessFs::from_write(abi)
        } else {
            AccessFs::from_write(abi) & AccessFs::from_file(abi)
        };
        let fd = PathFd::new(wo).map_err(|e| {
            SandboxError::LandlockInsufficient(format!("landlock wo open {}: {e}", wo.display()))
        })?;
        created = created
            .add_rule(PathBeneath::new(fd, access))
            .map_err(|e| {
                SandboxError::LandlockInsufficient(format!(
                    "landlock wo rule {}: {e}",
                    wo.display()
                ))
            })?;
    }

    for dev in &policy.rw_devices {
        if !dev.exists() {
            continue;
        }
        let fd = PathFd::new(dev).map_err(|e| {
            SandboxError::LandlockInsufficient(format!("landlock dev {}: {e}", dev.display()))
        })?;
        let access = (AccessFs::ReadFile | AccessFs::WriteFile | AccessFs::Truncate)
            & AccessFs::from_file(abi);
        created = created
            .add_rule(PathBeneath::new(fd, access))
            .map_err(|e| {
                SandboxError::LandlockInsufficient(format!(
                    "landlock dev rule {}: {e}",
                    dev.display()
                ))
            })?;
    }

    if policy.allow_proc_self {
        // Strictly scope read-only access to `/proc/self` without granting broad `/proc` execute.
        if let Ok(self_fd) = PathFd::new("/proc/self") {
            let ro_no_exec = AccessFs::ReadFile | AccessFs::ReadDir;
            created = created
                .add_rule(PathBeneath::new(self_fd, ro_no_exec))
                .map_err(|e| {
                    SandboxError::LandlockInsufficient(format!("landlock /proc/self rule: {e}"))
                })?;
        }
    }

    let status = created
        .restrict_self()
        .map_err(|e| SandboxError::LandlockInsufficient(format!("landlock restrict_self: {e}")))?;

    match status.ruleset {
        RulesetStatus::FullyEnforced => Ok(LandlockEnforcement {
            abi: detected,
            fs_enforced: true,
            net_enforced,
            scope_enforced,
        }),
        RulesetStatus::PartiallyEnforced | RulesetStatus::NotEnforced => {
            Err(SandboxError::LandlockInsufficient(format!(
                "landlock ruleset not fully enforced at {detected:?}: {:?}",
                status.ruleset
            )))
        }
    }
}

fn baseline_system_ro_paths() -> Vec<PathBuf> {
    [
        "/usr",
        "/lib",
        "/lib64",
        "/bin",
        "/etc/ld.so.cache",
        "/etc/ld.so.conf",
        "/etc/ld.so.conf.d",
        "/etc/alternatives",
        "/etc/localtime",
        "/etc/ssl/certs",
        "/dev/urandom",
        "/dev/random",
        "/nix/store",
    ]
    .into_iter()
    .map(PathBuf::from)
    .filter(|p| Path::new(p).exists())
    .collect()
}

fn baseline_rw_devices() -> Vec<PathBuf> {
    ["/dev/null", "/dev/zero"]
        .into_iter()
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .collect()
}
