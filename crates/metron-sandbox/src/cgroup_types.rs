//! Shared cgroup / direct-supervisor limit and completion types.

use crate::jail_hardening::SandboxReceipt;

/// Cgroup v2 resource limits applied to every transient `systemd-run --user` unit
/// and mapped to `setrlimit` bounds in direct-supervisor mode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CgroupLimits {
    /// Transient systemd unit name (`metron-...`).
    pub unit_name: String,
    /// Hard memory ceiling (`MemoryMax=<bytes>`, `MemorySwapMax=0`).
    pub memory_max_bytes: u64,
    /// Maximum number of tasks/threads (`TasksMax=<n>`).
    pub tasks_max: u16,
    /// CPU quota percentage (`CPUQuota=<pct>%`, e.g., `200`).
    pub cpu_quota_percent: u16,
    /// Per-file size limit (`LimitFSIZE=<bytes>`).
    pub limit_fsize_bytes: u64,
    /// Wall-clock deadline in milliseconds (`RuntimeMaxSec` + parent kill).
    pub timeout_ms: u32,
    /// Maximum stdout/stderr bytes captured per stream.
    pub output_limit: usize,
    /// Minimum free disk reserve required on `scratch` (bytes).
    pub scratch_reserve_bytes: u64,
}

impl Default for CgroupLimits {
    fn default() -> Self {
        Self {
            unit_name: format!("metron-sandbox-{}", std::process::id()),
            memory_max_bytes: 512 * 1024 * 1024,
            tasks_max: 64,
            cpu_quota_percent: 200,
            limit_fsize_bytes: 67_108_864,
            timeout_ms: 10_000,
            output_limit: 64 * 1024,
            scratch_reserve_bytes: 2 * 1024 * 1024 * 1024,
        }
    }
}

/// Output captured from a completed cgroup- or rlimit-bounded execution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CgroupCompleted {
    /// Captured standard output (bounded by `output_limit`).
    pub stdout: Vec<u8>,
    /// Captured standard error (bounded by `output_limit`).
    pub stderr: Vec<u8>,
    /// True if stdout or stderr exceeded `output_limit`.
    pub truncated: bool,
    /// Exit status code if exited normally.
    pub exit_code: Option<i32>,
    /// True if terminated due to wall-clock deadline timeout or cancellation.
    pub timed_out: bool,
    /// True only when exited with status 0 and not timed out.
    pub success: bool,
    /// Pre-exec sandbox enforcement attestation recorded by `metron-jail`, when available.
    pub receipt: Option<SandboxReceipt>,
}
