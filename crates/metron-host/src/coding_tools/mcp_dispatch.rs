//! Host-side `McpToolDispatcher` and `LaneAWasm` execution wired behind `AuthorizedEffect`
//! with durable `metron-store-sqlite` `ResourceReceipt` persistence.

use metron_harness::policy::AuthorizedEffect;
use metron_mcp::{McpCallOutcome, MetronMcpServer, SandboxedToolRegistry, SqliteReceiptSink};
use metron_protocol::content_digest;
use metron_sandbox::{LaneARequest, ResourceReceipt, run_lane_a_jail};
use metron_store_sqlite::SqliteStore;
use serde_json::{Map, Value};
use std::{path::Path, sync::Arc};

/// Host-side MCP & Lane A Wasm dispatcher enforcing `AuthorizedEffect` authority and
/// persisting every `ResourceReceipt` into `metron-store-sqlite`.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct McpToolDispatcher {
    registry: Arc<SandboxedToolRegistry>,
}

#[allow(dead_code)]
impl McpToolDispatcher {
    /// Builds a dispatcher from an allowlisted `SandboxedToolRegistry` (typically backed by
    /// [`SqliteReceiptSink`] for durable persistence).
    #[must_use]
    pub fn new(registry: Arc<SandboxedToolRegistry>) -> Self {
        Self { registry }
    }

    /// Creates a [`SqliteReceiptSink`] attached to an existing `SqliteStore` file for `task_id`.
    pub fn sqlite_sink(store_path: &Path, task_id: &str) -> Result<Arc<SqliteReceiptSink>, String> {
        let store = SqliteStore::open_existing(store_path)
            .map_err(|e| format!("open sqlite store for mcp sink: {e:?}"))?;
        let sink = SqliteReceiptSink::new(store, task_id)?;
        Ok(Arc::new(sink))
    }

    /// Executes an allowlisted MCP tool (`LaneAWasm` or `LaneBNative`) strictly behind an
    /// `AuthorizedEffect` over an in-memory `rmcp 3.11` duplex session, persisting its
    /// `McpResourceReceipt` through the registry's `ReceiptSink`.
    pub fn dispatch_authorized(
        &self,
        effect: &AuthorizedEffect,
        tool_name: &str,
        args: Map<String, Value>,
    ) -> Result<McpCallOutcome, String> {
        // Bind authority check: only an admitted `AuthorizedEffect` may invoke tools.
        let _dispatch_id = &effect.request().action.dispatch_id;
        let server = MetronMcpServer::new(Arc::clone(&self.registry));
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("tokio runtime init failed: {e}"))?;

        rt.block_on(async move {
            server
                .call_via_duplex(tool_name, args)
                .await
                .map_err(|e| format!("mcp tool call failed: {e}"))
        })
    }

    /// Directly executes a sealed `LaneAWasm` request behind `AuthorizedEffect` and persists
    /// its deterministic `ResourceReceipt` into `metron-store-sqlite`.
    pub fn dispatch_lane_a_wasm(
        effect: &AuthorizedEffect,
        tool_name: &str,
        req: &LaneARequest<'_>,
        store_path: &Path,
    ) -> Result<ResourceReceipt, String> {
        let task_id = effect.request().action.task_id.as_str();
        let receipt = run_lane_a_jail(req).map_err(|e| format!("lane_a execution failed: {e}"))?;
        let store = SqliteStore::open_existing(store_path)
            .map_err(|e| format!("open sqlite store: {e:?}"))?;
        store
            .ensure_external_receipts_table()
            .map_err(|e| format!("ensure external_receipts: {e:?}"))?;
        let json =
            serde_json::to_string(&receipt).map_err(|e| format!("encode lane_a receipt: {e}"))?;
        let digest = content_digest(json.as_bytes());
        store
            .record_external_receipt("lane_a_wasm", task_id, tool_name, &json, digest.as_str())
            .map_err(|e| format!("persist lane_a receipt: {e:?}"))?;
        Ok(receipt)
    }
}
