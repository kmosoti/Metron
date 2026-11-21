//! Deterministic configuration, cryptographic envelope, and receipt types for Lane A (`wasm_aot`).

use crate::{CgroupLimits, error::SandboxError};
use serde::{Deserialize, Serialize};
use wasmtime::{InstanceAllocationStrategy, PoolingAllocationConfig};

/// Pinned Wasmtime version bound into every AOT cryptographic seal.
pub const PINNED_WASMTIME_VERSION: &str = "51.0.0";

/// Returns the canonical host target triple validated against `.cwasm` headers.
pub fn host_target_triple() -> Result<&'static str, SandboxError> {
    #[cfg(all(target_arch = "x86_64", target_os = "linux", target_env = "gnu"))]
    return Ok("x86_64-unknown-linux-gnu");
    #[cfg(all(target_arch = "x86_64", target_os = "linux", target_env = "musl"))]
    return Ok("x86_64-unknown-linux-musl");
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    return Ok("aarch64-unknown-linux-gnu");
    #[cfg(not(any(
        all(target_arch = "x86_64", target_os = "linux", target_env = "gnu"),
        all(target_arch = "x86_64", target_os = "linux", target_env = "musl"),
        all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"),
    )))]
    Err(SandboxError::Denied(
        "unsupported host triple for wasm_aot".into(),
    ))
}

const fn default_max_cwasm_bytes() -> usize {
    64 * 1024 * 1024
}

const fn default_max_memory_bytes() -> usize {
    32 * 1024 * 1024
}

const fn default_max_table_elements() -> usize {
    10_000
}

const fn default_pool_max_instances() -> u32 {
    4
}

/// Deterministic runtime engine tunables hashed into the cryptographic `.cwasm` seal.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EngineTunables {
    /// Enforce Wasm instruction fuel metering (`consume_fuel(true)`).
    pub consume_fuel: bool,
    /// Enforce epoch-based preemption (`epoch_interruption(true)`).
    pub epoch_interruption: bool,
    /// Maximum Wasm execution stack in bytes.
    pub max_wasm_stack_bytes: usize,
    /// Initial virtual memory reservation per linear memory in bytes.
    pub memory_reservation_bytes: u64,
    /// Linear memory guard region size in bytes.
    pub memory_guard_size_bytes: u64,
    /// Maximum `.cwasm` artifact bytes accepted by the loader (hashed into seal).
    #[serde(default = "default_max_cwasm_bytes")]
    pub max_cwasm_bytes: usize,
    /// Maximum linear memory bytes permitted per pooled instance.
    #[serde(default = "default_max_memory_bytes")]
    pub max_memory_bytes: usize,
    /// Maximum table elements permitted per pooled instance.
    #[serde(default = "default_max_table_elements")]
    pub max_table_elements: usize,
    /// Maximum concurrent component instances in the Wasmtime pooling allocator.
    #[serde(default = "default_pool_max_instances")]
    pub pool_max_instances: u32,
}

impl Default for EngineTunables {
    fn default() -> Self {
        Self {
            consume_fuel: true,
            epoch_interruption: true,
            max_wasm_stack_bytes: 512 * 1024,
            memory_reservation_bytes: 32 * 1024 * 1024,
            memory_guard_size_bytes: 64 * 1024,
            max_cwasm_bytes: default_max_cwasm_bytes(),
            max_memory_bytes: default_max_memory_bytes(),
            max_table_elements: default_max_table_elements(),
            pool_max_instances: default_pool_max_instances(),
        }
    }
}

impl EngineTunables {
    /// Computes the canonical BLAKE3 digest of these runtime tunables, including
    /// `max_cwasm_bytes`, pooling bounds, and host arch/OS/pointer-width configuration.
    #[must_use]
    pub fn digest(&self) -> [u8; 32] {
        let mut h = blake3::Hasher::new();
        h.update(b"metron-wasm-engine-tunables/v2\0");
        h.update(std::env::consts::ARCH.as_bytes());
        h.update(b"\0");
        h.update(std::env::consts::OS.as_bytes());
        h.update(b"\0");
        h.update(&usize::BITS.to_le_bytes());
        h.update(&[
            u8::from(self.consume_fuel),
            u8::from(self.epoch_interruption),
        ]);
        h.update(&(self.max_wasm_stack_bytes as u64).to_le_bytes());
        h.update(&self.memory_reservation_bytes.to_le_bytes());
        h.update(&self.memory_guard_size_bytes.to_le_bytes());
        h.update(&(self.max_cwasm_bytes as u64).to_le_bytes());
        h.update(&(self.max_memory_bytes as u64).to_le_bytes());
        h.update(&(self.max_table_elements as u64).to_le_bytes());
        h.update(&self.pool_max_instances.to_le_bytes());
        *h.finalize().as_bytes()
    }

    /// Builds a Wasmtime runtime `Config` with `InstanceAllocationStrategy::Pooling`
    /// bounded to `max_memory_bytes`, `max_table_elements`, and `pool_max_instances`.
    pub fn to_wasmtime_config(&self) -> Result<wasmtime::Config, SandboxError> {
        if !self.consume_fuel || !self.epoch_interruption {
            return Err(SandboxError::Denied(
                "wasm_aot requires consume_fuel and epoch_interruption".into(),
            ));
        }
        let max_mem = self.max_memory_bytes.max(64 * 1024);
        let reservation = self.memory_reservation_bytes.max(max_mem as u64);
        let instances = self.pool_max_instances.max(1);

        let mut pool = PoolingAllocationConfig::default();
        pool.total_component_instances(instances)
            .total_core_instances(instances.saturating_mul(2))
            .total_memories(instances.saturating_mul(2))
            .total_tables(instances.saturating_mul(2))
            .total_stacks(instances)
            .max_memories_per_component(4)
            .max_tables_per_component(8)
            .max_memory_size(max_mem)
            .table_elements(self.max_table_elements.max(64));

        let mut cfg = wasmtime::Config::new();
        cfg.consume_fuel(true);
        cfg.epoch_interruption(true);
        cfg.max_wasm_stack(self.max_wasm_stack_bytes);
        cfg.memory_reservation(reservation);
        cfg.memory_guard_size(self.memory_guard_size_bytes);
        cfg.allocation_strategy(InstanceAllocationStrategy::Pooling(pool));
        Ok(cfg)
    }
}

/// Serializable metadata envelope accompanying a sealed `.cwasm` component binary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AotEnvelopeMeta {
    /// Pinned Wasmtime version string.
    pub wasmtime_version: String,
    /// Host target triple.
    pub target_triple: String,
    /// Hex-encoded BLAKE3 digest of `EngineTunables`.
    pub engine_tunables_digest_hex: String,
    /// Hex-encoded BLAKE3 digest of `.cwasm` bytes.
    pub cwasm_blake3_hex: String,
    /// Hex-encoded domain-separated HMAC-SHA256 seal tag.
    pub hmac_tag_hex: String,
}

/// Decoded 32-byte digests and HMAC tag from [`AotEnvelopeMeta`].
pub type DecodedSealParts = ([u8; 32], [u8; 32], [u8; 32]);

impl AotEnvelopeMeta {
    /// Decodes hex fields and returns `(tunables_digest, cwasm_blake3, hmac_tag)`.
    pub fn decode_parts(&self) -> Result<DecodedSealParts, SandboxError> {
        Ok((
            decode_hex_32(&self.engine_tunables_digest_hex)?,
            decode_hex_32(&self.cwasm_blake3_hex)?,
            decode_hex_32(&self.hmac_tag_hex)?,
        ))
    }
}

/// Resource limits enforced on a Lane A (`wasm_aot`) invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaneALimits {
    /// Deterministic fuel allowance.
    pub fuel_budget: u64,
    /// Maximum linear memory bytes permitted by `StoreLimits`.
    pub max_memory_bytes: usize,
    /// Maximum table elements permitted by `StoreLimits`.
    pub max_table_elements: usize,
    /// Wall-clock deadline in milliseconds (enforced via epochs + cgroup).
    pub timeout_ms: u32,
    /// Maximum `.cwasm` binary bytes permitted during verification.
    pub max_cwasm_bytes: usize,
    /// Maximum captured stdout/stderr bytes per stream.
    pub max_output_bytes: usize,
    /// Underlying Lane B cgroup caps when spawned as a jailed service.
    pub cgroup: CgroupLimits,
}

impl Default for LaneALimits {
    fn default() -> Self {
        Self {
            fuel_budget: 2_000_000,
            max_memory_bytes: 32 * 1024 * 1024,
            max_table_elements: 10_000,
            timeout_ms: 5_000,
            max_cwasm_bytes: 64 * 1024 * 1024,
            max_output_bytes: 64 * 1024,
            cgroup: CgroupLimits::default(),
        }
    }
}

/// Deterministic accounting receipt emitted by every Lane A (`wasm_aot`) execution.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResourceReceipt {
    /// Exit status code (`0` on clean exit, `1` on WASI error, `124` on trap).
    pub exit_code: i32,
    /// Exact deterministic WebAssembly fuel units consumed.
    pub fuel_consumed: u64,
    /// High-water linear memory bytes approved for the Store.
    pub peak_memory_bytes: u64,
    /// SHA-256 hex digest of captured stdout bytes.
    pub stdout_sha256: String,
    /// SHA-256 hex digest of captured stderr bytes.
    pub stderr_sha256: String,
    /// Captured stdout bytes (bounded by `max_output_bytes`).
    pub stdout: Vec<u8>,
    /// Captured stderr bytes (bounded by `max_output_bytes`).
    pub stderr: Vec<u8>,
    /// Verified `.cwasm` BLAKE3 digest.
    pub cwasm_blake3: [u8; 32],
    /// Verified `EngineTunables` BLAKE3 digest.
    pub engine_tunables_digest: [u8; 32],
    /// Deterministic trap classifier if execution trapped (`None` on clean exit).
    pub trapped: Option<String>,
}

/// Decodes a 64-character hex string into a 32-byte digest or seal key.
pub fn decode_hex_32(hex: &str) -> Result<[u8; 32], SandboxError> {
    if hex.len() != 64 {
        return Err(SandboxError::Denied("expected 32-byte hex digest".into()));
    }
    let mut out = [0u8; 32];
    for (idx, chunk) in hex.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let hi = hex_nibble(chunk[0])?;
        let lo = hex_nibble(chunk[1])?;
        out[idx] = (hi << 4) | lo;
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> Result<u8, SandboxError> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(SandboxError::Denied("invalid hex byte".into())),
    }
}

/// Encodes raw bytes as lowercase hex.
#[must_use]
pub fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}
