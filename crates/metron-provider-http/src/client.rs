//! Bounded `reqwest` adapter implementing `ModelProvider` with redirect disablement,
//! payload limits, pricing table, SSE streaming (`complete_stream`), exponential backoff
//! with `Retry-After`, circuit breaker on `429`/`5xx`, secret scrubbing, and `ResourceReceipt`
//! preservation on both success and failure (`Transport` and `Upstream`).

use crate::{
    anthropic::{self, ANTHROPIC_API_VERSION},
    body::{BodyReadError, read_bounded_body},
    openai,
    port::{
        EndpointConfig, ModelProvider, ProviderError, ProviderKind, ProviderOutcome,
        ProviderRequest, StreamDelta,
    },
    receipt::{
        build_transport_error, build_upstream_error, finalize_outcome, hex_sha256, redact_secret,
    },
    resilience::{CircuitBreaker, is_retryable_status, parse_retry_after_ms},
    sse::consume_sse_response,
    url::validate_base_url,
};
use reqwest::{Client, Response, redirect::Policy};
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

/// HTTP inference client supporting OpenAI and Anthropic-compatible endpoints.
#[derive(Clone, Debug)]
pub struct HttpModelProvider {
    config: EndpointConfig,
    client: Client,
    breaker: Arc<CircuitBreaker>,
}

impl HttpModelProvider {
    /// Builds a bounded HTTP provider client (`redirect = none`, explicit timeout, circuit breaker).
    pub fn new(config: EndpointConfig) -> Result<Self, ProviderError> {
        validate_base_url(&config.base_url)?;
        if config.model.trim().is_empty() {
            return Err(ProviderError::InvalidConfig(
                "model identifier must be non-empty".into(),
            ));
        }
        if config.max_response_bytes == 0 || config.timeout_ms == 0 {
            return Err(ProviderError::InvalidConfig(
                "timeout_ms and max_response_bytes must be positive".into(),
            ));
        }
        let client = Client::builder()
            .redirect(Policy::none())
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .map_err(|e| {
                ProviderError::InvalidConfig(redact_secret(
                    &e.to_string(),
                    config.api_key.as_deref(),
                ))
            })?;
        let breaker = Arc::new(CircuitBreaker::new(config.circuit_breaker));
        Ok(Self {
            config,
            client,
            breaker,
        })
    }

    /// Returns the sanitized endpoint configuration.
    #[must_use]
    pub const fn config(&self) -> &EndpointConfig {
        &self.config
    }

    fn endpoint_url(&self) -> String {
        let base = self.config.base_url.trim_end_matches('/');
        match self.config.kind {
            ProviderKind::OpenAiCompatible => format!("{base}/v1/chat/completions"),
            ProviderKind::AnthropicCompatible => format!("{base}/v1/messages"),
        }
    }

    async fn send_with_resilience(
        &self,
        req: &ProviderRequest,
        body: &[u8],
        request_sha256: &str,
    ) -> Result<Response, ProviderError> {
        let url = self.endpoint_url();
        let max_retries = self.config.retry.max_retries;
        let mut attempt = 0u32;

        loop {
            if let Err(msg) = self.breaker.allow_request() {
                return Err(self.transport_err(req, request_sha256, msg));
            }

            let mut builder = self
                .client
                .post(&url)
                .header("content-type", "application/json")
                .body(body.to_vec());

            if let Some(key) = &self.config.api_key {
                match self.config.kind {
                    ProviderKind::OpenAiCompatible => {
                        builder = builder.header("authorization", format!("Bearer {key}"));
                    }
                    ProviderKind::AnthropicCompatible => {
                        builder = builder
                            .header("x-api-key", key)
                            .header("anthropic-version", ANTHROPIC_API_VERSION);
                    }
                }
            }

            match builder.send().await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    if is_retryable_status(status) {
                        self.breaker.record_transient_failure();
                        if attempt < max_retries {
                            let ra = resp
                                .headers()
                                .get("retry-after")
                                .and_then(|v| v.to_str().ok())
                                .and_then(parse_retry_after_ms);
                            let delay = self.config.retry.delay_for(attempt, ra);
                            attempt = attempt.saturating_add(1);
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                    } else if (200..300).contains(&status) {
                        self.breaker.record_success();
                    }
                    return Ok(resp);
                }
                Err(err) => {
                    self.breaker.record_transient_failure();
                    if attempt < max_retries {
                        let delay = self.config.retry.delay_for(attempt, None);
                        attempt = attempt.saturating_add(1);
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    return Err(self.transport_err(req, request_sha256, err.to_string()));
                }
            }
        }
    }

    fn build_request_body(
        &self,
        req: &ProviderRequest,
        stream: bool,
    ) -> Result<Vec<u8>, ProviderError> {
        match self.config.kind {
            ProviderKind::OpenAiCompatible => openai::build_body(&self.config.model, req, stream),
            ProviderKind::AnthropicCompatible => {
                anthropic::build_body(&self.config.model, req, stream)
            }
        }
    }

    async fn execute(&self, req: &ProviderRequest) -> Result<ProviderOutcome, ProviderError> {
        let body = self.build_request_body(req, false)?;
        let request_sha256 = hex_sha256(&body);
        let resp = self
            .send_with_resilience(req, &body, &request_sha256)
            .await?;
        let status = resp.status().as_u16();
        let raw = match read_bounded_body(resp, self.config.max_response_bytes).await {
            Ok(raw) => raw,
            Err(BodyReadError::Transport(msg)) => {
                return Err(self.transport_err(req, &request_sha256, msg));
            }
            Err(BodyReadError::TooLarge(msg)) => {
                return Err(self.upstream_err(req, Some(status), &request_sha256, b"", msg));
            }
        };
        if !(200..300).contains(&status) {
            return Err(self.upstream_err(
                req,
                Some(status),
                &request_sha256,
                &raw,
                format!("non-success HTTP status {status}"),
            ));
        }

        let parsed = match self.config.kind {
            ProviderKind::OpenAiCompatible => openai::parse_body(&raw),
            ProviderKind::AnthropicCompatible => anthropic::parse_body(&raw),
        };
        let (text, tool_calls, finish_reason, usage) = match parsed {
            Ok(v) => v,
            Err(msg) => {
                return Err(self.upstream_err(req, Some(status), &request_sha256, &raw, msg));
            }
        };
        Ok(finalize_outcome(
            &self.config,
            self.endpoint_url(),
            req,
            status,
            &request_sha256,
            &raw,
            text,
            tool_calls,
            finish_reason,
            usage,
        ))
    }

    async fn execute_stream(
        &self,
        req: &ProviderRequest,
        on_delta: &mut (dyn FnMut(StreamDelta) + Send),
    ) -> Result<ProviderOutcome, ProviderError> {
        let body = self.build_request_body(req, true)?;
        let request_sha256 = hex_sha256(&body);
        let resp = self
            .send_with_resilience(req, &body, &request_sha256)
            .await?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let (raw, msg) = match read_bounded_body(resp, self.config.max_response_bytes).await {
                Ok(bytes) => (bytes, format!("non-success HTTP status {status}")),
                Err(BodyReadError::TooLarge(err)) => (
                    Vec::new(),
                    format!("non-success HTTP status {status}: {err}"),
                ),
                Err(BodyReadError::Transport(_)) => {
                    (Vec::new(), format!("non-success HTTP status {status}"))
                }
            };
            return Err(self.upstream_err(req, Some(status), &request_sha256, &raw, msg));
        }
        let (raw_all, text, finish_reason, tool_calls, usage) = match consume_sse_response(
            resp,
            self.config.kind,
            self.config.max_response_bytes,
            on_delta,
        )
        .await
        {
            Ok(v) => v,
            Err((true, msg)) => return Err(self.transport_err(req, &request_sha256, msg)),
            Err((false, msg)) => {
                return Err(self.upstream_err(req, Some(status), &request_sha256, b"", msg));
            }
        };
        let outcome = finalize_outcome(
            &self.config,
            self.endpoint_url(),
            req,
            status,
            &request_sha256,
            &raw_all,
            text,
            tool_calls,
            finish_reason,
            usage,
        );
        on_delta(StreamDelta::Completed(Box::new(outcome.clone())));
        Ok(outcome)
    }

    fn transport_err(
        &self,
        req: &ProviderRequest,
        request_sha256: &str,
        raw_message: String,
    ) -> ProviderError {
        build_transport_error(
            &self.config,
            self.endpoint_url(),
            req,
            request_sha256,
            raw_message,
        )
    }

    fn upstream_err(
        &self,
        req: &ProviderRequest,
        status: Option<u16>,
        request_sha256: &str,
        raw: &[u8],
        raw_message: String,
    ) -> ProviderError {
        build_upstream_error(
            &self.config,
            self.endpoint_url(),
            req,
            status,
            request_sha256,
            raw,
            raw_message,
        )
    }
}

impl ModelProvider for HttpModelProvider {
    fn complete<'a>(
        &'a self,
        req: &'a ProviderRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderOutcome, ProviderError>> + Send + 'a>> {
        Box::pin(self.execute(req))
    }

    fn complete_stream<'a>(
        &'a self,
        req: &'a ProviderRequest,
        on_delta: &'a mut (dyn FnMut(StreamDelta) + Send),
    ) -> Pin<Box<dyn Future<Output = Result<ProviderOutcome, ProviderError>> + Send + 'a>> {
        Box::pin(self.execute_stream(req, on_delta))
    }
}
