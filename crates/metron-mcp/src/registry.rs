//! Allowlisted MCP tool registry routing every execution through `metron-sandbox`
//! with pre-sandbox JSON Schema validation, cancellation propagation, bounded ring-buffer
//! receipt retention, and pluggable `ReceiptSink` persistence.

use crate::{
    executor::run_sandboxed_spec,
    receipt::{BoundedReceiptRing, McpResourceReceipt, ReceiptSink, hex_sha256},
    schema::{MAX_MCP_ARGS_BYTES, serialize_bounded_args, validate_args_against_schema},
};
use metron_sandbox::{AotEnvelopeMeta, CgroupLimits, EngineTunables, LaneALimits};
use rmcp::model::{JsonObject, Tool};
use serde_json::{Map, Value};
use std::{
    collections::BTreeMap,
    fmt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use thiserror::Error;

/// Failure returned by `SandboxedToolRegistry` or `MetronMcpClient`.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum McpBridgeError {
    /// Tool name is missing, malformed, or unregistered.
    #[error("unknown or invalid MCP tool: {0}")]
    UnknownTool(String),
    /// Tool call arguments violated the tool's registered JSON Schema.
    #[error("invalid MCP tool arguments: {0}")]
    InvalidArguments(String),
    /// Invalid tool registration or configuration.
    #[error("invalid MCP tool registration: {0}")]
    InvalidRegistration(String),
    /// In-flight tool execution was cancelled via MCP `notifications/cancelled`.
    #[error("MCP tool execution cancelled")]
    Cancelled,
    /// Underlying sandbox rejected or trapped tool execution.
    #[error("sandbox execution failure: {0}")]
    Sandbox(String),
    /// Durable receipt sink failed to persist an operation receipt.
    #[error("receipt sink failure: {0}")]
    Sink(String),
    /// `rmcp` transport or protocol error.
    #[error("MCP protocol error: {0}")]
    Protocol(String),
}

/// Pinned tool execution backend inside `metron-sandbox` (zero raw shell dispatch).
#[derive(Clone)]
pub enum SandboxedToolSpec {
    /// Executes an AOT-compiled, HMAC/BLAKE3-sealed Wasm component inside Lane A (`wasm_aot`).
    LaneAWasm {
        /// Public MCP tool name.
        name: String,
        /// Human/model description advertised in `tools/list`.
        description: String,
        /// JSON Schema for tool arguments.
        input_schema: Arc<JsonObject>,
        /// Precompiled `.cwasm` artifact bytes.
        cwasm_bytes: Vec<u8>,
        /// Cryptographic `.cwasm` seal envelope.
        meta: AotEnvelopeMeta,
        /// 32-byte HMAC seal verification key (redacted in `Debug`).
        hmac_key: [u8; 32],
        /// Deterministic Wasmtime runtime tunables.
        tunables: EngineTunables,
        /// Fuel, memory, and epoch limits.
        limits: LaneALimits,
        /// Disk-backed scratch directory for the jailed child.
        scratch: PathBuf,
        /// Optional explicit `metron-jail` trampoline path.
        trampoline_bin: Option<PathBuf>,
        /// Whether to enforce `systemd-run --user --collect` cgroup wrapper.
        use_systemd: bool,
    },
    /// Executes a SHA-256-pinned host binary inside Lane B (`native_jail`).
    LaneBNative {
        /// Public MCP tool name.
        name: String,
        /// Human/model description advertised in `tools/list`.
        description: String,
        /// JSON Schema for tool arguments.
        input_schema: Arc<JsonObject>,
        /// Absolute host executable path sealed into `memfd` before execution.
        executable_path: PathBuf,
        /// Expected SHA-256 lowercase hex digest of `executable_path`.
        sha256: String,
        /// Static pinned argv passed before any serialized payload.
        fixed_args: Vec<String>,
        /// Optional workspace directory mounted read-only (or read-write if configured).
        workspace: Option<PathBuf>,
        /// Whether `workspace` is writable inside Landlock.
        writable_workspace: bool,
        /// Disk-backed private scratch directory.
        scratch: PathBuf,
        /// Cgroup limits enforced under `systemd-run --user --collect`.
        cgroup: CgroupLimits,
        /// Optional explicit `metron-jail` trampoline path.
        trampoline_bin: Option<PathBuf>,
        /// Whether to enforce `systemd-run --user --collect` cgroup wrapper.
        use_systemd: bool,
    },
}

impl fmt::Debug for SandboxedToolSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LaneAWasm { name, scratch, .. } => f
                .debug_struct("LaneAWasm")
                .field("name", name)
                .field("hmac_key", &"<redacted>")
                .field("scratch", scratch)
                .finish(),
            Self::LaneBNative {
                name,
                executable_path,
                sha256,
                ..
            } => f
                .debug_struct("LaneBNative")
                .field("name", name)
                .field("executable_path", executable_path)
                .field("sha256", sha256)
                .finish(),
        }
    }
}

impl SandboxedToolSpec {
    /// Tool name exposed via MCP.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::LaneAWasm { name, .. } | Self::LaneBNative { name, .. } => name,
        }
    }

    /// Returns the registered JSON Schema for tool arguments.
    #[must_use]
    pub fn input_schema(&self) -> &JsonObject {
        match self {
            Self::LaneAWasm { input_schema, .. } | Self::LaneBNative { input_schema, .. } => {
                input_schema
            }
        }
    }

    /// Converts this entry into an `rmcp::model::Tool` descriptor.
    #[must_use]
    pub fn to_rmcp_tool(&self) -> Tool {
        match self {
            Self::LaneAWasm {
                name,
                description,
                input_schema,
                ..
            }
            | Self::LaneBNative {
                name,
                description,
                input_schema,
                ..
            } => Tool::new(name.clone(), description.clone(), Arc::clone(input_schema)),
        }
    }
}

/// Allowlisted registry of sandboxed tools, bounded receipt ring, and durable `ReceiptSink`.
#[derive(Debug, Default)]
pub struct SandboxedToolRegistry {
    tools: BTreeMap<String, Arc<SandboxedToolSpec>>,
    receipts: BoundedReceiptRing,
    sink: Option<Arc<dyn ReceiptSink>>,
    seq: AtomicU64,
}

impl SandboxedToolRegistry {
    /// Creates an empty registry (no tools permitted by default).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Configures a custom capacity for the in-memory bounded receipt ring buffer.
    #[must_use]
    pub fn with_receipt_capacity(mut self, capacity: usize) -> Self {
        self.receipts = BoundedReceiptRing::new(capacity);
        self
    }

    /// Attaches a durable `ReceiptSink` (such as [`crate::SqliteReceiptSink`]).
    #[must_use]
    pub fn with_receipt_sink(mut self, sink: Arc<dyn ReceiptSink>) -> Self {
        self.sink = Some(sink);
        self
    }

    /// Registers one pinned Lane A or Lane B tool.
    pub fn register(&mut self, spec: SandboxedToolSpec) -> Result<(), McpBridgeError> {
        let name = spec.name();
        if name.trim().is_empty()
            || name.len() > 128
            || !name
                .chars()
                .all(|c| !c.is_control() && !c.is_whitespace() && c != '/' && c != '\\')
        {
            return Err(McpBridgeError::InvalidRegistration(
                "tool name must be non-empty, <=128 chars, and free of whitespace, control, or path separators".into(),
            ));
        }
        match &spec {
            SandboxedToolSpec::LaneAWasm { cwasm_bytes, .. } if cwasm_bytes.is_empty() => {
                return Err(McpBridgeError::InvalidRegistration(
                    "LaneAWasm cwasm_bytes must be non-empty".into(),
                ));
            }
            SandboxedToolSpec::LaneBNative {
                executable_path,
                sha256,
                ..
            } => {
                if !executable_path.is_absolute() {
                    return Err(McpBridgeError::InvalidRegistration(
                        "LaneBNative executable_path must be absolute".into(),
                    ));
                }
                if sha256.len() != 64
                    || !sha256
                        .bytes()
                        .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
                {
                    return Err(McpBridgeError::InvalidRegistration(
                        "LaneBNative sha256 must be a 64-character lowercase hex digest".into(),
                    ));
                }
            }
            _ => {}
        }
        if self.tools.contains_key(name) {
            return Err(McpBridgeError::InvalidRegistration(format!(
                "duplicate tool name: {name}"
            )));
        }
        self.tools.insert(name.to_owned(), Arc::new(spec));
        Ok(())
    }

    /// Lists registered `rmcp` tool descriptors in deterministic order.
    #[must_use]
    pub fn list_tools(&self) -> Vec<Tool> {
        self.tools.values().map(|t| t.to_rmcp_tool()).collect()
    }

    /// Returns a single registered tool descriptor by name.
    #[must_use]
    pub fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools.get(name).map(|t| t.to_rmcp_tool())
    }

    /// Looks up the execution spec for a registered tool.
    #[must_use]
    pub fn lookup(&self, name: &str) -> Option<Arc<SandboxedToolSpec>> {
        self.tools.get(name).cloned()
    }

    /// Returns a snapshot of bounded recent `McpResourceReceipt`s recorded by this server.
    #[must_use]
    pub fn receipts(&self) -> Vec<McpResourceReceipt> {
        self.receipts.snapshot()
    }

    /// Executes `spec` strictly inside `metron-sandbox`, validating `args` against
    /// `input_schema` and recording a `McpResourceReceipt`.
    pub fn execute_spec(
        &self,
        spec: &SandboxedToolSpec,
        args: &Map<String, Value>,
    ) -> Result<(Value, McpResourceReceipt), McpBridgeError> {
        self.execute_spec_cancellable(spec, args, None)
    }

    /// Cancellable variant of [`Self::execute_spec`]: flipping `cancel` terminates the
    /// jailed child process promptly.
    pub fn execute_spec_cancellable(
        &self,
        spec: &SandboxedToolSpec,
        args: &Map<String, Value>,
        cancel: Option<&AtomicBool>,
    ) -> Result<(Value, McpResourceReceipt), McpBridgeError> {
        validate_args_against_schema(spec.input_schema(), args)?;
        if cancel.is_some_and(|c| c.load(Ordering::Acquire)) {
            return Err(McpBridgeError::Cancelled);
        }

        let seq = self
            .seq
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map(|prev| prev + 1)
            .map_err(|_| McpBridgeError::Protocol("execution sequence counter exhausted".into()))?;
        let args_json = serialize_bounded_args(args, MAX_MCP_ARGS_BYTES)?;
        let args_digest = hex_sha256(&args_json);

        let (payload, receipt) = run_sandboxed_spec(seq, spec, &args_json, args_digest, cancel)?;
        self.receipts.push(receipt.clone());
        if let Some(sink) = &self.sink {
            sink.record(&receipt).map_err(McpBridgeError::Sink)?;
        }
        Ok((payload, receipt))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_rejects_malformed_name_relative_path_and_invalid_sha256() {
        let mut reg = SandboxedToolRegistry::new();
        let empty_schema = Arc::new(Map::new());
        let mk_native = |name: &str, path: &str, sha: &str| SandboxedToolSpec::LaneBNative {
            name: name.to_owned(),
            description: "test".to_owned(),
            input_schema: Arc::clone(&empty_schema),
            executable_path: PathBuf::from(path),
            sha256: sha.to_owned(),
            fixed_args: Vec::new(),
            workspace: None,
            writable_workspace: false,
            scratch: PathBuf::from("/tmp/scratch"),
            cgroup: CgroupLimits::default(),
            trampoline_bin: None,
            use_systemd: false,
        };
        let valid_sha = "a".repeat(64);
        assert!(matches!(
            reg.register(mk_native("../bad", "/bin/sh", &valid_sha)),
            Err(McpBridgeError::InvalidRegistration(_))
        ));
        assert!(matches!(
            reg.register(mk_native("rel_tool", "bin/sh", &valid_sha)),
            Err(McpBridgeError::InvalidRegistration(_))
        ));
        assert!(matches!(
            reg.register(mk_native("bad_sha", "/bin/sh", "ABCDEF")),
            Err(McpBridgeError::InvalidRegistration(_))
        ));
    }
}
