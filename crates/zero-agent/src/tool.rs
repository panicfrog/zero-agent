use async_trait::async_trait;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
}

impl ToolResult {
    pub fn ok(content: impl Into<String>) -> Self {
        ToolResult { content: content.into(), is_error: false }
    }

    pub fn err(content: impl Into<String>) -> Self {
        ToolResult { content: content.into(), is_error: true }
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    /// 返回 JSON Schema object，描述工具参数
    fn parameters_schema(&self) -> Value;

    async fn execute(
        &self,
        tool_call_id: &str,
        args: Value,
    ) -> ToolResult;

    /// 标记该工具会派生子 agent。
    /// 子 agent 加载 skill 时会过滤掉此类工具，防止递归。
    fn is_agent_spawner(&self) -> bool { false }

    /// 将本工具转换为发给 LLM 的 ToolSpec
    fn to_spec(&self) -> zero_ai::types::ToolSpec {
        zero_ai::types::ToolSpec {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters_schema(),
        }
    }
}
