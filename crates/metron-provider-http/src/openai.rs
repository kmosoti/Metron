//! OpenAI-compatible `/v1/chat/completions` JSON codec (messages, tools, tool calls,
//! `response_format` JSON schema, cache markers, and streaming).

use crate::port::{
    CacheControl, MessageRole, ProviderError, ProviderRequest, ProviderUsage, ResponseFormat,
    ToolCall,
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: OpenAiChoiceMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiChoiceMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<OpenAiToolCall>,
}

#[derive(Deserialize)]
struct OpenAiToolCall {
    id: String,
    function: OpenAiToolFunction,
}

#[derive(Deserialize)]
struct OpenAiToolFunction {
    name: String,
    #[serde(default)]
    arguments: String,
}

#[derive(Deserialize)]
struct OpenAiUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

pub(crate) fn build_body(
    model: &str,
    req: &ProviderRequest,
    stream: bool,
) -> Result<Vec<u8>, ProviderError> {
    let mut messages = Vec::with_capacity(req.messages.len());
    for m in &req.messages {
        let role = match m.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };
        let mut obj = serde_json::Map::new();
        obj.insert("role".to_owned(), json!(role));
        if m.cache_control == Some(CacheControl::Ephemeral) {
            obj.insert(
                "content".to_owned(),
                json!([{
                    "type": "text",
                    "text": m.content,
                    "cache_control": { "type": "ephemeral" }
                }]),
            );
        } else {
            obj.insert("content".to_owned(), json!(m.content));
        }
        if m.role == MessageRole::Tool {
            let Some(id) = &m.tool_call_id else {
                return Err(ProviderError::InvalidConfig(
                    "OpenAI tool message requires tool_call_id".into(),
                ));
            };
            obj.insert("tool_call_id".to_owned(), json!(id));
        }
        if !m.tool_calls.is_empty() {
            let calls: Vec<Value> = m
                .tool_calls
                .iter()
                .map(|tc| {
                    json!({
                        "id": tc.id,
                        "type": "function",
                        "function": {
                            "name": tc.name,
                            "arguments": tc.arguments.to_string(),
                        }
                    })
                })
                .collect();
            obj.insert("tool_calls".to_owned(), Value::Array(calls));
        }
        messages.push(Value::Object(obj));
    }

    let mut payload = serde_json::Map::new();
    payload.insert("model".to_owned(), json!(model));
    payload.insert("messages".to_owned(), Value::Array(messages));
    payload.insert("max_tokens".to_owned(), json!(req.max_tokens));
    if let Some(milli) = req.temperature_milli {
        payload.insert("temperature".to_owned(), json!(f32::from(milli) / 1000.0));
    }
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    }
                })
            })
            .collect();
        payload.insert("tools".to_owned(), Value::Array(tools));
    }
    if let Some(fmt) = &req.response_format {
        let rf = match fmt {
            ResponseFormat::Text => json!({ "type": "text" }),
            ResponseFormat::JsonObject => json!({ "type": "json_object" }),
            ResponseFormat::JsonSchema {
                name,
                schema,
                strict,
            } => json!({
                "type": "json_schema",
                "json_schema": {
                    "name": name,
                    "schema": schema,
                    "strict": strict,
                }
            }),
        };
        payload.insert("response_format".to_owned(), rf);
    }
    if stream {
        payload.insert("stream".to_owned(), json!(true));
        payload.insert(
            "stream_options".to_owned(),
            json!({ "include_usage": true }),
        );
    }

    serde_json::to_vec(&Value::Object(payload))
        .map_err(|e| ProviderError::InvalidConfig(e.to_string()))
}

pub(crate) fn parse_body(
    raw: &[u8],
) -> Result<(String, Vec<ToolCall>, String, ProviderUsage), String> {
    let parsed: OpenAiResponse =
        serde_json::from_slice(raw).map_err(|e| format!("invalid OpenAI response JSON: {e}"))?;
    let first = parsed
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| "OpenAI response contained zero choices".to_owned())?;
    let text = first.message.content.unwrap_or_default();
    let tool_calls = first
        .message
        .tool_calls
        .into_iter()
        .map(|tc| {
            let arguments = serde_json::from_str(&tc.function.arguments)
                .unwrap_or(Value::String(tc.function.arguments));
            ToolCall {
                id: tc.id,
                name: tc.function.name,
                arguments,
            }
        })
        .collect();
    let finish = first.finish_reason.unwrap_or_else(|| "stop".to_owned());
    let usage = match parsed.usage {
        Some(u) => ProviderUsage {
            input_tokens: u.prompt_tokens,
            output_tokens: u.completion_tokens,
            total_tokens: if u.total_tokens > 0 {
                u.total_tokens
            } else {
                u.prompt_tokens.saturating_add(u.completion_tokens)
            },
            spend_micros_usd: 0,
        },
        None => ProviderUsage::default(),
    };
    Ok((text, tool_calls, finish, usage))
}
