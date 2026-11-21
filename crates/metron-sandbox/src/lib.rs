//! Metron two-lane zero-bubblewrap sandbox (`Lane B: native_jail` and `Lane A: wasm_aot`).
//!
//! All kernel confinement uses unprivileged `systemd-run --user --collect` cgroup v2 (or
//! hardened direct `setrlimit` + `PR_SET_PDEATHSIG` supervision), rootless namespaces
//! (`CLONE_NEWUSER | CLONE_NEWNET | CLONE_NEWIPC | CLONE_NEWUTS`), `PR_SET_NO_NEW_PRIVS`,
//! strict `landlock 0.4.8` (`CompatLevel::HardRequirement`), `seccompiler 0.5.0`,
//! `/proc/self/fd` hygiene enforcement, and direct sealed `memfd` execution.

pub mod direct_supervisor;
pub mod error;
pub mod jail_hardening;
pub mod landlock_fs;
pub mod memfd;
pub mod seccomp_bpf;
pub mod systemd_cg;
pub mod trampoline;

pub use direct_supervisor::{check_scratch_space, read_receipt, run_direct_supervised};
pub use error::SandboxError;
pub use jail_hardening::{
    DirectRlimits, NamespacePolicy, SandboxReceipt, apply_pdeathsig_kill, apply_rlimits,
    apply_rootless_namespaces, userns_supported, verify_fd_hygiene,
};
pub use landlock_fs::{
    LandlockAbi, LandlockEnforcement, LandlockPolicy, apply_landlock, probe_kernel_abi,
};
pub use memfd::{
    MAX_EXECUTABLE_BYTES, SealedMemfd, open_and_seal_regular, require_disk_backed, seal_bytes,
    sha256_regular_file,
};
pub use seccomp_bpf::{SeccompPolicy, build_filter, install_seccomp};
pub use systemd_cg::{CgroupCompleted, CgroupLimits, resolve_manager_tool, systemd_user_available};
pub use trampoline::{ExecSource, InheritedMemfd, TrampolineSpec};

use std::{
    fs::OpenOptions,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static SPEC_SEQ: AtomicU64 = AtomicU64::new(0);

/// High-level Lane B native jail configuration.
#[derive(Clone, Debug)]
pub struct NativeJailConfig {
    /// Cgroup v2 unit name and resource caps.
    pub cgroup: CgroupLimits,
    /// Primary executable source.
    pub primary: ExecSource,
    /// Optional secondary inherited `memfd`s (e.g., `RUSTC`).
    pub inherited: Vec<InheritedMemfd>,
    /// Arguments passed to the executable.
    pub args: Vec<String>,
    /// Workspace root (mounted read-only unless `writable_workspace` is `true`).
    pub workspace: Option<PathBuf>,
    /// Whether `workspace` is granted read-write access.
    pub writable_workspace: bool,
    /// Private disk-backed scratch directory (always read-write).
    pub scratch: PathBuf,
    /// Additional explicit read-only inputs (e.g., pinned toolchain root).
    pub ro_inputs: Vec<PathBuf>,
    /// Working directory inside the jail.
    pub workdir: PathBuf,
    /// Explicit sanitized environment variables.
    pub env: Vec<(String, String)>,
    /// Optional explicit trampoline override (used by tests or custom installs).
    pub trampoline_bin: Option<PathBuf>,
    /// Rootless namespace isolation policy (`Require` by default; fail closed).
    pub namespace_policy: NamespacePolicy,
    /// Whether to allow `socket(AF_UNIX, ...)` inside the jail (`false` by default).
    pub allow_unix_sockets: bool,
}

/// Output from a Lane B native jail execution.
pub type NativeJailOutput = CgroupCompleted;

/// Runs a Lane B jailed process under `systemd-run --user --collect`, rootless namespaces,
/// `PR_SET_NO_NEW_PRIVS`, strict `landlock 0.4.8`, `seccompiler 0.5.0`, and sealed `memfd`.
pub fn run_native_jail(
    cfg: &NativeJailConfig,
    stdin_memfd: Option<&SealedMemfd>,
) -> Result<NativeJailOutput, SandboxError> {
    require_disk_backed(&cfg.scratch)?;
    check_scratch_space(&cfg.scratch, cfg.cgroup.scratch_reserve_bytes)?;
    let (receipt_path, _receipt_guard) = alloc_receipt_path(&cfg.scratch);
    let mut spec = build_lane_b_spec(cfg);
    spec.rlimits = Some(DirectRlimits::systemd_supplement(&cfg.cgroup));
    spec.receipt_path = Some(receipt_path.clone());

    let (spec_path, _guard) = write_spec_file(&cfg.scratch, &spec)?;
    let (bin, mut prefix_args) = resolve_trampoline(cfg.trampoline_bin.as_deref())?;
    prefix_args.push("--policy-file".to_owned());
    prefix_args.push(spec_path.to_string_lossy().into_owned());

    let stdin_file = stdin_memfd.map(SealedMemfd::as_file);
    let mut completed = systemd_cg::run_in_systemd_cgroup(
        &cfg.cgroup,
        &cfg.scratch,
        &bin,
        &prefix_args,
        stdin_file,
    )?;
    completed.receipt = read_receipt(&receipt_path);
    Ok(completed)
}

/// Runs the Lane B trampoline under the hardened direct supervisor (when `systemd-run` is
/// absent): enforces `RLIMIT_AS`, `RLIMIT_CPU`, `RLIMIT_FSIZE`, `RLIMIT_NPROC`, `RLIMIT_NOFILE`,
/// `PR_SET_PDEATHSIG(SIGKILL)`, wall-clock deadline + `SIGKILL`, rootless namespaces, strict
/// Landlock, seccomp-bpf, and FD hygiene.
pub fn run_direct_jail(
    cfg: &NativeJailConfig,
    stdin_memfd: Option<&SealedMemfd>,
) -> Result<NativeJailOutput, SandboxError> {
    require_disk_backed(&cfg.scratch)?;
    check_scratch_space(&cfg.scratch, cfg.cgroup.scratch_reserve_bytes)?;
    let (receipt_path, _receipt_guard) = alloc_receipt_path(&cfg.scratch);
    let mut spec = build_lane_b_spec(cfg);
    spec.rlimits = Some(DirectRlimits::from_cgroup_limits(&cfg.cgroup));
    spec.receipt_path = Some(receipt_path.clone());

    let (spec_path, _guard) = write_spec_file(&cfg.scratch, &spec)?;
    let (bin, mut prefix_args) = resolve_trampoline(cfg.trampoline_bin.as_deref())?;
    prefix_args.push("--policy-file".to_owned());
    prefix_args.push(spec_path.to_string_lossy().into_owned());

    let stdin_file = stdin_memfd.map(SealedMemfd::as_file);
    run_direct_supervised(
        &cfg.cgroup,
        &cfg.scratch,
        &bin,
        &prefix_args,
        stdin_file,
        Some(&receipt_path),
    )
}

/// Builds a `TrampolineSpec` from a `NativeJailConfig`.
#[must_use]
pub fn build_lane_b_spec(cfg: &NativeJailConfig) -> TrampolineSpec {
    let mut ro_inputs = cfg.ro_inputs.clone();
    let mut rw_paths = vec![cfg.scratch.clone()];
    if let Some(ws) = &cfg.workspace {
        if cfg.writable_workspace {
            rw_paths.push(ws.clone());
        } else {
            ro_inputs.push(ws.clone());
        }
    }

    let mut env = vec![
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
        (
            "HOME".to_owned(),
            cfg.scratch.to_string_lossy().into_owned(),
        ),
        (
            "TMPDIR".to_owned(),
            cfg.scratch.to_string_lossy().into_owned(),
        ),
    ];
    for (k, v) in &cfg.env {
        if let Some(existing) = env.iter_mut().find(|(ek, _)| ek == k) {
            existing.1 = v.clone();
        } else {
            env.push((k.clone(), v.clone()));
        }
    }

    TrampolineSpec {
        primary: cfg.primary.clone(),
        inherited: cfg.inherited.clone(),
        args: cfg.args.clone(),
        workdir: cfg.workdir.clone(),
        env,
        landlock: LandlockPolicy::lane_b(ro_inputs, rw_paths),
        seccomp: SeccompPolicy {
            deny_prot_exec: false,
            allow_unix_sockets: cfg.allow_unix_sockets,
        },
        namespace_policy: cfg.namespace_policy,
        rlimits: None,
        receipt_path: None,
    }
}

/// Resolves the `metron-jail` trampoline executable and any internal dispatch prefix args.
pub fn resolve_trampoline(explicit: Option<&Path>) -> Result<(PathBuf, Vec<String>), SandboxError> {
    if let Some(path) = explicit {
        return Ok((path.to_path_buf(), Vec::new()));
    }
    if let Ok(path) = std::env::var("METRON_JAIL_BIN")
        && !path.is_empty()
    {
        return Ok((PathBuf::from(path), Vec::new()));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        for candidate_dir in [Some(dir), dir.parent()] {
            let Some(base) = candidate_dir else {
                continue;
            };
            let jail = base.join("metron-jail");
            if jail.is_file() {
                return Ok((jail, Vec::new()));
            }
            let runtime = base.join("metron-runtime");
            if runtime.is_file() {
                return Ok((runtime, vec!["__metron-jail".to_owned()]));
            }
        }
    }
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join("metron-jail");
            if candidate.is_file() {
                return Ok((candidate, Vec::new()));
            }
        }
    }
    Err(SandboxError::Denied(
        "metron-jail trampoline executable not found".into(),
    ))
}

struct FileCleanupGuard(PathBuf);
impl Drop for FileCleanupGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn alloc_receipt_path(scratch: &Path) -> (PathBuf, FileCleanupGuard) {
    let seq = SPEC_SEQ.fetch_add(1, Ordering::Relaxed);
    let path = scratch.join(format!(
        ".metron-jail-receipt-{}-{seq}.json",
        std::process::id()
    ));
    (path.clone(), FileCleanupGuard(path))
}

fn write_spec_file(
    scratch: &Path,
    spec: &TrampolineSpec,
) -> Result<(PathBuf, FileCleanupGuard), SandboxError> {
    let seq = SPEC_SEQ.fetch_add(1, Ordering::Relaxed);
    let path = scratch.join(format!(
        ".metron-jail-spec-{}-{seq}.json",
        std::process::id()
    ));
    let bytes = serde_json::to_vec(spec)
        .map_err(|e| SandboxError::Denied(format!("jail spec encode failed: {e}")))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|_| SandboxError::Denied("jail spec file creation failed".into()))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| SandboxError::Denied("jail spec file write failed".into()))?;
    Ok((path.clone(), FileCleanupGuard(path)))
}
