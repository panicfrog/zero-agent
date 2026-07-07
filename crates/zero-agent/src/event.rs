use crate::tool::ToolResult;
use zero_ai::types::{AssistantMessage, Message, StreamEvent, ToolResultMessage};

#[derive(Debug, Clone)]
pub enum AgentEvent {
    AgentStart {
        session_id: String,
    },
    AgentEnd {
        session_id: String,
        messages: Vec<Message>,
    },
    TurnStart {
        session_id: String,
    },
    TurnEnd {
        session_id: String,
        message: AssistantMessage,
        tool_results: Vec<ToolResultMessage>,
    },
    /// LLM 开始输出（含初始空 AssistantMessage）
    MessageStart {
        session_id: String,
        message: AssistantMessage,
    },
    /// LLM 流式增量事件
    MessageDelta {
        session_id: String,
        event: StreamEvent,
    },
    /// LLM 一轮输出完成
    MessageEnd {
        session_id: String,
        message: AssistantMessage,
    },
    ToolExecutionStart {
        session_id: String,
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
    },
    ToolExecutionEnd {
        session_id: String,
        tool_call_id: String,
        tool_name: String,
        result: ToolResult,
    },
}

impl AgentEvent {
    pub fn session_id(&self) -> &str {
        match self {
            AgentEvent::AgentStart { session_id } => session_id,
            AgentEvent::AgentEnd { session_id, .. } => session_id,
            AgentEvent::TurnStart { session_id } => session_id,
            AgentEvent::TurnEnd { session_id, .. } => session_id,
            AgentEvent::MessageStart { session_id, .. } => session_id,
            AgentEvent::MessageDelta { session_id, .. } => session_id,
            AgentEvent::MessageEnd { session_id, .. } => session_id,
            AgentEvent::ToolExecutionStart { session_id, .. } => session_id,
            AgentEvent::ToolExecutionEnd { session_id, .. } => session_id,
        }
    }
}
