use crate::{
    BoxStream,
    provider::{LlmProvider, StreamOptions},
    types::{
        AssistantContentBlock, AssistantMessage, LlmContext, Message, Model, StopReason,
        StreamEvent, ToolCall, Usage, ContentBlock,
    },
};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider;

// ---------------------------------------------------------------------------
// LlmProvider 实现
// ---------------------------------------------------------------------------

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn stream(
        &self,
        model: &Model,
        context: &LlmContext,
        options: &StreamOptions,
    ) -> Result<BoxStream<Result<StreamEvent>>> {
        let body = self.build_request(model, context, options);
        let url = model
            .base_url
            .as_deref()
            .unwrap_or(ANTHROPIC_API_URL)
            .to_string();

        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .context("failed to build http client")?;
        let resp = client
            .post(&url)
            .header("x-api-key", &model.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .context("anthropic request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("anthropic error {status}: {text}"));
        }

        let model_id = model.id.clone();
        let state = Arc::new(Mutex::new(ParseState::new(model_id)));

        let stream = resp
            .bytes_stream()
            .eventsource()
            .flat_map(move |item| {
                let state = state.clone();
                let events: Vec<Result<StreamEvent>> = match item {
                    Ok(event) => {
                        if event.event == "message_stop" {
                            let msg = state.lock().unwrap().finalize();
                            vec![Ok(StreamEvent::Done(msg))]
                        } else {
                            let mut st = state.lock().unwrap();
                            parse_sse_event(&event.event, &event.data, &mut st)
                                .into_iter()
                                .map(Ok)
                                .collect()
                        }
                    }
                    Err(e) => vec![Err(anyhow!("SSE error: {e}"))],
                };
                futures::stream::iter(events)
            });

        Ok(Box::pin(stream))
    }
}

// ---------------------------------------------------------------------------
// 请求构建
// ---------------------------------------------------------------------------

impl AnthropicProvider {
    fn build_request(&self, model: &Model, context: &LlmContext, options: &StreamOptions) -> Value {
        let mut body = serde_json::json!({
            "model": model.id,
            "max_tokens": options.max_tokens.unwrap_or(model.max_tokens),
            "stream": true,
            "messages": self.convert_messages(&context.messages),
        });

        if let Some(sp) = &context.system_prompt {
            // 使用数组格式，在最后一个 block 打上 cache_control，
            // 让 Anthropic 在该位置（system prompt 末尾）建立缓存断点。
            body["system"] = serde_json::json!([{
                "type": "text",
                "text": sp,
                "cache_control": { "type": "ephemeral" }
            }]);
        }

        if let Some(temp) = options.temperature {
            body["temperature"] = serde_json::json!(temp);
        }

        if !context.tools.is_empty() {
            body["tools"] = self.convert_tools(&context.tools);
        }

        body
    }

    fn convert_messages(&self, messages: &[Message]) -> Value {
        let mut result = Vec::new();
        for msg in messages {
            match msg {
                Message::User(u) => {
                    let content: Vec<Value> = u
                        .content
                        .iter()
                        .map(|b| match b {
                            ContentBlock::Text { text } => {
                                serde_json::json!({"type": "text", "text": text})
                            }
                        })
                        .collect();
                    result.push(serde_json::json!({"role": "user", "content": content}));
                }
                Message::Assistant(a) => {
                    let content: Vec<Value> = a
                        .content
                        .iter()
                        .map(|b| match b {
                            AssistantContentBlock::Text { text } => {
                                serde_json::json!({"type": "text", "text": text})
                            }
                            AssistantContentBlock::ToolUse(tc) => {
                                serde_json::json!({
                                    "type": "tool_use",
                                    "id": tc.id,
                                    "name": tc.name,
                                    "input": tc.arguments,
                                })
                            }
                            AssistantContentBlock::Thinking { thinking } => {
                                serde_json::json!({"type": "thinking", "thinking": thinking})
                            }
                        })
                        .collect();
                    result.push(serde_json::json!({"role": "assistant", "content": content}));
                }
                Message::ToolResult(tr) => {
                    // Anthropic tool_result 作为 user role 的 content block
                    let content: Vec<Value> = tr
                        .content
                        .iter()
                        .map(|b| match b {
                            ContentBlock::Text { text } => {
                                serde_json::json!({"type": "text", "text": text})
                            }
                        })
                        .collect();
                    result.push(serde_json::json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": tr.tool_call_id,
                            "content": content,
                            "is_error": tr.is_error,
                        }]
                    }));
                }
            }
        }
        Value::Array(result)
    }

    fn convert_tools(&self, tools: &[crate::types::ToolSpec]) -> Value {
        let arr: Vec<Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.parameters,
                })
            })
            .collect();
        Value::Array(arr)
    }
}

// ---------------------------------------------------------------------------
// SSE 解析状态机
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum BlockType {
    Text,
    Thinking(String),
    ToolUse { id: String, name: String, json_buf: String },
}

struct ParseState {
    message: AssistantMessage,
    blocks: HashMap<usize, BlockType>,
}

impl ParseState {
    fn new(model_id: String) -> Self {
        ParseState {
            message: AssistantMessage {
                model: model_id,
                ..Default::default()
            },
            blocks: HashMap::new(),
        }
    }

    fn finalize(&mut self) -> AssistantMessage {
        let msg = self.message.clone();
        if msg.stop_reason == StopReason::Stop && !msg.content.is_empty() {
            // already set
        }
        msg
    }
}

fn parse_sse_event(event_type: &str, data: &str, state: &mut ParseState) -> Vec<StreamEvent> {
    let json: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    match event_type {
        "message_start" => {
            if let Some(usage) = json.pointer("/message/usage") {
                state.message.usage = parse_usage(usage);
            }
            if let Some(m) = json.pointer("/message/model").and_then(Value::as_str) {
                state.message.model = m.to_string();
            }
            let start_msg = AssistantMessage {
                model: state.message.model.clone(),
                ..Default::default()
            };
            vec![StreamEvent::Start(start_msg)]
        }

        "content_block_start" => {
            let index = match json["index"].as_u64() {
                Some(i) => i as usize,
                None => return vec![],
            };
            let block_type = match json.pointer("/content_block/type").and_then(Value::as_str) {
                Some(t) => t,
                None => return vec![],
            };
            match block_type {
                "text" => { state.blocks.insert(index, BlockType::Text); }
                "thinking" => { state.blocks.insert(index, BlockType::Thinking(String::new())); }
                "tool_use" => {
                    let id = json.pointer("/content_block/id").and_then(Value::as_str).unwrap_or("").to_string();
                    let name = json.pointer("/content_block/name").and_then(Value::as_str).unwrap_or("").to_string();
                    state.blocks.insert(index, BlockType::ToolUse { id, name, json_buf: String::new() });
                }
                _ => {}
            }
            vec![]
        }

        "content_block_delta" => {
            let index = match json["index"].as_u64() {
                Some(i) => i as usize,
                None => return vec![],
            };
            let delta_type = match json.pointer("/delta/type").and_then(Value::as_str) {
                Some(t) => t,
                None => return vec![],
            };
            match delta_type {
                "thinking_delta" => {
                    let delta = json.pointer("/delta/thinking").and_then(Value::as_str).unwrap_or("").to_string();
                    if let Some(BlockType::Thinking(buf)) = state.blocks.get_mut(&index) {
                        buf.push_str(&delta);
                    }
                    vec![]  // thinking 不对外暴露
                }
                "text_delta" => {
                    let delta = json.pointer("/delta/text").and_then(Value::as_str).unwrap_or("").to_string();
                    match state.message.content.last_mut() {
                        Some(AssistantContentBlock::Text { text }) => text.push_str(&delta),
                        _ => state.message.content.push(AssistantContentBlock::Text { text: delta.clone() }),
                    }
                    vec![StreamEvent::TextDelta { index, delta }]
                }
                "input_json_delta" => {
                    let partial = json.pointer("/delta/partial_json").and_then(Value::as_str).unwrap_or("").to_string();
                    if let Some(BlockType::ToolUse { json_buf, .. }) = state.blocks.get_mut(&index) {
                        json_buf.push_str(&partial);
                    }
                    vec![StreamEvent::ToolCallDelta { index, delta: partial }]
                }
                _ => vec![],
            }
        }

        "content_block_stop" => {
            let index = match json["index"].as_u64() {
                Some(i) => i as usize,
                None => return vec![],
            };
            match state.blocks.remove(&index) {
                Some(BlockType::ToolUse { id, name, json_buf }) => {
                    let arguments = serde_json::from_str(&json_buf)
                        .unwrap_or(Value::Object(Default::default()));
                    let tc = ToolCall { id, name, arguments };
                    state.message.content.push(AssistantContentBlock::ToolUse(tc.clone()));
                    vec![StreamEvent::ToolCallEnd { index, tool_call: tc }]
                }
                Some(BlockType::Thinking(thinking)) => {
                    state.message.content.push(AssistantContentBlock::Thinking { thinking });
                    vec![]
                }
                _ => vec![],
            }
        }

        "message_delta" => {
            if let Some(sr) = json.pointer("/delta/stop_reason").and_then(Value::as_str) {
                state.message.stop_reason = parse_stop_reason(sr);
            }
            if let Some(usage) = json.get("usage") {
                let u = parse_usage(usage);
                // 合并：只覆盖非零的字段，保留 message_start 里可能已有的值
                if u.input_tokens > 0 { state.message.usage.input_tokens = u.input_tokens; }
                if u.output_tokens > 0 { state.message.usage.output_tokens = u.output_tokens; }
                if u.cache_read_tokens > 0 { state.message.usage.cache_read_tokens = u.cache_read_tokens; }
                if u.cache_write_tokens > 0 { state.message.usage.cache_write_tokens = u.cache_write_tokens; }
            }
            vec![]
        }

        _ => vec![],
    }
}

fn parse_usage(v: &Value) -> Usage {
    Usage {
        input_tokens: v["input_tokens"].as_u64().unwrap_or(0),
        output_tokens: v["output_tokens"].as_u64().unwrap_or(0),
        cache_read_tokens: v["cache_read_input_tokens"].as_u64().unwrap_or(0),
        cache_write_tokens: v["cache_creation_input_tokens"].as_u64().unwrap_or(0),
    }
}

fn parse_stop_reason(s: &str) -> StopReason {
    match s {
        "end_turn" => StopReason::Stop,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::Length,
        "error" => StopReason::Error,
        _ => StopReason::Stop,
    }
}
