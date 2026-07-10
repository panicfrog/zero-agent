use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ---------------------------------------------------------------------------
// Content blocks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
}

impl ContentBlock {
    pub fn text(s: impl Into<String>) -> Self {
        ContentBlock::Text { text: s.into() }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            ContentBlock::Text { text } => Some(text),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantContentBlock {
    Text { text: String },
    ToolUse(ToolCall),
    Thinking { thinking: String },
}

// ---------------------------------------------------------------------------
// Tool call
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    pub content: Vec<ContentBlock>,
    #[serde(default = "now_ms")]
    pub timestamp: u64,
}

impl UserMessage {
    pub fn new(text: impl Into<String>) -> Self {
        UserMessage {
            content: vec![ContentBlock::text(text)],
            timestamp: now_ms(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AssistantMessage {
    pub content: Vec<AssistantContentBlock>,
    pub model: String,
    pub usage: Usage,
    pub stop_reason: StopReason,
    pub error_message: Option<String>,
    #[serde(default = "now_ms")]
    pub timestamp: u64,
}

impl AssistantMessage {
    pub fn text_output(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                AssistantContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn tool_calls(&self) -> Vec<&ToolCall> {
        self.content
            .iter()
            .filter_map(|b| match b {
                AssistantContentBlock::ToolUse(tc) => Some(tc),
                _ => None,
            })
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultMessage {
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: Vec<ContentBlock>,
    pub is_error: bool,
    #[serde(default = "now_ms")]
    pub timestamp: u64,
}

// ---------------------------------------------------------------------------
// Usage & StopReason
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    #[default]
    Stop,
    ToolUse,
    Length,
    Error,
    Aborted,
}

// ---------------------------------------------------------------------------
// LLM context & tool spec
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct LlmContext {
    pub system_prompt: Option<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema object for the tool's parameters
    pub parameters: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Model & Provider
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Model {
    pub id: String,
    pub provider: Provider,
    pub api_key: String,
    pub base_url: Option<String>,
    pub max_tokens: u32,
}

impl Model {
    pub fn anthropic(id: impl Into<String>, api_key: impl Into<String>) -> Self {
        Model {
            id: id.into(),
            provider: Provider::Anthropic,
            api_key: api_key.into(),
            base_url: None,
            max_tokens: 8192,
        }
    }

    pub fn openai(id: impl Into<String>, api_key: impl Into<String>) -> Self {
        Model {
            id: id.into(),
            provider: Provider::OpenAI,
            api_key: api_key.into(),
            base_url: None,
            max_tokens: 4096,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provider {
    Anthropic,
    OpenAI,
}

// ---------------------------------------------------------------------------
// Stream events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// LLM 开始响应，携带空的初始 AssistantMessage（含 model 信息）
    Start(AssistantMessage),
    /// 文本增量
    TextDelta { index: usize, delta: String },
    /// 工具调用参数增量（累积中的 JSON 字符串）
    ToolCallDelta { index: usize, delta: String },
    /// 一个工具调用块完整结束
    ToolCallEnd { index: usize, tool_call: ToolCall },
    /// 流正常结束，携带完整的 AssistantMessage
    Done(AssistantMessage),
    /// 流出错，携带带有 error_message 的 AssistantMessage
    Error(AssistantMessage),
}
