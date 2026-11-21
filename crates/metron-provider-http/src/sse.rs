//! Incremental Server-Sent Events (SSE) parser for OpenAI and Anthropic streaming responses.

use crate::port::{ProviderKind, ProviderUsage, StreamDelta, ToolCall};
use reqwest::Response;
use serde_json::Value;
use std::collections::BTreeMap;

/// Reads a bounded HTTP SSE response stream chunk-by-chunk, invoking `on_delta`,
/// and returns `(raw_bytes, text, finish_reason, tool_calls, usage)`.
pub(crate) async fn consume_sse_response(
    mut resp: Response,
    kind: ProviderKind,
    max_response_bytes: usize,
    on_delta: &mut (dyn FnMut(StreamDelta) + Send),
) -> Result<(Vec<u8>, String, String, Vec<ToolCall>, ProviderUsage), (bool, String)> {
    let mut acc = SseAccumulator::new(kind);
    let mut raw_all = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| (true, e.to_string()))? {
        if raw_all.len().saturating_add(chunk.len()) > max_response_bytes {
            return Err((
                false,
                "streaming response exceeded max_response_bytes".into(),
            ));
        }
        raw_all.extend_from_slice(&chunk);
        acc.push_chunk(&chunk, on_delta);
    }
    let (text, finish_reason, tool_calls, usage) = acc.finish(on_delta);
    Ok((raw_all, text, finish_reason, tool_calls, usage))
}

#[derive(Default)]
struct PartialTool {
    id: String,
    name: String,
    args_buf: String,
}

/// Stateful incremental SSE decoder assembling final text, tool calls, finish reason, and usage.
pub struct SseAccumulator {
    kind: ProviderKind,
    line_buf: String,
    text: String,
    finish_reason: Option<String>,
    usage: ProviderUsage,
    tools: BTreeMap<usize, PartialTool>,
}

impl SseAccumulator {
    /// Creates an SSE accumulator for the specified provider dialect.
    #[must_use]
    pub fn new(kind: ProviderKind) -> Self {
        Self {
            kind,
            line_buf: String::new(),
            text: String::new(),
            finish_reason: None,
            usage: ProviderUsage::default(),
            tools: BTreeMap::new(),
        }
    }

    /// Feeds a raw UTF-8 chunk from HTTP response body and emits parsed [`StreamDelta`]s.
    pub fn push_chunk(&mut self, chunk: &[u8], on_delta: &mut dyn FnMut(StreamDelta)) {
        self.line_buf.push_str(&String::from_utf8_lossy(chunk));
        while let Some(pos) = self.line_buf.find('\n') {
            let line = self.line_buf[..pos].trim_end_matches('\r').to_owned();
            self.line_buf.drain(..=pos);
            self.process_line(&line, on_delta);
        }
    }

    /// Flushes any trailing line without newline and returns `(text, finish_reason, tool_calls, usage)`.
    pub fn finish(
        mut self,
        on_delta: &mut dyn FnMut(StreamDelta),
    ) -> (String, String, Vec<ToolCall>, ProviderUsage) {
        if !self.line_buf.trim().is_empty() {
            let tail = std::mem::take(&mut self.line_buf);
            self.process_line(tail.trim(), on_delta);
        }
        let tool_calls = self
            .tools
            .into_values()
            .map(|t| {
                let arguments =
                    serde_json::from_str(&t.args_buf).unwrap_or(Value::String(t.args_buf));
                ToolCall {
                    id: t.id,
                    name: t.name,
                    arguments,
                }
            })
            .collect();
        let default_finish = match self.kind {
            ProviderKind::OpenAiCompatible => "stop",
            ProviderKind::AnthropicCompatible => "end_turn",
        };
        if self.usage.total_tokens == 0 {
            self.usage.total_tokens = self
                .usage
                .input_tokens
                .saturating_add(self.usage.output_tokens);
        }
        (
            self.text,
            self.finish_reason
                .unwrap_or_else(|| default_finish.to_owned()),
            tool_calls,
            self.usage,
        )
    }

    fn process_line(&mut self, line: &str, on_delta: &mut dyn FnMut(StreamDelta)) {
        let Some(data) = line.strip_prefix("data:") else {
            return;
        };
        let payload = data.trim();
        if payload.is_empty() || payload == "[DONE]" {
            return;
        }
        let Ok(val) = serde_json::from_str::<Value>(payload) else {
            return;
        };
        match self.kind {
            ProviderKind::OpenAiCompatible => self.handle_openai(&val, on_delta),
            ProviderKind::AnthropicCompatible => self.handle_anthropic(&val, on_delta),
        }
    }

    fn handle_openai(&mut self, val: &Value, on_delta: &mut dyn FnMut(StreamDelta)) {
        if let Some(u) = val.get("usage").and_then(Value::as_object) {
            if let Some(p) = u.get("prompt_tokens").and_then(Value::as_u64) {
                self.usage.input_tokens = p;
            }
            if let Some(c) = u.get("completion_tokens").and_then(Value::as_u64) {
                self.usage.output_tokens = c;
            }
            if let Some(t) = u.get("total_tokens").and_then(Value::as_u64) {
                self.usage.total_tokens = t;
            }
        }
        let Some(choice) = val
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
        else {
            return;
        };
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(reason.to_owned());
        }
        let Some(delta) = choice.get("delta") else {
            return;
        };
        if let Some(content) = delta.get("content").and_then(Value::as_str)
            && !content.is_empty()
        {
            self.text.push_str(content);
            on_delta(StreamDelta::TextDelta(content.to_owned()));
        }
        if let Some(tcs) = delta.get("tool_calls").and_then(Value::as_array) {
            for tc in tcs {
                let idx = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let entry = self.tools.entry(idx).or_default();
                let id = tc.get("id").and_then(Value::as_str).map(str::to_owned);
                if let Some(ref i) = id {
                    entry.id.clone_from(i);
                }
                let func = tc.get("function");
                let name = func
                    .and_then(|f| f.get("name"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if let Some(ref n) = name {
                    entry.name.clone_from(n);
                }
                let arg_delta = func
                    .and_then(|f| f.get("arguments"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                entry.args_buf.push_str(&arg_delta);
                on_delta(StreamDelta::ToolCallDelta {
                    index: idx,
                    id,
                    name,
                    arguments_delta: arg_delta,
                });
            }
        }
    }

    fn handle_anthropic(&mut self, val: &Value, on_delta: &mut dyn FnMut(StreamDelta)) {
        let Some(kind) = val.get("type").and_then(Value::as_str) else {
            return;
        };
        match kind {
            "message_start" => {
                if let Some(u) = val
                    .get("message")
                    .and_then(|m| m.get("usage"))
                    .and_then(Value::as_object)
                    && let Some(i) = u.get("input_tokens").and_then(Value::as_u64)
                {
                    self.usage.input_tokens = i;
                }
            }
            "content_block_start" => {
                let idx = val.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                if let Some(block) = val.get("content_block")
                    && block.get("type").and_then(Value::as_str) == Some("tool_use")
                {
                    let entry = self.tools.entry(idx).or_default();
                    let id = block.get("id").and_then(Value::as_str).map(str::to_owned);
                    let name = block.get("name").and_then(Value::as_str).map(str::to_owned);
                    if let Some(ref i) = id {
                        entry.id.clone_from(i);
                    }
                    if let Some(ref n) = name {
                        entry.name.clone_from(n);
                    }
                    on_delta(StreamDelta::ToolCallDelta {
                        index: idx,
                        id,
                        name,
                        arguments_delta: String::new(),
                    });
                }
            }
            "content_block_delta" => {
                let idx = val.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let Some(delta) = val.get("delta") else {
                    return;
                };
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        if let Some(t) = delta.get("text").and_then(Value::as_str)
                            && !t.is_empty()
                        {
                            self.text.push_str(t);
                            on_delta(StreamDelta::TextDelta(t.to_owned()));
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(j) = delta.get("partial_json").and_then(Value::as_str) {
                            let entry = self.tools.entry(idx).or_default();
                            entry.args_buf.push_str(j);
                            on_delta(StreamDelta::ToolCallDelta {
                                index: idx,
                                id: None,
                                name: None,
                                arguments_delta: j.to_owned(),
                            });
                        }
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(r) = val
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    self.finish_reason = Some(r.to_owned());
                }
                if let Some(o) = val
                    .get("usage")
                    .and_then(|u| u.get("output_tokens"))
                    .and_then(Value::as_u64)
                {
                    self.usage.output_tokens = o;
                }
            }
            _ => {}
        }
    }
}
