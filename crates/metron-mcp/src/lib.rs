//! Typed `rmcp 3.11` MCP client/server bridge enforcing `metron-sandbox` isolation,
//! JSON Schema validation, cancellation propagation, bounded receipt retention,
//! and durable `ReceiptSink` persistence into `metron-store-sqlite`.

pub mod bridge;
mod executor;
pub mod receipt;
pub mod registry;
pub mod schema;

pub use bridge::{McpCallOutcome, MetronMcpClient, MetronMcpServer};
pub use receipt::{
    BoundedReceiptRing, DEFAULT_RECEIPT_RING_CAPACITY, McpLaneKind, McpResourceReceipt,
    ReceiptSink, SqliteReceiptSink,
};
pub use registry::{McpBridgeError, SandboxedToolRegistry, SandboxedToolSpec};
pub use schema::validate_args_against_schema;
