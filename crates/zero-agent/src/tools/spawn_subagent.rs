use crate::{
    context::{AfterToolCallHook, AgentContextBuilder, BeforeToolCallHook},
    loop_::agent_run,
    subagent::SubAgentRegistry,
    tool::{Tool, ToolResult},
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use zero_ai::types::Model;

/// 内置工具：按名字派发单个已定义的子 agent，同步等待结果。
/// 子 agent 在独立上下文中运行，过程对主 agent 不可见，只有最终输出返回。
pub struct SpawnSubAgentTool {
    registry: Arc<SubAgentRegistry>,
    model: Model,
    parent_session_id: String,
    before_tool_call: Option<Arc<dyn BeforeToolCallHook>>,
    after_tool_call: Option<Arc<dyn AfterToolCallHook>>,
}

impl SpawnSubAgentTool {
    pub fn new(
        registry: Arc<SubAgentRegistry>,
        model: Model,
        parent_session_id: String,
        before_tool_call: Option<Arc<dyn BeforeToolCallHook>>,
        after_tool_call: Option<Arc<dyn AfterToolCallHook>>,
    ) -> Self {
        SpawnSubAgentTool { registry, model, parent_session_id, before_tool_call, after_tool_call }
    }
}

#[async_trait]
impl Tool for SpawnSubAgentTool {
    fn name(&self) -> &str { "spawn_subagent" }

    fn is_agent_spawner(&self) -> bool { true }

    fn description(&self) -> &str {
        "Spawn a pre-defined sub-agent to handle an isolated task. \
         The sub-agent runs in its own context — only the final result is returned. \
         Use this for tasks that require isolation or a specialized agent."
    }

    fn parameters_schema(&self) -> Value {
        let available = if self.registry.agents.is_empty() {
            "  (no sub-agents registered)".to_string()
        } else {
            self.registry.descriptions().join("\n")
        };

        serde_json::json!({
            "type": "object",
            "properties": {
                "agent_name": {
                    "type": "string",
                    "description": format!(
                        "Name of the sub-agent to spawn.\nAvailable sub-agents:\n{}", available
                    )
                },
                "prompt": {
                    "type": "string",
                    "description": "The task prompt to send to the sub-agent."
                }
            },
            "required": ["agent_name", "prompt"]
        })
    }

    async fn execute(&self, _id: &str, args: Value) -> ToolResult {
        let agent_name = match args["agent_name"].as_str() {
            Some(n) => n,
            None => return ToolResult::err("missing required argument: agent_name"),
        };
        let prompt = match args["prompt"].as_str() {
            Some(p) => p.to_string(),
            None => return ToolResult::err("missing required argument: prompt"),
        };

        let def = match self.registry.get(agent_name) {
            Some(d) => d,
            None => return ToolResult::err(format!(
                "unknown sub-agent: '{}'. Available: [{}]",
                agent_name,
                self.registry.names().join(", ")
            )),
        };

        // 子 agent session_id = "{parent}/{agent_name}-{seq}"，seq 全局递增保证唯一
        let seq = crate::SUBAGENT_SEQ.fetch_add(1, Ordering::Relaxed);
        let sub_session_id = format!("{}/{}-{}", self.parent_session_id, agent_name, seq);

        let mut builder = AgentContextBuilder::new(def.system_prompt.clone(), self.model.clone())
            .session_id(sub_session_id)
            .is_subagent(true);

        for skill in &def.skills {
            builder = builder.skill_arc(Arc::clone(skill));
        }
        for tool in &def.extra_tools {
            builder = builder.tool(Arc::clone(tool));
        }
        if let Some(hook) = &self.before_tool_call {
            builder = builder.before_tool_call(hook.clone());
        }
        if let Some(hook) = &self.after_tool_call {
            builder = builder.after_tool_call(hook.clone());
        }

        let sub_ctx = builder.build();

        match agent_run(sub_ctx, prompt, None).await {
            Ok(output) => ToolResult::ok(output),
            Err(e) => ToolResult::err(format!("sub-agent '{}' error: {e}", agent_name)),
        }
    }
}
