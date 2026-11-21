//! `rmcp 3.11` typed server and client bridge delegating tool execution to `metron-sandbox`
//! with pre-sandbox schema validation and live `RequestContext` cancellation propagation.

use crate::{
    receipt::McpResourceReceipt,
    registry::{McpBridgeError, SandboxedToolRegistry},
    schema::validate_args_against_schema,
};
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    model::{
        CallToolRequestMethod, CallToolRequestParams, CallToolResponse, CallToolResult, JsonObject,
        ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    },
    service::{RequestContext, RoleClient, RoleServer, RunningService},
    transport::IntoTransport,
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// `rmcp 3.11` `ServerHandler` routing all `tools/call` requests through `metron-sandbox`.
#[derive(Clone, Debug)]
pub struct MetronMcpServer {
    registry: Arc<SandboxedToolRegistry>,
    server_name: String,
    server_version: String,
}

impl MetronMcpServer {
    /// Creates an MCP server backed by an allowlisted `SandboxedToolRegistry`.
    #[must_use]
    pub fn new(registry: Arc<SandboxedToolRegistry>) -> Self {
        Self {
            registry,
            server_name: "metron-mcp".to_owned(),
            server_version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    /// Returns bounded recent `McpResourceReceipt`s recorded by this server so far.
    #[must_use]
    pub fn receipts(&self) -> Vec<McpResourceReceipt> {
        self.registry.receipts()
    }

    /// Executes one tool call over an in-memory `rmcp 3.11` duplex session (enforcing full
    /// MCP handshake, JSON Schema validation, sandbox isolation, and `ReceiptSink` persistence).
    pub async fn call_via_duplex(
        &self,
        tool_name: &str,
        arguments: JsonObject,
    ) -> Result<McpCallOutcome, McpBridgeError> {
        let server = self.clone();
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let server_handle = tokio::spawn(async move {
            if let Ok(running) = server.serve(server_io).await {
                let _ = running.waiting().await;
            }
        });
        let client = MetronMcpClient::connect(client_io).await?;
        let outcome = client.call_sandboxed_tool(tool_name, arguments).await;
        let _ = client.close().await;
        let _ = server_handle.await;
        outcome
    }
}

impl ServerHandler for MetronMcpServer {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::new(ServerCapabilities::builder().enable_tools().build());
        info.server_info.name = self.server_name.clone();
        info.server_info.version = self.server_version.clone();
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(self.registry.list_tools()))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.registry.get_tool(name)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let Some(spec) = self.registry.lookup(&request.name) else {
            return Err(McpError::method_not_found::<CallToolRequestMethod>());
        };
        let args = request.arguments.unwrap_or_default();
        if let Err(err) = validate_args_against_schema(spec.input_schema(), &args) {
            return Err(McpError::invalid_params(err.to_string(), None));
        }

        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_worker = Arc::clone(&cancel);
        let registry = Arc::clone(&self.registry);

        let mut handle = tokio::task::spawn_blocking(move || {
            registry.execute_spec_cancellable(&spec, &args, Some(&cancel_worker))
        });

        let outcome = tokio::select! {
            res = &mut handle => {
                res.map_err(|e| McpError::internal_error(format!("sandbox join failed: {e}"), None))?
            }
            () = context.ct.cancelled() => {
                cancel.store(true, Ordering::Release);
                let _ = handle.await;
                return Err(McpError::internal_error("tool execution cancelled", None));
            }
        };

        match outcome {
            Ok((output, receipt)) => {
                let is_ok = receipt.exit_code == 0 && receipt.trapped.is_none();
                let payload = json!({
                    "output": output,
                    "receipt": receipt,
                });
                let result = if is_ok {
                    CallToolResult::structured(payload)
                } else {
                    CallToolResult::structured_error(payload)
                };
                Ok(CallToolResponse::Complete(result))
            }
            Err(McpBridgeError::InvalidArguments(msg)) => Err(McpError::invalid_params(msg, None)),
            Err(McpBridgeError::Cancelled) => {
                Err(McpError::internal_error("tool execution cancelled", None))
            }
            Err(err) => Ok(CallToolResponse::Complete(
                CallToolResult::structured_error(json!({
                    "error": err.to_string(),
                })),
            )),
        }
    }
}

/// Typed `rmcp 3.11` client outcome extracting both tool output and `McpResourceReceipt`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpCallOutcome {
    /// Structured JSON output payload emitted by the sandboxed tool.
    pub output: Value,
    /// Mandatory external-operation receipt returned by the server.
    pub receipt: McpResourceReceipt,
    /// Whether the tool marked `is_error`.
    pub is_error: bool,
}

/// Typed `rmcp 3.11` MCP client wrapper for invoking sandboxed tools.
pub struct MetronMcpClient {
    service: RunningService<RoleClient, ()>,
}

impl MetronMcpClient {
    /// Connects to an MCP server over any `rmcp` transport (duplex, stdio, or HTTP).
    pub async fn connect<T, E, A>(transport: T) -> Result<Self, McpBridgeError>
    where
        T: IntoTransport<RoleClient, E, A>,
        E: std::error::Error + Send + Sync + 'static,
    {
        let service = ()
            .serve(transport)
            .await
            .map_err(|e| McpBridgeError::Protocol(format!("MCP initialize failed: {e}")))?;
        Ok(Self { service })
    }

    /// Queries all available tools advertised by the connected MCP server.
    pub async fn list_tools(&self) -> Result<Vec<Tool>, McpBridgeError> {
        let res = self
            .service
            .list_tools(None)
            .await
            .map_err(|e| McpBridgeError::Protocol(e.to_string()))?;
        Ok(res.tools)
    }

    /// Invokes a sandboxed tool and verifies that a typed `McpResourceReceipt` was returned.
    pub async fn call_sandboxed_tool(
        &self,
        name: &str,
        arguments: JsonObject,
    ) -> Result<McpCallOutcome, McpBridgeError> {
        let params = CallToolRequestParams::new(name.to_owned()).with_arguments(arguments);
        let result = self
            .service
            .call_tool(params)
            .await
            .map_err(|e| McpBridgeError::Protocol(e.to_string()))?;
        let is_error = result.is_error.unwrap_or(false);
        let structured = result
            .structured_content
            .ok_or_else(|| McpBridgeError::Protocol("missing structured tool payload".into()))?;
        let Some(receipt_val) = structured.get("receipt").cloned() else {
            if let Some(err_msg) = structured.get("error").and_then(Value::as_str) {
                return Err(McpBridgeError::Sandbox(err_msg.to_owned()));
            }
            return Err(McpBridgeError::Protocol(
                "missing ResourceReceipt in response".into(),
            ));
        };
        let receipt: McpResourceReceipt = serde_json::from_value(receipt_val)
            .map_err(|e| McpBridgeError::Protocol(format!("invalid receipt JSON: {e}")))?;
        let output = structured.get("output").cloned().unwrap_or(Value::Null);
        Ok(McpCallOutcome {
            output,
            receipt,
            is_error,
        })
    }

    /// Cleanly cancels and shuts down the MCP client session.
    pub async fn close(self) -> Result<(), McpBridgeError> {
        self.service
            .cancel()
            .await
            .map_err(|e| McpBridgeError::Protocol(format!("MCP shutdown failed: {e}")))?;
        Ok(())
    }
}
