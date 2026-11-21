//! Pluggable `ModelProvider` port and bounded OpenAI / Anthropic HTTP adapters.

pub mod anthropic;
pub mod client;
pub mod openai;
pub mod port;
pub mod pricing;
pub mod receipt;
pub mod resilience;
pub mod sse;

pub use client::HttpModelProvider;
pub use port::{
    CacheControl, EndpointConfig, MessageRole, ModelProvider, ProviderError, ProviderKind,
    ProviderMessage, ProviderOutcome, ProviderRequest, ProviderUsage, ResourceReceipt,
    ResponseFormat, StreamDelta, ToolCall, ToolDefinition,
};
pub use pricing::{ModelPricing, estimate_spend_micros_usd, lookup_model_pricing};
pub use resilience::{CircuitBreaker, CircuitBreakerConfig, RetryPolicy};
