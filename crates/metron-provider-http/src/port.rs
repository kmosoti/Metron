//! Pluggable `ModelProvider` port trait, tool-calling / structured-output / caching types,
//! endpoint config with secret redaction, and ADR-0004 `ResourceReceipt` types.

pub use crate::receipt::{ProviderUsage, ResourceReceipt};
use crate::{
    pricing::ModelPricing,
    resilience::{CircuitBreakerConfig, RetryPolicy},
};
use metron_protocol::{Digest, ProducerReceiptId, TaskId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fmt, future::Future, pin::Pin};
use thiserror::Error;

/// Supported external HTTP inference API dialects.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// OpenAI-compatible Chat Completions endpoint (`/v1/chat/completions`).
    OpenAiCompatible,
    /// Anthropic-compatible Messages endpoint (`/v1/messages`).
    AnthropicCompatible,
}

/// Sanitized endpoint configuration for an HTTP inference provider.
#[derive(Clone, Eq, PartialEq)]
pub struct EndpointConfig {
    /// API dialect.
    pub kind: ProviderKind,
    /// Base URL without trailing endpoint suffix (e.g. `https://api.openai.com`).
    pub base_url: String,
    /// Model identifier sent in the request JSON.
    pub model: String,
    /// Optional secret bearer / `x-api-key` credential (never printed in `Debug`).
    pub api_key: Option<String>,
    /// Request wall-clock timeout in milliseconds.
    pub timeout_ms: u64,
    /// Hard maximum HTTP response payload bytes read from the server.
    pub max_response_bytes: usize,
    /// Optional explicit token pricing override (defaults to built-in model rate table).
    pub pricing_override: Option<ModelPricing>,
    /// Retry / exponential backoff policy for HTTP `429`/`5xx`.
    pub retry: RetryPolicy,
    /// Circuit breaker thresholds for repeated transient upstream failures.
    pub circuit_breaker: CircuitBreakerConfig,
}

impl fmt::Debug for EndpointConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EndpointConfig")
            .field("kind", &self.kind)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("timeout_ms", &self.timeout_ms)
            .field("max_response_bytes", &self.max_response_bytes)
            .field("pricing_override", &self.pricing_override)
            .field("retry", &self.retry)
            .field("circuit_breaker", &self.circuit_breaker)
            .finish()
    }
}

/// Prompt prefix cache-control marker (for Anthropic prompt caching and OpenAI cache hints).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheControl {
    /// Marks a prompt prefix block eligible for ephemeral upstream KV cache reuse.
    Ephemeral,
}

/// Role of a chat message sent to a model endpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    /// System prompt instructions.
    System,
    /// User / harness prompt turn.
    User,
    /// Prior assistant turn (may carry `tool_calls`).
    Assistant,
    /// Tool execution result turn correlated via `tool_call_id`.
    Tool,
}

/// Model-authored tool invocation emitted in a response or echoed in assistant history.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Provider-assigned tool call correlation ID.
    pub id: String,
    /// Tool name to dispatch.
    pub name: String,
    /// Parsed JSON arguments object.
    pub arguments: Value,
}

/// Typed tool schema advertised to the model endpoint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Unique tool name.
    pub name: String,
    /// Concise tool description.
    pub description: String,
    /// JSON Schema describing the tool's input object.
    pub input_schema: Value,
}

/// Structured output format constraint requested from the model endpoint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseFormat {
    /// Free-form text response.
    Text,
    /// Valid JSON object response.
    JsonObject,
    /// JSON response constrained to an explicit JSON Schema.
    JsonSchema {
        /// Schema name identifier.
        name: String,
        /// JSON Schema object.
        schema: Value,
        /// Whether upstream strict grammar decoding should be requested.
        strict: bool,
    },
}

/// One typed prompt message.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderMessage {
    /// Speaker role.
    pub role: MessageRole,
    /// Message text body.
    pub content: String,
    /// Tool call ID when `role == MessageRole::Tool`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Assistant tool invocations when `role == MessageRole::Assistant`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Optional prompt-prefix cache-control marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

impl ProviderMessage {
    /// Builds a simple message with no tool metadata or cache marker.
    #[must_use]
    pub fn text(role: MessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            cache_control: None,
        }
    }

    /// Builds a `MessageRole::Tool` result message bound to `tool_call_id`.
    #[must_use]
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Tool,
            content: content.into(),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: Vec::new(),
            cache_control: None,
        }
    }
}

/// Bounded inference request submitted through `ModelProvider`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderRequest {
    /// Metron task identifier for receipt binding.
    pub task_id: TaskId,
    /// Metron intent digest for receipt binding.
    pub intent_id: Digest,
    /// Metron dispatch digest for receipt binding.
    pub dispatch_id: Digest,
    /// Producer receipt identifier.
    pub producer_receipt_id: ProducerReceiptId,
    /// Conversation messages.
    pub messages: Vec<ProviderMessage>,
    /// Maximum output completion tokens requested.
    pub max_tokens: u32,
    /// Optional temperature expressed in integer thousandths (`0..=2000`).
    pub temperature_milli: Option<u16>,
    /// Optional tool schemas advertised to the model.
    pub tools: Vec<ToolDefinition>,
    /// Optional structured output constraint (`response_format` JSON schema).
    pub response_format: Option<ResponseFormat>,
}

/// Successful completion result plus immutable `ResourceReceipt`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderOutcome {
    /// Extracted completion text.
    pub text: String,
    /// Parsed model tool invocations (empty if none requested).
    pub tool_calls: Vec<ToolCall>,
    /// Upstream stop / finish reason.
    pub finish_reason: String,
    /// Measured token & cost usage.
    pub usage: ProviderUsage,
    /// Mandatory external-operation receipt.
    pub receipt: ResourceReceipt,
}

/// Incremental SSE event emitted during [`ModelProvider::complete_stream`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StreamDelta {
    /// Incremental text content chunk.
    TextDelta(String),
    /// Incremental tool call chunk.
    ToolCallDelta {
        /// Tool call index in the response.
        index: usize,
        /// Optional tool call ID (typically present on the first chunk).
        id: Option<String>,
        /// Optional tool name (typically present on the first chunk).
        name: Option<String>,
        /// Partial JSON arguments fragment.
        arguments_delta: String,
    },
    /// Final assembled completion outcome and `ResourceReceipt`.
    Completed(Box<ProviderOutcome>),
}

/// Redacted failure type returned by `ModelProvider` adapters.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum ProviderError {
    /// Endpoint configuration or request payload rejected before sending.
    #[error("invalid provider request: {0}")]
    InvalidConfig(String),
    /// Network transport, timeout, circuit-breaker, or TLS failure (secrets redacted + failure receipt preserved).
    #[error("provider transport error: {message}")]
    Transport {
        /// Sanitized diagnostic string (secrets redacted).
        message: String,
        /// Failure `ResourceReceipt` preserved for auditability.
        receipt: Box<ResourceReceipt>,
    },
    /// Upstream HTTP non-success status or malformed response body.
    #[error("provider HTTP error (status={status:?}): {message}")]
    Upstream {
        /// HTTP status code if a response arrived.
        status: Option<u16>,
        /// Sanitized diagnostic string.
        message: String,
        /// Failure `ResourceReceipt` recorded for auditability.
        receipt: Box<ResourceReceipt>,
    },
}

/// Asynchronous hexagonal port for external or local model inference adapters.
pub trait ModelProvider: Send + Sync {
    /// Executes one bounded completion call and always produces a `ResourceReceipt`.
    fn complete<'a>(
        &'a self,
        req: &'a ProviderRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderOutcome, ProviderError>> + Send + 'a>>;

    /// Executes one bounded SSE streaming completion call, emitting [`StreamDelta`]s
    /// incrementally and returning the final assembled [`ProviderOutcome`].
    fn complete_stream<'a>(
        &'a self,
        req: &'a ProviderRequest,
        on_delta: &'a mut (dyn FnMut(StreamDelta) + Send),
    ) -> Pin<Box<dyn Future<Output = Result<ProviderOutcome, ProviderError>> + Send + 'a>>;
}
