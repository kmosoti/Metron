//! ADR-0004 `ResourceReceipt` types, bounded in-memory ring buffer, and durable
//! `ReceiptSink` persistence into `metron-store-sqlite`.

use metron_protocol::{
    ActualUsage, BoundedText, DecimalUsage, Receipt, ReceiptData, ReceiptOutput, SchemaVersion,
    TaskId, UsageVector, canonical_bytes, content_digest, structural_digest,
};
use metron_store_sqlite::SqliteStore;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{collections::VecDeque, fmt, sync::Mutex};

/// Default maximum receipts retained in the in-memory bounded ring buffer.
pub const DEFAULT_RECEIPT_RING_CAPACITY: usize = 1024;

/// Isolation lane used by `metron-mcp` to execute a registered tool.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpLaneKind {
    /// Lane A (`wasm_aot`) Wasmtime 51 AOT inside Landlock + two-phase seccomp (`PROT_EXEC` blocked).
    WasmAot,
    /// Lane B (`native_jail`) unprivileged `systemd-run --user` + Landlock + seccomp + sealed `memfd`.
    NativeJail,
}

/// Deterministic operation receipt emitted for every MCP tool invocation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct McpResourceReceipt {
    /// Registered tool name invoked over MCP.
    pub tool_name: String,
    /// Sandbox isolation lane enforced on this invocation.
    pub lane: McpLaneKind,
    /// Exit status (`0` on success, non-zero on WASI/tool error or trap).
    pub exit_code: i32,
    /// Deterministic fuel units consumed (`0` for Lane B).
    pub fuel_consumed: u64,
    /// Peak linear memory bytes allocated (`0` for Lane B).
    pub peak_memory_bytes: u64,
    /// SHA-256 hex digest of captured standard output.
    pub stdout_sha256: String,
    /// SHA-256 hex digest of captured standard error.
    pub stderr_sha256: String,
    /// SHA-256 hex digest of canonical JSON arguments passed to the tool.
    pub args_digest: String,
    /// Deterministic trap classifier if execution trapped (`None` when untrapped).
    pub trapped: Option<String>,
    /// Validated `metron-protocol` receipt binding the invocation into the ledger format.
    pub protocol_receipt: Option<Receipt>,
}

/// Pluggable durable sink receiving every `McpResourceReceipt` emitted by the bridge.
pub trait ReceiptSink: Send + Sync + fmt::Debug {
    /// Persists one `McpResourceReceipt` durably (for example into `metron-store-sqlite`).
    fn record(&self, receipt: &McpResourceReceipt) -> Result<(), String>;
}

/// Durable `ReceiptSink` persisting `McpResourceReceipt`s directly into `metron-store-sqlite`.
pub struct SqliteReceiptSink {
    store: Mutex<SqliteStore>,
    task_id: String,
}

impl fmt::Debug for SqliteReceiptSink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SqliteReceiptSink")
            .field("task_id", &self.task_id)
            .finish()
    }
}

impl SqliteReceiptSink {
    /// Wraps a writable `SqliteStore` to persist MCP receipts under `task_id`.
    pub fn new(store: SqliteStore, task_id: impl Into<String>) -> Result<Self, String> {
        store
            .ensure_external_receipts_table()
            .map_err(|e| format!("sqlite external_receipts setup failed: {e:?}"))?;
        Ok(Self {
            store: Mutex::new(store),
            task_id: task_id.into(),
        })
    }

    /// Reads back all persisted `McpResourceReceipt` rows recorded for this sink's `task_id`.
    pub fn persisted_count(&self) -> Result<usize, String> {
        let guard = self.store.lock().unwrap_or_else(|e| e.into_inner());
        let rows = guard
            .list_external_receipts(Some(&self.task_id))
            .map_err(|e| format!("sqlite receipt query failed: {e:?}"))?;
        Ok(rows.len())
    }
}

impl ReceiptSink for SqliteReceiptSink {
    fn record(&self, receipt: &McpResourceReceipt) -> Result<(), String> {
        let json =
            serde_json::to_string(receipt).map_err(|e| format!("receipt serialize failed: {e}"))?;
        let digest = hex_sha256(json.as_bytes());
        let kind = match receipt.lane {
            McpLaneKind::WasmAot => "mcp_lane_a_wasm",
            McpLaneKind::NativeJail => "mcp_lane_b_native",
        };
        let guard = self.store.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .record_external_receipt(kind, &self.task_id, &receipt.tool_name, &json, &digest)
            .map(|_| ())
            .map_err(|e| format!("sqlite receipt persist failed: {e:?}"))
    }
}

/// Poison-safe bounded ring buffer retaining the most recent `capacity` receipts in memory.
#[derive(Debug)]
pub struct BoundedReceiptRing {
    inner: Mutex<VecDeque<McpResourceReceipt>>,
    capacity: usize,
}

impl Default for BoundedReceiptRing {
    fn default() -> Self {
        Self::new(DEFAULT_RECEIPT_RING_CAPACITY)
    }
}

impl BoundedReceiptRing {
    /// Creates a bounded ring buffer with at least 1 slot.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.max(1);
        Self {
            inner: Mutex::new(VecDeque::with_capacity(cap.min(256))),
            capacity: cap,
        }
    }

    /// Appends one receipt, evicting the oldest entry if full (never panics on poison).
    pub fn push(&self, receipt: McpResourceReceipt) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if guard.len() >= self.capacity {
            guard.pop_front();
        }
        guard.push_back(receipt);
    }

    /// Returns a cloned snapshot of retained receipts in chronological order.
    #[must_use]
    pub fn snapshot(&self) -> Vec<McpResourceReceipt> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.iter().cloned().collect()
    }
}

/// Builds a validated `metron_protocol::Receipt` from an MCP sandbox outcome.
pub(crate) fn build_protocol_receipt(
    seq: u64,
    tool_name: &str,
    args_digest: &str,
    stdout: &[u8],
    ok: bool,
    diagnostic: &str,
) -> Option<Receipt> {
    let task_id = TaskId::new("mcp_sandbox_bridge").ok()?;
    let producer_receipt_id = metron_protocol::ProducerReceiptId::new(format!("mcp_{seq}")).ok()?;
    let intent_id = structural_digest("mcp-intent/v1", &[tool_name.as_bytes()]).ok()?;
    let dispatch_id = structural_digest(
        "mcp-dispatch/v1",
        &[
            &seq.to_le_bytes(),
            tool_name.as_bytes(),
            args_digest.as_bytes(),
        ],
    )
    .ok()?;
    let action_digest = structural_digest("mcp-tool-action/v1", &[tool_name.as_bytes()]).ok()?;

    let output = if ok {
        let preview: String = String::from_utf8_lossy(stdout).chars().take(128).collect();
        let text = BoundedText::new(if preview.is_empty() {
            "ok".to_owned()
        } else {
            preview
        })
        .ok()?;
        ReceiptOutput::Fixture {
            bytes: text,
            digest: content_digest(stdout),
        }
    } else {
        let diag: String = diagnostic.chars().take(256).collect();
        ReceiptOutput::Failure {
            diagnostic: BoundedText::new(if diag.is_empty() {
                "sandbox tool execution failed".to_owned()
            } else {
                diag
            })
            .ok()?,
        }
    };

    let usage = if ok {
        ActualUsage::Known {
            usage: UsageVector {
                model_requests: DecimalUsage::new("0").ok()?,
                tool_calls: DecimalUsage::new("1").ok()?,
                spend_micros_usd: DecimalUsage::new("0").ok()?,
            },
        }
    } else {
        ActualUsage::Unknown {
            reason: BoundedText::new("mcp sandbox failure").ok()?,
            evidence: None,
        }
    };

    let receipt = Receipt::new(ReceiptData {
        schema_version: SchemaVersion::V1,
        producer_receipt_id,
        task_id,
        intent_id,
        dispatch_id,
        action_digest,
        usage,
        output,
    })
    .ok()?;
    let _ = canonical_bytes(&receipt, metron_protocol::MAX_ENVELOPE).ok()?;
    Some(receipt)
}

pub(crate) fn hex_sha256(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    Sha256::digest(bytes)
        .iter()
        .flat_map(|&b| [H[(b >> 4) as usize] as char, H[(b & 0x0f) as usize] as char])
        .collect()
}
