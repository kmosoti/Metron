//! ADR-0004 `ResourceReceipt` and token usage accounting for HTTP model providers.

use crate::{
    port::{
        EndpointConfig, ProviderError, ProviderKind, ProviderOutcome, ProviderRequest, ToolCall,
    },
    pricing::estimate_spend_micros_usd,
};
use metron_protocol::{
    ActualUsage, BoundedText, Receipt, ReceiptData, ReceiptOutput, SchemaVersion, SqlU64,
    UsageVector, canonical_bytes, content_digest, structural_digest,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// Token usage counters reported by an inference HTTP response.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderUsage {
    /// Prompt / input tokens billed.
    pub input_tokens: u64,
    /// Completion / output tokens billed.
    pub output_tokens: u64,
    /// Total tokens billed.
    pub total_tokens: u64,
    /// Micro-USD spend estimate computed from the model pricing table (integer micros).
    pub spend_micros_usd: u64,
}

/// External operation receipt emitted for every HTTP model provider invocation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResourceReceipt {
    /// Dialect used (`open_ai_compatible` or `anthropic_compatible`).
    pub provider: ProviderKind,
    /// Target model identifier.
    pub model: String,
    /// Target endpoint URL.
    pub endpoint: String,
    /// HTTP status code received (`None` on transport failure).
    pub http_status: Option<u16>,
    /// SHA-256 hex digest of canonical serialized request body.
    pub request_sha256: String,
    /// SHA-256 hex digest of raw HTTP response body (or empty bytes on transport error).
    pub response_sha256: String,
    /// Token & spend accounting.
    pub usage: ProviderUsage,
    /// Validated `metron-protocol` ledger receipt when constructible.
    pub protocol_receipt: Option<Receipt>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn assemble_receipt(
    config: &EndpointConfig,
    endpoint: String,
    req: &ProviderRequest,
    status: Option<u16>,
    request_sha256: &str,
    raw: &[u8],
    usage: ProviderUsage,
    ok: bool,
    diagnostic: &str,
) -> ResourceReceipt {
    let protocol_receipt = build_protocol_receipt(req, raw, usage, ok, diagnostic);
    ResourceReceipt {
        provider: config.kind,
        model: config.model.clone(),
        endpoint,
        http_status: status,
        request_sha256: request_sha256.to_owned(),
        response_sha256: hex_sha256(raw),
        usage,
        protocol_receipt,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn finalize_outcome(
    config: &EndpointConfig,
    endpoint: String,
    req: &ProviderRequest,
    status: u16,
    request_sha256: &str,
    raw: &[u8],
    text: String,
    tool_calls: Vec<ToolCall>,
    finish_reason: String,
    mut usage: ProviderUsage,
) -> ProviderOutcome {
    usage.spend_micros_usd = estimate_spend_micros_usd(
        &config.model,
        config.pricing_override,
        usage.input_tokens,
        usage.output_tokens,
    );
    let receipt = assemble_receipt(
        config,
        endpoint,
        req,
        Some(status),
        request_sha256,
        raw,
        usage,
        true,
        "",
    );
    ProviderOutcome {
        text,
        tool_calls,
        finish_reason,
        usage,
        receipt,
    }
}

pub(crate) fn redact_secret(msg: &str, secret: Option<&str>) -> String {
    match secret {
        Some(s) if !s.is_empty() => msg.replace(s, "<redacted>"),
        _ => msg.to_owned(),
    }
}

pub(crate) fn build_transport_error(
    config: &EndpointConfig,
    endpoint: String,
    req: &ProviderRequest,
    request_sha256: &str,
    raw_message: String,
) -> ProviderError {
    let message = redact_secret(&raw_message, config.api_key.as_deref());
    let receipt = assemble_receipt(
        config,
        endpoint,
        req,
        None,
        request_sha256,
        b"",
        ProviderUsage::default(),
        false,
        &message,
    );
    ProviderError::Transport {
        message,
        receipt: Box::new(receipt),
    }
}

pub(crate) fn build_upstream_error(
    config: &EndpointConfig,
    endpoint: String,
    req: &ProviderRequest,
    status: Option<u16>,
    request_sha256: &str,
    raw: &[u8],
    raw_message: String,
) -> ProviderError {
    let message = redact_secret(&raw_message, config.api_key.as_deref());
    let receipt = assemble_receipt(
        config,
        endpoint,
        req,
        status,
        request_sha256,
        raw,
        ProviderUsage::default(),
        false,
        &message,
    );
    ProviderError::Upstream {
        status,
        message,
        receipt: Box::new(receipt),
    }
}

pub(crate) fn hex_sha256(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    Sha256::digest(bytes)
        .iter()
        .flat_map(|&b| [H[(b >> 4) as usize] as char, H[(b & 0x0f) as usize] as char])
        .collect()
}

pub(crate) fn build_protocol_receipt(
    req: &ProviderRequest,
    response_bytes: &[u8],
    usage: ProviderUsage,
    ok: bool,
    diagnostic: &str,
) -> Option<Receipt> {
    let action_digest = structural_digest(
        "model-provider-http/v1",
        &[
            req.task_id.as_str().as_bytes(),
            req.intent_id.as_str().as_bytes(),
            req.dispatch_id.as_str().as_bytes(),
        ],
    )
    .ok()?;
    let output = if ok {
        let truncated = String::from_utf8_lossy(response_bytes);
        let bounded: String = truncated.chars().take(128).collect();
        let text = BoundedText::new(if bounded.is_empty() {
            "empty".to_owned()
        } else {
            bounded
        })
        .ok()?;
        ReceiptOutput::Fixture {
            bytes: text,
            digest: content_digest(response_bytes),
        }
    } else {
        let diag: String = diagnostic.chars().take(256).collect();
        ReceiptOutput::Failure {
            diagnostic: BoundedText::new(if diag.is_empty() {
                "provider call failed".to_owned()
            } else {
                diag
            })
            .ok()?,
        }
    };
    let actual_usage = if ok {
        ActualUsage::Known {
            usage: UsageVector {
                model_requests: metron_protocol::DecimalUsage::new("1").ok()?,
                tool_calls: metron_protocol::DecimalUsage::new("0").ok()?,
                spend_micros_usd: metron_protocol::DecimalUsage::new(
                    usage.spend_micros_usd.to_string(),
                )
                .ok()?,
            },
        }
    } else {
        ActualUsage::Unknown {
            reason: BoundedText::new("http provider failure").ok()?,
            evidence: None,
        }
    };
    let receipt = Receipt::new(ReceiptData {
        schema_version: SchemaVersion::V1,
        producer_receipt_id: req.producer_receipt_id.clone(),
        task_id: req.task_id.clone(),
        intent_id: req.intent_id.clone(),
        dispatch_id: req.dispatch_id.clone(),
        action_digest,
        usage: actual_usage,
        output,
    })
    .ok()?;
    let _ = canonical_bytes(&receipt, metron_protocol::MAX_ENVELOPE).ok()?;
    let _ = SqlU64::new(0).ok()?;
    Some(receipt)
}
