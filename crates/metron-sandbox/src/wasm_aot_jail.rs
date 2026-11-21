//! Child-process Lane A (`wasm_aot`) confinement: `systemd-run --user --collect` or hardened
//! direct supervisor (real wall-clock timeout + `SIGKILL` + `rlimit`), strict `Landlock`,
//! zero-key-on-disk anonymous stdin pipe streaming, pre-deserialize baseline seccomp,
//! `F_SEAL_WRITE` `memfd`, stacked post-load W^X `PROT_EXEC` seccomp, and JSON receipt.

use crate::{
    CgroupLimits, DirectRlimits, LandlockPolicy, SealedMemfd, apply_landlock, apply_rlimits,
    check_scratch_space,
    direct_supervisor::run_direct_supervised_cancellable,
    error::SandboxError,
    memfd::{replace_stdin_with, require_disk_backed, seal_from_reader},
    resolve_trampoline, systemd_cg,
    wasm_aot::{
        AotEnvelopeMeta, EngineTunables, LaneALimits, PINNED_WASMTIME_VERSION, ResourceReceipt,
        execute_sealed_cwasm, host_target_triple,
    },
};
use metron_wasm_aot_loader::{AotSeal, compute_seal_tag};
use rustix::fs::{Mode, OFlags, open};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

static LANE_A_SEQ: AtomicU64 = AtomicU64::new(0);

/// Complete specification passed over a `0o600` private file to `--lane-a-policy`.
///
/// SECURITY: Contains ZERO secret key material — the 32-byte HMAC key is verified in
/// the parent process and delivered solely over the anonymous stdin pipe (never on disk).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LaneASpec {
    /// Cryptographic seal envelope for the streamed `.cwasm` component.
    pub meta: AotEnvelopeMeta,
    /// Deterministic engine tunables digest-checked against `meta`.
    pub tunables: EngineTunables,
    /// Resource limits (`fuel_budget`, `max_memory_bytes`, `timeout_ms`, etc.).
    pub fuel_budget: u64,
    /// Maximum Wasm linear memory bytes.
    pub max_memory_bytes: usize,
    /// Maximum table elements.
    pub max_table_elements: usize,
    /// Execution deadline in milliseconds.
    pub timeout_ms: u32,
    /// Maximum `.cwasm` payload bytes.
    pub max_cwasm_bytes: usize,
    /// Maximum captured stdout/stderr bytes.
    pub max_output_bytes: usize,
    /// Optional guest stdin bytes.
    pub stdin_bytes: Vec<u8>,
    /// Landlock policy applied before component deserialization & execution.
    pub landlock: LandlockPolicy,
    /// Optional `setrlimit` / `PR_SET_PDEATHSIG` bounds enforced inside the Lane A child.
    #[serde(default)]
    pub rlimits: Option<DirectRlimits>,
}

/// Request bundle for running a sealed `.cwasm` component inside a Lane A jail.
pub struct LaneARequest<'a> {
    /// Immutable `F_SEAL_WRITE` `memfd` holding `.cwasm` bytes.
    pub cwasm_memfd: &'a SealedMemfd,
    /// Cryptographic seal envelope accompanying the `.cwasm` bytes.
    pub meta: &'a AotEnvelopeMeta,
    /// 32-byte HMAC seal key (verified in parent + streamed via anonymous pipe, never written to disk).
    pub key: &'a [u8; 32],
    /// Deterministic engine tunables expected by the host.
    pub tunables: &'a EngineTunables,
    /// Fuel, memory, epoch, and cgroup bounds.
    pub limits: &'a LaneALimits,
    /// Disk-backed private scratch directory.
    pub scratch: &'a Path,
    /// Optional guest standard input bytes.
    pub stdin_bytes: &'a [u8],
    /// Optional explicit trampoline path override.
    pub trampoline_bin: Option<&'a Path>,
    /// Whether to wrap execution in `systemd-run --user --collect`.
    pub use_systemd: bool,
}

/// Pre-verifies `.cwasm` BLAKE3 + HMAC-SHA256 + host tuple in the parent before launch.
fn verify_parent_seal(req: &LaneARequest<'_>) -> Result<(), SandboxError> {
    let target = host_target_triple()?;
    let expected_tunables = req.tunables.digest();
    let (meta_tunables, expected_blake3, expected_tag) = req.meta.decode_parts()?;
    if req.meta.wasmtime_version != PINNED_WASMTIME_VERSION
        || req.meta.target_triple != target
        || meta_tunables != expected_tunables
    {
        return Err(SandboxError::Denied(
            "cwasm host/tunables metadata mismatch".into(),
        ));
    }
    let max_bytes = req.limits.max_cwasm_bytes.min(req.tunables.max_cwasm_bytes);
    let mut cloned = req
        .cwasm_memfd
        .as_file()
        .try_clone()
        .map_err(|_| SandboxError::Denied("cwasm memfd clone failed".into()))?;
    cloned
        .seek(SeekFrom::Start(0))
        .map_err(|_| SandboxError::Denied("cwasm memfd seek failed".into()))?;
    let mut hasher = blake3::Hasher::new();
    let mut total = 0usize;
    let mut buf = [0u8; 8192];
    loop {
        let n = cloned
            .read(&mut buf)
            .map_err(|_| SandboxError::Denied("cwasm memfd read failed".into()))?;
        if n == 0 {
            break;
        }
        total = total.saturating_add(n);
        if total > max_bytes {
            return Err(SandboxError::Denied("cwasm exceeds size limit".into()));
        }
        hasher.update(&buf[..n]);
    }
    if *hasher.finalize().as_bytes() != expected_blake3 {
        return Err(SandboxError::Denied("cwasm blake3 mismatch".into()));
    }
    let seal = AotSeal {
        wasmtime_version: &req.meta.wasmtime_version,
        target_triple: &req.meta.target_triple,
        engine_tunables_digest: meta_tunables,
        cwasm_blake3: expected_blake3,
        hmac_tag: expected_tag,
    };
    let computed = compute_seal_tag(req.key, &seal).map_err(|e| SandboxError::Denied(e.into()))?;
    // Constant-time tag comparison.
    let diff = computed
        .iter()
        .zip(expected_tag.iter())
        .fold(0u8, |acc, (&a, &b)| acc | (a ^ b));
    if diff != 0 {
        return Err(SandboxError::Denied("cwasm hmac seal invalid".into()));
    }
    Ok(())
}

/// Runs a compiled `.cwasm` artifact inside an isolated child jail (`systemd-run --user --collect`
/// or hardened direct supervisor with real timeout + `rlimit`, strict `Landlock`, two-phase
/// seccomp, `F_SEAL_WRITE` `memfd`, and zero secrets on scratch disk).
pub fn run_lane_a_jail(req: &LaneARequest<'_>) -> Result<ResourceReceipt, SandboxError> {
    run_lane_a_jail_cancellable(req, None)
}

/// Cancellable variant of [`run_lane_a_jail`]: flipping `cancel` immediately kills the child.
pub fn run_lane_a_jail_cancellable(
    req: &LaneARequest<'_>,
    cancel: Option<&AtomicBool>,
) -> Result<ResourceReceipt, SandboxError> {
    require_disk_backed(req.scratch)?;
    check_scratch_space(req.scratch, req.limits.cgroup.scratch_reserve_bytes)?;
    verify_parent_seal(req)?;

    let rlimits = if req.use_systemd {
        DirectRlimits::systemd_supplement(&req.limits.cgroup)
    } else {
        lane_a_direct_rlimits(req.tunables, &req.limits.cgroup)
    };
    let spec = LaneASpec {
        meta: req.meta.clone(),
        tunables: *req.tunables,
        fuel_budget: req.limits.fuel_budget,
        max_memory_bytes: req.limits.max_memory_bytes,
        max_table_elements: req.limits.max_table_elements,
        timeout_ms: req.limits.timeout_ms,
        max_cwasm_bytes: req.limits.max_cwasm_bytes,
        max_output_bytes: req.limits.max_output_bytes,
        stdin_bytes: req.stdin_bytes.to_vec(),
        landlock: LandlockPolicy::lane_b(Vec::new(), vec![req.scratch.to_path_buf()]),
        rlimits: Some(rlimits),
    };

    let seq = LANE_A_SEQ.fetch_add(1, Ordering::Relaxed);
    let spec_path = req
        .scratch
        .join(format!(".metron-lane-a-{}-{seq}.json", std::process::id()));
    let encoded = serde_json::to_vec(&spec)
        .map_err(|e| SandboxError::Denied(format!("lane_a spec encode failed: {e}")))?;
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&spec_path)
        .map_err(|_| SandboxError::Denied("lane_a spec creation failed".into()))?;
    f.write_all(&encoded)
        .and_then(|()| f.sync_all())
        .map_err(|_| SandboxError::Denied("lane_a spec write failed".into()))?;

    let _guard = CleanupFile(spec_path.clone());
    let (bin, mut args) = resolve_trampoline(req.trampoline_bin)?;
    args.push("--lane-a-policy".to_owned());
    args.push(spec_path.to_string_lossy().into_owned());

    // Stream 32-byte key + .cwasm bytes exclusively over the anonymous stdin pipe (zero secrets on disk).
    let key_prefix = Some(req.key.to_vec());
    let done = if req.use_systemd {
        systemd_cg::run_in_systemd_cgroup_cancellable(
            &req.limits.cgroup,
            req.scratch,
            &bin,
            &args,
            key_prefix,
            Some(req.cwasm_memfd.as_file()),
            cancel,
        )?
    } else {
        run_direct_supervised_cancellable(
            &req.limits.cgroup,
            req.scratch,
            &bin,
            &args,
            key_prefix,
            Some(req.cwasm_memfd.as_file()),
            None,
            cancel,
        )?
    };

    if done.timed_out {
        return Err(SandboxError::Denied(
            "lane_a child timed out and was killed".into(),
        ));
    }
    if !done.success {
        let msg = String::from_utf8_lossy(&done.stderr);
        return Err(SandboxError::Denied(format!(
            "lane_a child failed (code {:?}): {}",
            done.exit_code,
            msg.trim()
        )));
    }
    serde_json::from_slice::<ResourceReceipt>(&done.stdout)
        .map_err(|e| SandboxError::Denied(format!("lane_a receipt decode failed: {e}")))
}

fn lane_a_direct_rlimits(tunables: &EngineTunables, cgroup: &CgroupLimits) -> DirectRlimits {
    // Pooling allocator reserves virtual address space (PROT_NONE) proportional to
    // pool_max_instances * memory_reservation_bytes; size RLIMIT_AS to fit the pool + runtime.
    let pool_va = u64::from(tunables.pool_max_instances.max(1))
        .saturating_mul(2)
        .saturating_mul(
            tunables
                .memory_reservation_bytes
                .saturating_add(tunables.memory_guard_size_bytes.saturating_mul(2)),
        )
        .saturating_add(512 * 1024 * 1024);
    let mut rl = DirectRlimits::from_cgroup_limits(cgroup);
    rl.address_space_bytes = Some(cgroup.memory_max_bytes.max(pool_va));
    // Note: without userns, RLIMIT_NPROC counts all threads of the real UID; keep nproc
    // unbounded in Lane A direct mode so the epoch ticker thread cannot fail with EAGAIN
    // on busy desktop hosts (cgroup TasksMax enforces threads when systemd is active).
    rl.nproc = None;
    rl
}

/// Entrypoint run inside the `metron-jail --lane-a-policy SPEC.json` child process.
pub fn run_trampoline_from_spec(spec_path: &Path) -> ExitCode {
    match run_inner(spec_path) {
        Ok(receipt) => match serde_json::to_vec(&receipt) {
            Ok(bytes) if std::io::stdout().write_all(&bytes).is_ok() => ExitCode::SUCCESS,
            _ => ExitCode::from(125),
        },
        Err(err) => {
            eprintln!("metron-jail lane_a: {err}");
            ExitCode::from(126)
        }
    }
}

fn run_inner(spec_path: &Path) -> Result<ResourceReceipt, SandboxError> {
    let raw = {
        let file = File::from(
            open(
                spec_path,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| SandboxError::Denied("lane_a spec unreadable".into()))?,
        );
        let mut buf = Vec::new();
        file.take(4 * 1024 * 1024)
            .read_to_end(&mut buf)
            .map_err(|_| SandboxError::Denied("lane_a spec read failed".into()))?;
        buf
    };
    let _ = std::fs::remove_file(spec_path);

    let spec: LaneASpec = serde_json::from_slice(&raw)
        .map_err(|e| SandboxError::Denied(format!("invalid lane_a spec: {e}")))?;

    let (key, cwasm_memfd) = {
        let stdin = std::io::stdin();
        let mut locked = stdin.lock();
        let mut key_buf = [0u8; 32];
        locked
            .read_exact(&mut key_buf)
            .map_err(|_| SandboxError::Denied("lane_a key pipe read failed".into()))?;
        let memfd = seal_from_reader("metron-cwasm", locked, None, true)?;
        (key_buf, memfd)
    };
    let dev_null = open("/dev/null", OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty())
        .map_err(|_| SandboxError::Denied("failed to open /dev/null".into()))?;
    replace_stdin_with(rustix::fd::AsFd::as_fd(&dev_null))?;
    drop(dev_null);

    if let Some(rlimits) = &spec.rlimits {
        apply_rlimits(rlimits)?;
    }

    rustix::thread::set_no_new_privs(true)
        .map_err(|e| SandboxError::Denied(format!("PR_SET_NO_NEW_PRIVS failed: {e}")))?;
    apply_landlock(&spec.landlock)?;

    let limits = LaneALimits {
        fuel_budget: spec.fuel_budget,
        max_memory_bytes: spec.max_memory_bytes,
        max_table_elements: spec.max_table_elements,
        timeout_ms: spec.timeout_ms,
        max_cwasm_bytes: spec.max_cwasm_bytes,
        max_output_bytes: spec.max_output_bytes,
        cgroup: CgroupLimits::default(),
    };
    execute_sealed_cwasm(
        &cwasm_memfd,
        &spec.meta,
        &key,
        &spec.tunables,
        &limits,
        &spec.stdin_bytes,
        true,
    )
}

struct CleanupFile(PathBuf);
impl Drop for CleanupFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
