//! Live ephemeral-TCP HTTP integration tests for OpenAI and Anthropic endpoints.

use metron_protocol::{Digest, ProducerReceiptId, TaskId, content_digest};
use metron_provider_http::{
    CircuitBreakerConfig, EndpointConfig, HttpModelProvider, MessageRole, ModelProvider,
    ProviderError, ProviderKind, ProviderMessage, ProviderRequest, RetryPolicy,
};
use std::sync::{Arc, Mutex};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

const SECRET_KEY: &str = "sk-metron-super-secret-token-9999";

async fn spawn_http_once(status: &str, body: &'static str) -> (String, Arc<Mutex<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let captured = Arc::new(Mutex::new(String::new()));
    let cap_clone = Arc::clone(&captured);
    let status_owned = status.to_owned();

    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let n = sock.read(&mut buf).await.unwrap();
        *cap_clone.lock().unwrap() = String::from_utf8_lossy(&buf[..n]).into_owned();
        let reply = format!(
            "HTTP/1.1 {status_owned}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = sock.write_all(reply.as_bytes()).await;
    });

    (format!("http://{addr}"), captured)
}

fn sample_request() -> ProviderRequest {
    ProviderRequest {
        task_id: TaskId::new("task_http_01").unwrap(),
        intent_id: content_digest(b"intent"),
        dispatch_id: content_digest(b"dispatch"),
        producer_receipt_id: ProducerReceiptId::new("http_01").unwrap(),
        messages: vec![
            ProviderMessage::text(MessageRole::System, "You are concise."),
            ProviderMessage::text(MessageRole::User, "Say ok."),
        ],
        max_tokens: 64,
        temperature_milli: Some(0),
        tools: Vec::new(),
        response_format: None,
    }
}

fn test_cfg(
    kind: ProviderKind,
    base_url: String,
    model: &str,
    api_key: Option<&str>,
) -> EndpointConfig {
    EndpointConfig {
        kind,
        base_url,
        model: model.to_owned(),
        api_key: api_key.map(str::to_owned),
        timeout_ms: 3_000,
        max_response_bytes: 16 * 1024,
        pricing_override: None,
        retry: RetryPolicy {
            max_retries: 0,
            base_delay_ms: 10,
            max_delay_ms: 100,
        },
        circuit_breaker: CircuitBreakerConfig::default(),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn openai_compatible_roundtrip_emits_receipt_and_redacts_secret() {
    let (base_url, captured) = spawn_http_once(
        "200 OK",
        r#"{"choices":[{"message":{"content":"ok-openai"},"finish_reason":"stop"}],"usage":{"prompt_tokens":11,"completion_tokens":3,"total_tokens":14}}"#,
    )
    .await;

    let cfg = test_cfg(
        ProviderKind::OpenAiCompatible,
        base_url,
        "gpt-4o",
        Some(SECRET_KEY),
    );
    assert!(!format!("{cfg:?}").contains(SECRET_KEY));

    let provider = HttpModelProvider::new(cfg).unwrap();
    let out = provider.complete(&sample_request()).await.unwrap();
    assert_eq!(out.text, "ok-openai");
    assert_eq!(out.usage.total_tokens, 14);
    assert!(
        out.usage.spend_micros_usd > 0,
        "expected computed spend_micros_usd"
    );
    assert_eq!(out.receipt.http_status, Some(200));
    assert!(out.receipt.protocol_receipt.is_some());

    let raw_http = captured.lock().unwrap().clone();
    let lower = raw_http.to_ascii_lowercase();
    assert!(raw_http.starts_with("POST /v1/chat/completions HTTP/1.1"));
    assert!(lower.contains(&format!("authorization: bearer {SECRET_KEY}")));
    let _: Digest = out
        .receipt
        .protocol_receipt
        .unwrap()
        .data()
        .action_digest
        .clone();
}

#[tokio::test(flavor = "current_thread")]
async fn anthropic_compatible_roundtrip_sets_headers_and_system() {
    let (base_url, captured) = spawn_http_once(
        "200 OK",
        r#"{"content":[{"type":"text","text":"ok-anthropic"}],"stop_reason":"end_turn","usage":{"input_tokens":9,"output_tokens":4}}"#,
    )
    .await;

    let cfg = test_cfg(
        ProviderKind::AnthropicCompatible,
        base_url,
        "claude-sonnet-4",
        Some(SECRET_KEY),
    );
    let provider = HttpModelProvider::new(cfg).unwrap();
    let out = provider.complete(&sample_request()).await.unwrap();
    assert_eq!(out.text, "ok-anthropic");
    assert_eq!(out.usage.input_tokens, 9);
    assert_eq!(out.usage.output_tokens, 4);
    assert!(out.usage.spend_micros_usd > 0);
    assert!(out.receipt.protocol_receipt.is_some());

    let raw_http = captured.lock().unwrap().clone();
    let lower = raw_http.to_ascii_lowercase();
    assert!(raw_http.starts_with("POST /v1/messages HTTP/1.1"));
    assert!(lower.contains(&format!("x-api-key: {SECRET_KEY}")));
    assert!(lower.contains("anthropic-version: 2023-06-01"));
    assert!(raw_http.contains("\"system\":\"You are concise.\""));
}

#[tokio::test(flavor = "current_thread")]
async fn upstream_error_and_oversized_response_emit_redacted_failures() {
    let (base_url, _) =
        spawn_http_once("429 Too Many Requests", r#"{"error":"rate_limited"}"#).await;
    let cfg = test_cfg(
        ProviderKind::OpenAiCompatible,
        base_url,
        "gpt-4o",
        Some(SECRET_KEY),
    );
    let provider = HttpModelProvider::new(cfg).unwrap();
    let err = provider.complete(&sample_request()).await.unwrap_err();
    match &err {
        ProviderError::Upstream {
            status, receipt, ..
        } => {
            assert_eq!(*status, Some(429));
            assert_eq!(receipt.http_status, Some(429));
            assert!(receipt.protocol_receipt.is_some());
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
    assert!(!format!("{err:?}").contains(SECRET_KEY));

    let (base_url2, _) = spawn_http_once("200 OK", "0123456789abcdef0123456789abcdef").await;
    let mut cfg2 = test_cfg(ProviderKind::OpenAiCompatible, base_url2, "gpt-4o", None);
    cfg2.max_response_bytes = 8;
    let provider2 = HttpModelProvider::new(cfg2).unwrap();
    assert!(provider2.complete(&sample_request()).await.is_err());
}
