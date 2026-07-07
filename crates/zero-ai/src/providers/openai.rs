use crate::{
    BoxStream,
    provider::{LlmProvider, StreamOptions},
    types::{
        AssistantContentBlock, AssistantMessage, ContentBlock, LlmContext, Message, Model,
        StopReason, StreamEvent, ToolCall, Usage,
    },
};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const OPENAI_API_URL: &str = "https://api.openai.com/v1/chat/completions";

pub struct OpenAIProvider;

#[async_trait]
impl LlmProvider for OpenAIProvider {
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
            .unwrap_or(OPENAI_API_URL)
            .to_string();

        let client = reqwest::Client::new();
        let resp = client
            .post(&url)
            .bearer_auth(&model.api_key)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .context("openai request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("openai error {status}: {text}"));
        }

        let model_id = model.id.clone();
        // 用 Arc<Mutex> 让状态在 flat_map 闭包里共享
        let state = Arc::new(Mutex::new(OpenAIParseState::new(model_id)));

        let stream = resp
            .bytes_stream()
            .eventsource()
            .flat_map(move |item| {
                let state = state.clone();
                let events: Vec<Result<StreamEvent>> = match item {
                    Ok(event) => {
                        if event.data.trim() == "[DONE]" {
                            let msg = state.lock().unwrap().finalize();
                            vec![Ok(StreamEvent::Done(msg))]
                        } else {
                            let mut st = state.lock().unwrap();
                            parse_openai_delta(&event.data, &mut st)
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

impl OpenAIProvider {
    fn build_request(&self, model: &Model, context: &LlmContext, options: &StreamOptions) -> Value {
        let mut messages = Vec::new();

        if let Some(sp) = &context.system_prompt {
            messages.push(serde_json::json!({"role": "system", "content": sp}));
        }

        for msg in &context.messages {
            self.convert_message(msg, &mut messages);
        }

        let mut body = serde_json::json!({
            "model": model.id,
            "max_tokens": options.max_tokens.unwrap_or(model.max_tokens),
            "stream": true,
            "stream_options": {"include_usage": true},
            "messages": messages,
        });

        if let Some(temp) = options.temperature {
            body["temperature"] = serde_json::json!(temp);
        }

        if !context.tools.is_empty() {
            let tools: Vec<Value> = context
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect();
            body["tools"] = Value::Array(tools);
        }

        body
    }

    fn convert_message(&self, msg: &Message, out: &mut Vec<Value>) {
        match msg {
            Message::User(u) => {
                let text = u
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                    })
                    .collect::<Vec<_>>()
                    .join("");
                out.push(serde_json::json!({"role": "user", "content": text}));
            }
            Message::Assistant(a) => {
                let text = a
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        AssistantContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");

                let tool_calls: Vec<Value> = a
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        AssistantContentBlock::ToolUse(tc) => Some(serde_json::json!({
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.name,
                                "arguments": tc.arguments.to_string(),
                            }
                        })),
                        _ => None,
                    })
                    .collect();

                let mut m = serde_json::json!({"role": "assistant", "content": text});
                if !tool_calls.is_empty() {
                    m["tool_calls"] = Value::Array(tool_calls);
                }
                out.push(m);
            }
            Message::ToolResult(tr) => {
                let text = tr
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                    })
                    .collect::<Vec<_>>()
                    .join("");
                out.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": tr.tool_call_id,
                    "content": text,
                }));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// OpenAI delta 聚合状态机
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct ToolCallBuffer {
    id: String,
    name: String,
    arguments: String,
}

struct OpenAIParseState {
    model_id: String,
    message: AssistantMessage,
    tool_buffers: HashMap<usize, ToolCallBuffer>,
    started: bool,
}

impl OpenAIParseState {
    fn new(model_id: String) -> Self {
        OpenAIParseState {
            model_id: model_id.clone(),
            message: AssistantMessage {
                model: model_id,
                ..Default::default()
            },
            tool_buffers: HashMap::new(),
            started: false,
        }
    }

    fn finalize(&mut self) -> AssistantMessage {
        let mut indices: Vec<usize> = self.tool_buffers.keys().cloned().collect();
        indices.sort();
        for idx in indices {
            if let Some(buf) = self.tool_buffers.remove(&idx) {
                let arguments =
                    serde_json::from_str(&buf.arguments).unwrap_or(Value::String(buf.arguments));
                self.message
                    .content
                    .push(AssistantContentBlock::ToolUse(ToolCall {
                        id: buf.id,
                        name: buf.name,
                        arguments,
                    }));
            }
        }
        self.message.clone()
    }
}

/// 解析一个 SSE data chunk，返回 0 个或多个 StreamEvent
fn parse_openai_delta(data: &str, state: &mut OpenAIParseState) -> Vec<StreamEvent> {
    let json: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    let mut events = Vec::new();

    // usage chunk（choices 可能为空数组）
    if let Some(usage) = json.get("usage").filter(|v| !v.is_null()) {
        state.message.usage = Usage {
            input_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0),
            output_tokens: usage["completion_tokens"].as_u64().unwrap_or(0),
            ..Default::default()
        };
    }

    // choices 为空时（usage-only chunk）直接返回
    let choices = match json.get("choices").and_then(Value::as_array) {
        Some(c) if !c.is_empty() => c,
        _ => return events,
    };

    let choice = &choices[0];
    let delta = match choice.get("delta") {
        Some(d) => d,
        None => return events,
    };

    // 第一个有 delta 的 chunk 时发出 Start
    if !state.started {
        state.started = true;
        events.push(StreamEvent::Start(AssistantMessage {
            model: state.model_id.clone(),
            ..Default::default()
        }));
    }

    // finish_reason
    if let Some(reason) = choice.get("finish_reason").filter(|v| !v.is_null()) {
        state.message.stop_reason = match reason.as_str().unwrap_or("") {
            "stop" => StopReason::Stop,
            "tool_calls" => StopReason::ToolUse,
            "length" => StopReason::Length,
            _ => StopReason::Stop,
        };
    }

    // text delta（content 字段）
    if let Some(text) = delta.get("content").and_then(Value::as_str) {
        if !text.is_empty() {
            match state.message.content.last_mut() {
                Some(AssistantContentBlock::Text { text: t }) => t.push_str(text),
                _ => state
                    .message
                    .content
                    .push(AssistantContentBlock::Text { text: text.to_string() }),
            }
            events.push(StreamEvent::TextDelta {
                index: 0,
                delta: text.to_string(),
            });
        }
    }

    // tool_calls delta
    if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
        for tc_delta in tool_calls {
            let index = tc_delta["index"].as_u64().unwrap_or(0) as usize;
            let buf = state.tool_buffers.entry(index).or_default();

            if let Some(id) = tc_delta.get("id").and_then(Value::as_str) {
                buf.id = id.to_string();
            }
            if let Some(name) = tc_delta.pointer("/function/name").and_then(Value::as_str) {
                buf.name = name.to_string();
            }
            if let Some(args) = tc_delta.pointer("/function/arguments").and_then(Value::as_str) {
                buf.arguments.push_str(args);
                events.push(StreamEvent::ToolCallDelta {
                    index,
                    delta: args.to_string(),
                });
            }
        }
    }

    events
}
