//! Pluggable `ModelBackend` (`Native` / `Http` / `Script`) wiring `HttpModelProvider`
//! into `metron-host` behind `AuthorizedEffect` with durable SQLite `ResourceReceipt` persistence.

use metron_harness::{driver::DriverOutcome, policy::AuthorizedEffect};
use metron_inference_local::driver::LocalInferenceDriver;
use metron_protocol::{
    ActualUsage, BoundedText, CodingObservation, DecimalUsage, Digest, ProducerReceiptId, Receipt,
    ReceiptData, ReceiptOutput, UsageVector, content_digest,
};
use metron_provider_http::{
    CircuitBreakerConfig, EndpointConfig, HttpModelProvider, MessageRole, ModelProvider,
    ProviderError, ProviderKind, ProviderMessage, ProviderRequest, ResourceReceipt, RetryPolicy,
};
use metron_store_sqlite::SqliteStore;
use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, path::PathBuf};

/// Serializable HTTP `ModelProvider` settings (API key loaded from env var, never persisted).
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HttpProviderSettings {
    pub kind: ProviderKind,
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default = "default_http_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_http_max_response_bytes")]
    pub max_response_bytes: usize,
    #[serde(default = "default_http_max_tokens")]
    pub max_tokens: u32,
}

fn default_http_timeout_ms() -> u64 {
    30_000
}
fn default_http_max_response_bytes() -> usize {
    256 * 1024
}
fn default_http_max_tokens() -> u32 {
    2048
}

const MAX_HTTP_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

impl HttpProviderSettings {
    pub(crate) fn build_backend(&self, store_path: PathBuf) -> Result<HttpModelBackend, String> {
        if self.max_response_bytes == 0 || self.max_response_bytes > MAX_HTTP_RESPONSE_BYTES {
            return Err(format!(
                "http provider config: max_response_bytes must be in 1..={MAX_HTTP_RESPONSE_BYTES}"
            ));
        }
        if let Some(env_name) = &self.api_key_env
            && (env_name.is_empty()
                || env_name.contains('=')
                || !env_name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        {
            return Err("http provider config: invalid api_key_env name".to_owned());
        }
        let api_key = self
            .api_key_env
            .as_deref()
            .map(std::env::var)
            .transpose()
            .map_err(|_| "HTTP provider API key env var missing".to_owned())?;
        let provider = HttpModelProvider::new(EndpointConfig {
            kind: self.kind,
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            api_key,
            timeout_ms: self.timeout_ms,
            max_response_bytes: self.max_response_bytes,
            pricing_override: None,
            retry: RetryPolicy::default(),
            circuit_breaker: CircuitBreakerConfig::default(),
        })
        .map_err(|e| format!("http provider config: {e}"))?;
        Ok(HttpModelBackend::new(
            provider,
            store_path,
            "You are a bounded Metron coding agent.",
            self.max_tokens,
        ))
    }
}

/// Pluggable model execution backend owned by `coding_run::Driver`.
pub(crate) enum ModelBackend {
    /// Local supervised inference worker (`metron-inference-local`).
    Native(Box<LocalInferenceDriver>),
    /// Pluggable HTTP OpenAI/Anthropic `ModelProvider` (`metron-provider-http`).
    Http(Box<HttpModelBackend>),
    /// Controlled deterministic script replies for replay/testing.
    Script(VecDeque<String>),
}

impl ModelBackend {
    /// Cleanly shuts down any underlying supervised worker process.
    pub(crate) fn shutdown(&mut self) {
        if let Self::Native(native) = self {
            native.shutdown();
        }
    }

    #[cfg(test)]
    pub(crate) fn script_len(&self) -> usize {
        match self {
            Self::Script(q) => q.len(),
            Self::Native(_) | Self::Http(_) => 0,
        }
    }
}

/// HTTP `ModelProvider` adapter bound to a durable `SqliteStore` path for external receipts.
pub(crate) struct HttpModelBackend {
    provider: HttpModelProvider,
    store_path: PathBuf,
    system_prompt: String,
    max_tokens: u32,
}

impl HttpModelBackend {
    /// Creates an HTTP model backend persisting every `ResourceReceipt` into `store_path`.
    pub(crate) fn new(
        provider: HttpModelProvider,
        store_path: PathBuf,
        system_prompt: impl Into<String>,
        max_tokens: u32,
    ) -> Self {
        Self {
            provider,
            store_path,
            system_prompt: system_prompt.into(),
            max_tokens: max_tokens.max(16),
        }
    }

    /// Executes one `AuthorizedEffect` coding model step via `HttpModelProvider`,
    /// persists its `ResourceReceipt` into `metron-store-sqlite`, and returns a `DriverOutcome`.
    pub(crate) fn execute(
        &mut self,
        effect: &AuthorizedEffect,
        input_digest: &Digest,
        prompt: &str,
    ) -> DriverOutcome {
        let prepared = effect.request();
        let Ok(producer_receipt_id) = ProducerReceiptId::new(prepared.action.dispatch_id.as_str())
        else {
            return DriverOutcome::Uncertain;
        };
        let req = ProviderRequest {
            task_id: prepared.action.task_id.clone(),
            intent_id: prepared.intent_id.clone(),
            dispatch_id: prepared.action.dispatch_id.clone(),
            producer_receipt_id,
            messages: vec![
                ProviderMessage::text(MessageRole::System, &self.system_prompt),
                ProviderMessage::text(MessageRole::User, prompt),
            ],
            max_tokens: self.max_tokens,
            temperature_milli: Some(0),
            tools: Vec::new(),
            response_format: None,
        };

        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return DriverOutcome::Uncertain;
        };

        match rt.block_on(self.provider.complete(&req)) {
            Ok(outcome) => {
                if self
                    .persist_receipt(prepared.action.task_id.as_str(), &outcome.receipt)
                    .is_err()
                {
                    return DriverOutcome::Uncertain;
                }
                let Ok(content) = BoundedText::new(outcome.text) else {
                    return DriverOutcome::Uncertain;
                };
                let Ok(spend) = DecimalUsage::new(outcome.usage.spend_micros_usd.to_string())
                else {
                    return DriverOutcome::Uncertain;
                };
                let receipt = Receipt::new(ReceiptData {
                    schema_version: effect.schema_version(),
                    producer_receipt_id: ProducerReceiptId::new(
                        prepared.action.dispatch_id.as_str(),
                    )
                    .expect("digest is bounded identifier"),
                    task_id: prepared.action.task_id.clone(),
                    intent_id: prepared.intent_id.clone(),
                    dispatch_id: prepared.action.dispatch_id.clone(),
                    action_digest: prepared.action_digest.clone(),
                    usage: ActualUsage::Known {
                        usage: UsageVector {
                            model_requests: DecimalUsage::new("1").expect("one"),
                            tool_calls: DecimalUsage::new("0").expect("zero"),
                            spend_micros_usd: spend,
                        },
                    },
                    output: ReceiptOutput::Coding {
                        observation: Box::new(CodingObservation::ScriptedModel {
                            input_digest: input_digest.clone(),
                            content,
                        }),
                    },
                });
                match receipt {
                    Ok(r) => DriverOutcome::Receipt(Box::new(r)),
                    Err(_) => DriverOutcome::Uncertain,
                }
            }
            Err(ProviderError::Upstream {
                message, receipt, ..
            })
            | Err(ProviderError::Transport { message, receipt }) => {
                let _ = self.persist_receipt(prepared.action.task_id.as_str(), &receipt);
                let diag: String = message.chars().take(200).collect();
                let Ok(bounded) = BoundedText::new(if diag.is_empty() {
                    "http model provider failed".to_owned()
                } else {
                    diag
                }) else {
                    return DriverOutcome::Uncertain;
                };
                let Ok(r) = Receipt::new(ReceiptData {
                    schema_version: effect.schema_version(),
                    producer_receipt_id: ProducerReceiptId::new(
                        prepared.action.dispatch_id.as_str(),
                    )
                    .expect("digest is bounded identifier"),
                    task_id: prepared.action.task_id.clone(),
                    intent_id: prepared.intent_id.clone(),
                    dispatch_id: prepared.action.dispatch_id.clone(),
                    action_digest: prepared.action_digest.clone(),
                    usage: ActualUsage::Unknown {
                        reason: BoundedText::new("http model provider error").expect("bounded"),
                        evidence: None,
                    },
                    output: ReceiptOutput::Failure {
                        diagnostic: bounded,
                    },
                }) else {
                    return DriverOutcome::Uncertain;
                };
                DriverOutcome::Receipt(Box::new(r))
            }
            Err(ProviderError::InvalidConfig(_)) => DriverOutcome::Uncertain,
        }
    }

    fn persist_receipt(&self, task_id: &str, receipt: &ResourceReceipt) -> Result<(), String> {
        let store = SqliteStore::open_existing(&self.store_path)
            .map_err(|e| format!("open sqlite store: {e:?}"))?;
        store
            .ensure_external_receipts_table()
            .map_err(|e| format!("ensure external_receipts: {e:?}"))?;
        let json =
            serde_json::to_string(receipt).map_err(|e| format!("encode http receipt: {e}"))?;
        let digest = content_digest(json.as_bytes());
        store
            .record_external_receipt(
                "http_model",
                task_id,
                &receipt.model,
                &json,
                digest.as_str(),
            )
            .map(|_| ())
            .map_err(|e| format!("persist http receipt: {e:?}"))
    }
}
