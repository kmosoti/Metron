//! Durable persistence for external `ResourceReceipt` records (`metron-mcp` tools
//! and `metron-provider-http` model completions) inside `SqliteStore`.

use crate::{SqliteStore, operational};
use metron_protocol::store::StoreError;
use rusqlite::params;

/// One persisted external-operation `ResourceReceipt` row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalReceiptRecord {
    /// Monotonic SQLite row identifier.
    pub id: i64,
    /// Receipt category (e.g., `"mcp_tool"`, `"http_model"`, `"lane_a_wasm"`).
    pub kind: String,
    /// Associated Metron task identifier.
    pub task_id: String,
    /// Tool name or model identifier.
    pub tool_or_model: String,
    /// Canonical JSON payload of the `ResourceReceipt`.
    pub payload_json: String,
    /// Hex SHA-256 digest of `payload_json`.
    pub digest_sha256: String,
}

impl SqliteStore {
    /// Ensures the `external_receipts` table and index exist on writable connections.
    pub fn ensure_external_receipts_table(&self) -> Result<(), StoreError> {
        if self.read_only {
            return Err(StoreError::Integrity);
        }
        self.connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS external_receipts (\
                 id INTEGER PRIMARY KEY AUTOINCREMENT,\
                 kind TEXT NOT NULL,\
                 task_id TEXT NOT NULL,\
                 tool_or_model TEXT NOT NULL,\
                 payload_json TEXT NOT NULL,\
                 digest_sha256 TEXT NOT NULL);\
                 CREATE INDEX IF NOT EXISTS idx_external_receipts_task \
                 ON external_receipts(task_id, id);",
            )
            .map_err(operational)
    }

    /// Persists one external `ResourceReceipt` row durably into SQLite.
    pub fn record_external_receipt(
        &self,
        kind: &str,
        task_id: &str,
        tool_or_model: &str,
        payload_json: &str,
        digest_sha256: &str,
    ) -> Result<i64, StoreError> {
        if self.read_only {
            return Err(StoreError::Integrity);
        }
        self.connection
            .execute(
                "INSERT INTO external_receipts(kind, task_id, tool_or_model, payload_json, digest_sha256) \
                 VALUES(?1, ?2, ?3, ?4, ?5)",
                params![kind, task_id, tool_or_model, payload_json, digest_sha256],
            )
            .map_err(operational)?;
        Ok(self.connection.last_insert_rowid())
    }

    /// Lists persisted external `ResourceReceipt` rows in insertion order, optionally filtered by `task_id`.
    pub fn list_external_receipts(
        &self,
        task_id: Option<&str>,
    ) -> Result<Vec<ExternalReceiptRecord>, StoreError> {
        let map_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<ExternalReceiptRecord> {
            Ok(ExternalReceiptRecord {
                id: row.get(0)?,
                kind: row.get(1)?,
                task_id: row.get(2)?,
                tool_or_model: row.get(3)?,
                payload_json: row.get(4)?,
                digest_sha256: row.get(5)?,
            })
        };
        let mut out = Vec::new();
        if let Some(tid) = task_id {
            let mut stmt = self
                .connection
                .prepare(
                    "SELECT id, kind, task_id, tool_or_model, payload_json, digest_sha256 \
                     FROM external_receipts WHERE task_id = ?1 ORDER BY id",
                )
                .map_err(operational)?;
            let rows = stmt.query_map([tid], map_row).map_err(operational)?;
            for row in rows {
                out.push(row.map_err(operational)?);
            }
        } else {
            let mut stmt = self
                .connection
                .prepare(
                    "SELECT id, kind, task_id, tool_or_model, payload_json, digest_sha256 \
                     FROM external_receipts ORDER BY id",
                )
                .map_err(operational)?;
            let rows = stmt.query_map([], map_row).map_err(operational)?;
            for row in rows {
                out.push(row.map_err(operational)?);
            }
        }
        Ok(out)
    }
}
