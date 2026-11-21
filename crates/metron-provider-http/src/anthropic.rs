//! Anthropic-compatible `/v1/messages` JSON codec (system blocks with `cache_control`,
//! `tool_use` / `tool_result`, JSON schema output hints, and streaming).

use crate::port::{
    CacheControl, MessageRole, ProviderError, ProviderRequest, ProviderUsage, ResponseFormat,
    ToolCall,
};
use serde::Deserialize;
use serde_json::{Value, json};

/// Pinned Anthropic API version header value.
pub const ANTHROPIC_API_VERSION: &str = "2023-06-01";

#[derive(Deserialize)]
struct AnthropicResponse {
    #[serde(default)]
    content: Vec<AnthropicBlock>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<AnthropicUsage>,
}

#[derive(Deserialize)]
struct AnthropicBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Option<Value>,
}

#[derive(Deserialize)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

pub(crate) fn build_body(
    model: &str,
    req: &ProviderRequest,
    stream: bool,
) -> Result<Vec<u8>, ProviderError> {
    let mut system_plain: Vec<&str> = Vec::new();
    let mut system_blocks: Vec<Value> = Vec::new();
    let mut has_cached_system = false;
    let mut messages: Vec<Value> = Vec::new();

    for msg in &req.messages {
        match msg.role {
            MessageRole::System => {
                system_plain.push(&msg.content);
                if msg.cache_control == Some(CacheControl::Ephemeral) {
                    has_cached_system = true;
                    system_blocks.push(json!({
                        "type": "text",
                        "text": msg.content,
                        "cache_control": { "type": "ephemeral" }
                    }));
                } else {
                    system_blocks.push(json!({ "type": "text", "text": msg.content }));
                }
            }
            MessageRole::User => {
                let content = if msg.cache_control == Some(CacheControl::Ephemeral) {
                    json!([{
                        "type": "text",
                        "text": msg.content,
                        "cache_control": { "type": "ephemeral" }
                    }])
                } else {
                    json!(msg.content)
                };
                messages.push(json!({ "role": "user", "content": content }));
            }
            MessageRole::Assistant => {
                if msg.tool_calls.is_empty() && msg.cache_control.is_none() {
                    messages.push(json!({ "role": "assistant", "content": msg.content }));
                } else {
                    let mut blocks = Vec::new();
                    if !msg.content.is_empty() {
                        let mut b = json!({ "type": "text", "text": msg.content });
                        if msg.cache_control == Some(CacheControl::Ephemeral) {
                            b["cache_control"] = json!({ "type": "ephemeral" });
                        }
                        blocks.push(b);
                    }
                    for tc in &msg.tool_calls {
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.name,
                            "input": tc.arguments,
                        }));
                    }
                    messages.push(json!({ "role": "assistant", "content": blocks }));
                }
            }
            MessageRole::Tool => {
                let Some(id) = &msg.tool_call_id else {
                    return Err(ProviderError::InvalidConfig(
                        "Anthropic tool_result message requires tool_call_id".into(),
                    ));
                };
                let mut block = json!({
                    "type": "tool_result",
                    "tool_use_id": id,
                    "content": msg.content,
                });
                if msg.cache_control == Some(CacheControl::Ephemeral) {
                    block["cache_control"] = json!({ "type": "ephemeral" });
                }
                messages.push(json!({ "role": "user", "content": [block] }));
            }
        }
    }

    if messages.is_empty() {
        return Err(ProviderError::InvalidConfig(
            "Anthropic request requires at least one non-system message".into(),
        ));
    }

    let mut payload = serde_json::Map::new();
    payload.insert("model".to_owned(), json!(model));
    payload.insert("max_tokens".to_owned(), json!(req.max_tokens));
    if !system_plain.is_empty() {
        if has_cached_system {
            payload.insert("system".to_owned(), Value::Array(system_blocks));
        } else {
            payload.insert("system".to_owned(), json!(system_plain.join("\n\n")));
        }
    }
    payload.insert("messages".to_owned(), Value::Array(messages));
    if let Some(milli) = req.temperature_milli {
        payload.insert("temperature".to_owned(), json!(f32::from(milli) / 1000.0));
    }
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.input_schema,
                })
            })
            .collect();
        payload.insert("tools".to_owned(), Value::Array(tools));
    }
    if let Some(ResponseFormat::JsonSchema { name, schema, .. }) = &req.response_format {
        payload.insert(
            "output_format".to_owned(),
            json!({
                "type": "json_schema",
                "name": name,
                "schema": schema,
            }),
        );
    } else if matches!(req.response_format, Some(ResponseFormat::JsonObject)) {
        payload.insert("output_format".to_owned(), json!({ "type": "json_object" }));
    }
    if stream {
        payload.insert("stream".to_owned(), json!(true));
    }

    serde_json::to_vec(&Value::Object(payload))
        .map_err(|e| ProviderError::InvalidConfig(e.to_string()))
}

pub(crate) fn parse_body(
    raw: &[u8],
) -> Result<(String, Vec<ToolCall>, String, ProviderUsage), String> {
    let parsed: AnthropicResponse =
        serde_json::from_slice(raw).map_err(|e| format!("invalid Anthropic response JSON: {e}"))?;
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for block in parsed.content {
        if block.kind == "text" {
            text.push_str(&block.text);
        } else if block.kind == "tool_use"
            && let (Some(id), Some(name)) = (block.id, block.name)
        {
            tool_calls.push(ToolCall {
                id,
                name,
                arguments: block.input.unwrap_or(Value::Null),
            });
        }
    }
    let finish = parsed.stop_reason.unwrap_or_else(|| "end_turn".to_owned());
    let usage = match parsed.usage {
        Some(u) => ProviderUsage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            total_tokens: u.input_tokens.saturating_add(u.output_tokens),
            spend_micros_usd: 0,
        },
        None => ProviderUsage::default(),
    };
    Ok((text, tool_calls, finish, usage))
}
