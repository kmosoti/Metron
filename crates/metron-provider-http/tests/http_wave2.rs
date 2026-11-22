//! Wave 2 HTTP provider tests: tool calling, JSON schema `response_format`, `cache_control`,
//! pricing table, SSE `complete_stream`, `429` retry + `Retry-After`, circuit breaker, and
//! transport failure `ResourceReceipt` preservation.

use metron_protocol::{ProducerReceiptId, TaskId, content_digest};
use metron_provider_http::{
    CacheControl, CircuitBreakerConfig, EndpointConfig, HttpModelProvider, MessageRole,
    ModelProvider, ProviderError, ProviderKind, ProviderMessage, ProviderRequest, ResponseFormat,
    RetryPolicy, StreamDelta, ToolDefinition,
};
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn base_req() -> ProviderRequest {
    ProviderRequest {
        task_id: TaskId::new("task_wave2").unwrap(),
        intent_id: content_digest(b"intent2"),
        dispatch_id: content_digest(b"dispatch2"),
        producer_receipt_id: ProducerReceiptId::new("http_w2").unwrap(),
        messages: vec![
            ProviderMessage {
                role: MessageRole::System,
                content: "Cached system prefix.".to_owned(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                cache_control: Some(CacheControl::Ephemeral),
            },
            ProviderMessage::text(MessageRole::User, "Compute 2+2."),
            ProviderMessage::tool_result("call_1", r#"{"result":4}"#),
        ],
        max_tokens: 128,
        temperature_milli: Some(0),
        tools: vec![ToolDefinition {
            name: "wasm_compute".to_owned(),
            description: "Run sandboxed compute".to_owned(),
            input_schema: json!({"type":"object","properties":{"x":{"type":"integer"}}}),
        }],
        response_format: Some(ResponseFormat::JsonSchema {
            name: "answer".to_owned(),
            schema: json!({"type":"object","properties":{"ans":{"type":"integer"}}}),
            strict: true,
        }),
    }
}

async fn spawn_sequence(replies: Vec<String>) -> (String, Arc<AtomicUsize>, Arc<Mutex<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let last_req = Arc::new(Mutex::new(String::new()));
    let hits_bg = Arc::clone(&hits);
    let last_bg = Arc::clone(&last_req);

    tokio::spawn(async move {
        for reply in replies {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            hits_bg.fetch_add(1, Ordering::Relaxed);
            let mut buf = vec![0u8; 16384];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            *last_bg.lock().unwrap() = String::from_utf8_lossy(&buf[..n]).into_owned();
            let _ = sock.write_all(reply.as_bytes()).await;
        }
    });
    (format!("http://{addr}"), hits, last_req)
}

#[tokio::test(flavor = "current_thread")]
async fn openai_tools_schema_cache_and_sse_streaming() {
    let sse_body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"lo\",\"tool_calls\":[{\"index\":0,\"id\":\"call_9\",\"function\":{\"name\":\"wasm_compute\",\"arguments\":\"{\\\"x\\\":2}\"}}]},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":1000,\"completion_tokens\":200,\"total_tokens\":1200}}\n\n",
        "data: [DONE]\n\n"
    );
    let reply = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse_body}",
        sse_body.len()
    );
    let (base_url, _, last_req) = spawn_sequence(vec![reply]).await;
    let provider = HttpModelProvider::new(EndpointConfig {
        kind: ProviderKind::OpenAiCompatible,
        base_url,
        model: "gpt-4o".to_owned(),
        api_key: None,
        timeout_ms: 3_000,
        max_response_bytes: 32 * 1024,
        pricing_override: None,
        retry: RetryPolicy::default(),
        circuit_breaker: CircuitBreakerConfig::default(),
    })
    .unwrap();

    let mut deltas = Vec::new();
    let out = provider
        .complete_stream(&base_req(), &mut |d| deltas.push(d))
        .await
        .unwrap();

    assert_eq!(out.text, "Hello");
    assert_eq!(out.tool_calls.len(), 1);
    assert_eq!(out.tool_calls[0].id, "call_9");
    assert_eq!(out.tool_calls[0].name, "wasm_compute");
    assert_eq!(out.tool_calls[0].arguments, json!({"x": 2}));
    assert!(out.usage.spend_micros_usd > 0);
    assert!(
        deltas
            .iter()
            .any(|d| matches!(d, StreamDelta::TextDelta(t) if t == "Hel"))
    );
    assert!(
        deltas
            .iter()
            .any(|d| matches!(d, StreamDelta::Completed(_)))
    );

    let raw = last_req.lock().unwrap().clone();
    assert!(raw.contains("\"role\":\"tool\""));
    assert!(raw.contains("\"tool_call_id\":\"call_1\""));
    assert!(raw.contains("\"response_format\""));
    assert!(raw.contains("\"cache_control\""));
    assert!(raw.contains("\"stream\":true"));
}

#[tokio::test(flavor = "current_thread")]
async fn retry_429_recovers_and_circuit_breaker_preserves_transport_receipt() {
    let ok_body = r#"{"choices":[{"message":{"content":"recovered"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
    let replies = vec![
        "HTTP/1.1 429 Too Many Requests\r\nretry-after: 0\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}".to_owned(),
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{ok_body}",
            ok_body.len()
        ),
    ];
    let (base_url, hits, _) = spawn_sequence(replies).await;
    let provider = HttpModelProvider::new(EndpointConfig {
        kind: ProviderKind::OpenAiCompatible,
        base_url,
        model: "gpt-4o-mini".to_owned(),
        api_key: None,
        timeout_ms: 3_000,
        max_response_bytes: 16 * 1024,
        pricing_override: None,
        retry: RetryPolicy {
            max_retries: 2,
            base_delay_ms: 5,
            max_delay_ms: 50,
        },
        circuit_breaker: CircuitBreakerConfig {
            failure_threshold: 5,
            cooldown_ms: 5_000,
        },
    })
    .unwrap();
    let out = provider.complete(&base_req()).await.unwrap();
    assert_eq!(out.text, "recovered");
    assert_eq!(hits.load(Ordering::Relaxed), 2);

    // Trip circuit breaker on consecutive 503s and confirm Transport preserves failure ResourceReceipt.
    let err_503 =
        "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}"
            .to_owned();
    let (bad_url, _, _) = spawn_sequence(vec![err_503.clone(), err_503]).await;
    let tripped = HttpModelProvider::new(EndpointConfig {
        kind: ProviderKind::OpenAiCompatible,
        base_url: bad_url,
        model: "gpt-4o-mini".to_owned(),
        api_key: None,
        timeout_ms: 2_000,
        max_response_bytes: 16 * 1024,
        pricing_override: None,
        retry: RetryPolicy {
            max_retries: 0,
            base_delay_ms: 5,
            max_delay_ms: 50,
        },
        circuit_breaker: CircuitBreakerConfig {
            failure_threshold: 2,
            cooldown_ms: 60_000,
        },
    })
    .unwrap();
    assert!(tripped.complete(&base_req()).await.is_err());
    assert!(tripped.complete(&base_req()).await.is_err());
    let third = tripped.complete(&base_req()).await.unwrap_err();
    match third {
        ProviderError::Transport { message, receipt } => {
            assert!(message.contains("circuit breaker open"), "got: {message}");
            assert_eq!(receipt.http_status, None);
            assert!(receipt.protocol_receipt.is_some());
        }
        other => panic!("expected Transport with failure receipt, got {other:?}"),
    }
}

fn cfg_with_url(base_url: &str, max_response_bytes: usize) -> EndpointConfig {
    EndpointConfig {
        kind: ProviderKind::OpenAiCompatible,
        base_url: base_url.to_owned(),
        model: "gpt-4o-mini".to_owned(),
        api_key: None,
        timeout_ms: 2_000,
        max_response_bytes,
        pricing_override: None,
        retry: RetryPolicy {
            max_retries: 0,
            base_delay_ms: 5,
            max_delay_ms: 50,
        },
        circuit_breaker: CircuitBreakerConfig {
            failure_threshold: 0,
            cooldown_ms: 1_000,
        },
    }
}

#[test]
fn rejects_ssrf_metadata_userinfo_query_and_obfuscated_base_urls() {
    for bad in [
        "http://169.254.169.254/latest",
        "http://[::ffff:169.254.169.254]",
        "http://[fe80::1]:8080",
        "http://0.0.0.0:8080",
        "http://metadata.google.internal",
        "https://user:pass@api.example.com",
        "https://api.example.com?foo=bar",
        "http://2852039166",
        "http://169.16690942",
        "http://169.254.43518",
        "http://0.0",
        "http://0177.0.0.1",
        "http://169.254.169.254.",
        "http://metadata.google.internal.",
        "ftp://127.0.0.1:8080",
    ] {
        assert!(
            matches!(
                HttpModelProvider::new(cfg_with_url(bad, 4096)),
                Err(ProviderError::InvalidConfig(_))
            ),
            "expected InvalidConfig for {bad}"
        );
    }

    for good in [
        "http://127.0.0.1:8080",
        "http://localhost:11434",
        "https://[::1]:8443/v1",
        "https://api.openai.com",
    ] {
        assert!(
            HttpModelProvider::new(cfg_with_url(good, 4096)).is_ok(),
            "expected valid base_url for {good}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_chunked_response_and_stream_error_exceeding_max_response_bytes() {
    let oversized = "x".repeat(256);
    let chunked_ok = format!(
        "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n{:x}\r\n{oversized}\r\n0\r\n\r\n",
        oversized.len()
    );
    let chunked_err = format!(
        "HTTP/1.1 500 Internal Server Error\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n{:x}\r\n{oversized}\r\n0\r\n\r\n",
        oversized.len()
    );
    let (base_url, _, _) = spawn_sequence(vec![chunked_ok, chunked_err]).await;
    let provider = HttpModelProvider::new(cfg_with_url(&base_url, 64)).unwrap();

    match provider.complete(&base_req()).await.unwrap_err() {
        ProviderError::Upstream { message, .. } => {
            assert!(message.contains("exceeds"), "got: {message}");
        }
        other => panic!("expected Upstream limit error, got {other:?}"),
    }

    let mut deltas = Vec::new();
    match provider
        .complete_stream(&base_req(), &mut |d| deltas.push(d))
        .await
        .unwrap_err()
    {
        ProviderError::Upstream {
            status, message, ..
        } => {
            assert_eq!(status, Some(500));
            assert!(message.contains("exceeds"), "got: {message}");
        }
        other => panic!("expected Upstream error for oversized 500 stream, got {other:?}"),
    }
}

#[test]
fn sse_handles_split_utf8_chunks_and_u64_max_cooldown_does_not_panic() {
    let line = b"data: {\"choices\":[{\"delta\":{\"content\":\"\xF0\x9F\xA6\x80\"}}]}\n\n";
    let split = line.iter().position(|&b| b == 0xF0).unwrap() + 2;
    let mut acc = metron_provider_http::sse::SseAccumulator::new(ProviderKind::OpenAiCompatible);
    let mut deltas = Vec::new();
    acc.push_chunk(&line[..split], &mut |d| deltas.push(d));
    acc.push_chunk(&line[split..], &mut |d| deltas.push(d));
    let (text, _, _, _) = acc.finish(&mut |d| deltas.push(d));
    assert_eq!(text, "🦀");

    let breaker = metron_provider_http::CircuitBreaker::new(CircuitBreakerConfig {
        failure_threshold: 1,
        cooldown_ms: u64::MAX,
    });
    breaker.record_transient_failure();
    assert!(breaker.allow_request().is_err());
}
