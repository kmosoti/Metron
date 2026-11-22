//! Unprivileged jail trampoline: verifies FD hygiene, enters rootless namespaces,
//! seals `memfd`s, sets `PR_SET_NO_NEW_PRIVS`, enforces strict Landlock + seccomp-bpf,
//! records `SandboxReceipt`, applies `setrlimit`, and `exec`s `/proc/self/fd/<n>` directly.

use crate::{
    error::SandboxError,
    jail_hardening::{
        DirectRlimits, NamespacePolicy, SandboxReceipt, apply_pdeathsig_kill, apply_rlimits,
        apply_rootless_namespaces, verify_fd_hygiene,
    },
    landlock_fs::{LandlockPolicy, apply_landlock},
    memfd::{
        SealedMemfd, open_and_seal_regular, read_bounded_regular_file, replace_stdin_with,
        seal_from_reader,
    },
    seccomp_bpf::{SeccompPolicy, install_seccomp},
};
use rustix::{
    fd::AsRawFd,
    fs::{Mode, OFlags, open},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::Write,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

/// Source of the primary executable executed inside the jail.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ExecSource {
    /// Open regular file on disk, verify optional SHA-256, and seal into `memfd`.
    Path {
        /// Host path to the executable binary.
        path: PathBuf,
        /// Optional expected SHA-256 hex digest.
        sha256: Option<String>,
        /// Diagnostic label for the `memfd`.
        label: String,
    },
    /// Read executable bytes from stdin pipe into a sealed `memfd`, verify SHA-256,
    /// then bind `/dev/null` to stdin before `exec`.
    StdinSealed {
        /// Expected SHA-256 hex digest of the streamed bytes.
        sha256: String,
        /// Diagnostic label for the `memfd`.
        label: String,
    },
}

/// Secondary executable sealed into an inherited `memfd` and exposed via an env var
/// (for example `RUSTC=/proc/self/fd/<n>` when running `cargo`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InheritedMemfd {
    /// Environment variable pointing to `/proc/self/fd/<n>`.
    pub env_var: String,
    /// Host path to the executable binary.
    pub path: PathBuf,
    /// Expected SHA-256 hex digest.
    pub sha256: String,
    /// Diagnostic label for the `memfd`.
    pub label: String,
}

/// Complete specification consumed by `metron-jail` before locking down the process.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TrampolineSpec {
    /// Primary executable source.
    pub primary: ExecSource,
    /// Secondary inherited `memfd` descriptors.
    pub inherited: Vec<InheritedMemfd>,
    /// Arguments passed to the primary executable (excluding `argv[0]`).
    pub args: Vec<String>,
    /// Working directory inside the jail.
    pub workdir: PathBuf,
    /// Explicit sanitized environment variables (`env_clear()` is always applied first).
    pub env: Vec<(String, String)>,
    /// Strict Landlock filesystem/network/scope ruleset.
    pub landlock: LandlockPolicy,
    /// Seccomp-BPF syscall/socket filter.
    pub seccomp: SeccompPolicy,
    /// Rootless user/net/ipc/uts namespace isolation policy.
    #[serde(default)]
    pub namespace_policy: NamespacePolicy,
    /// Optional `setrlimit` / `PR_SET_PDEATHSIG` bounds applied immediately before `exec`.
    #[serde(default)]
    pub rlimits: Option<DirectRlimits>,
    /// Optional path (within scratch) where `metron-jail` writes its `SandboxReceipt`.
    #[serde(default)]
    pub receipt_path: Option<PathBuf>,
}

/// Entrypoint invoked by `metron-jail`, `metron-runtime __metron-jail`, or Lane A `--lane-a-policy`.
pub fn run_from_args(args: &[String]) -> ExitCode {
    if args.len() == 2 && args[0] == "--lane-a-policy" {
        return crate::wasm_aot_jail::run_trampoline_from_spec(Path::new(&args[1]));
    }
    match run_inner(args) {
        Ok(()) => ExitCode::from(127),
        Err(err) => {
            eprintln!("metron-jail: {err}");
            ExitCode::from(126)
        }
    }
}

fn run_inner(args: &[String]) -> Result<(), SandboxError> {
    if args.len() != 2 || args[0] != "--policy-file" {
        return Err(SandboxError::Denied(
            "usage: metron-jail --policy-file SPEC.json".into(),
        ));
    }
    let spec_path = Path::new(&args[1]);
    let raw = read_bounded_regular_file(spec_path, 4 * 1024 * 1024, "jail spec")?;
    let _ = std::fs::remove_file(spec_path);

    let spec: TrampolineSpec = serde_json::from_slice(&raw)
        .map_err(|e| SandboxError::Denied(format!("invalid jail spec: {e}")))?;

    execute_spec(&spec)
}

/// Seals executables in `memfd`, isolates namespaces, applies strict Landlock + seccomp,
/// emits `SandboxReceipt`, verifies FD hygiene, applies `rlimits`, and replaces the current
/// process image via `CommandExt::exec` (never returning on success).
pub fn execute_spec(spec: &TrampolineSpec) -> Result<(), SandboxError> {
    // Isolate the jailed process tree into its own process group (PGID == PID) so direct
    // supervisors can atomically SIGKILL the child and any shell-spawned grandchildren.
    let _ = rustix::process::setpgid(None, None);
    if spec.rlimits.is_some_and(|r| r.pdeathsig_kill) {
        apply_pdeathsig_kill()?;
    }

    let primary: SealedMemfd = match &spec.primary {
        ExecSource::Path {
            path,
            sha256,
            label,
        } => open_and_seal_regular(path, label, sha256.as_deref(), false)?,
        ExecSource::StdinSealed { sha256, label } => {
            let stdin = std::io::stdin();
            let locked = stdin.lock();
            let memfd = seal_from_reader(label, locked, Some(sha256.as_str()), false)?;
            let dev_null = open("/dev/null", OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty())
                .map_err(|_| SandboxError::Denied("failed to open /dev/null".into()))?;
            replace_stdin_with(rustix::fd::AsFd::as_fd(&dev_null))?;
            memfd
        }
    };
    primary.clear_cloexec()?;

    let mut allowed_memfds = Vec::with_capacity(1 + spec.inherited.len());
    allowed_memfds.push(primary.as_file().as_raw_fd());

    let mut extra_fds = Vec::with_capacity(spec.inherited.len());
    let mut env_pairs = spec.env.clone();
    for item in &spec.inherited {
        let memfd =
            open_and_seal_regular(&item.path, &item.label, Some(item.sha256.as_str()), false)?;
        memfd.clear_cloexec()?;
        allowed_memfds.push(memfd.as_file().as_raw_fd());
        env_pairs.push((
            item.env_var.clone(),
            memfd.self_fd_path().to_string_lossy().into_owned(),
        ));
        extra_fds.push(memfd);
    }

    let (namespaces_active, namespaces_degraded_reason) =
        apply_rootless_namespaces(spec.namespace_policy)?;

    rustix::thread::set_no_new_privs(true)
        .map_err(|e| SandboxError::Denied(format!("PR_SET_NO_NEW_PRIVS failed: {e}")))?;
    let landlock = apply_landlock(&spec.landlock)?;
    install_seccomp(spec.seccomp)?;

    // Verify FD hygiene before writing receipt, and again immediately before exec
    // after the receipt handle has been closed.
    verify_fd_hygiene(&allowed_memfds)?;

    if let Some(receipt_path) = &spec.receipt_path {
        let receipt = SandboxReceipt {
            landlock,
            seccomp_active: true,
            namespaces_active,
            namespaces_degraded_reason,
            rlimits_applied: spec.rlimits.is_some(),
            fd_hygiene_verified: true,
        };
        write_receipt(receipt_path, &receipt)?;
    }

    verify_fd_hygiene(&allowed_memfds)?;

    if let Some(rlimits) = &spec.rlimits {
        apply_rlimits(rlimits)?;
    }

    let mut cmd = Command::new(primary.self_fd_path());
    cmd.env_clear();
    for (k, v) in env_pairs {
        cmd.env(k, v);
    }
    cmd.current_dir(&spec.workdir);
    cmd.args(&spec.args);

    let err = cmd.exec();
    drop(extra_fds);
    drop(primary);
    Err(SandboxError::Denied(format!("memfd exec failed: {err}")))
}

fn write_receipt(path: &Path, receipt: &SandboxReceipt) -> Result<(), SandboxError> {
    let bytes = serde_json::to_vec(receipt)
        .map_err(|e| SandboxError::Denied(format!("receipt encode failed: {e}")))?;
    let mut file = File::from(
        open(
            path,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|e| SandboxError::Denied(format!("receipt open failed: {e}")))?,
    );
    file.write_all(&bytes)
        .map_err(|e| SandboxError::Denied(format!("receipt write failed: {e}")))?;
    Ok(())
}
