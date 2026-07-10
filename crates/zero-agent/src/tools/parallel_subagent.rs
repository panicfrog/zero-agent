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
use tokio::sync::Semaphore;
use zero_ai::types::Model;

const MAX_PARALLEL: usize = 4;

/// 内置工具：并发派发多个已定义的子 agent，等待全部完成后按原始顺序返回结果。
pub struct ParallelSubAgentTool {
    registry: Arc<SubAgentRegistry>,
    model: Model,
    parent_session_id: String,
    before_tool_call: Option<Arc<dyn BeforeToolCallHook>>,
    after_tool_call: Option<Arc<dyn AfterToolCallHook>>,
}

impl ParallelSubAgentTool {
    pub fn new(
        registry: Arc<SubAgentRegistry>,
        model: Model,
        parent_session_id: String,
        before_tool_call: Option<Arc<dyn BeforeToolCallHook>>,
        after_tool_call: Option<Arc<dyn AfterToolCallHook>>,
    ) -> Self {
        ParallelSubAgentTool { registry, model, parent_session_id, before_tool_call, after_tool_call }
    }
}

#[async_trait]
impl Tool for ParallelSubAgentTool {
    fn name(&self) -> &str { "parallel_subagent" }

    fn is_agent_spawner(&self) -> bool { true }

    fn description(&self) -> &str {
        "Spawn multiple pre-defined sub-agents in parallel. All tasks run concurrently; \
         results are returned in original order. Use when tasks are independent of each other."
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
                "tasks": {
                    "type": "array",
                    "description": "List of independent tasks to run in parallel.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "agent_name": {
                                "type": "string",
                                "description": format!(
                                    "Name of the sub-agent to spawn.\nAvailable:\n{}", available
                                )
                            },
                            "prompt": {
                                "type": "string",
                                "description": "The task prompt for this sub-agent."
                            }
                        },
                        "required": ["agent_name", "prompt"]
                    },
                    "minItems": 1,
                    "maxItems": 16
                }
            },
            "required": ["tasks"]
        })
    }

    async fn execute(&self, _id: &str, args: Value) -> ToolResult {
        let tasks = match args["tasks"].as_array() {
            Some(t) => t.clone(),
            None => return ToolResult::err("missing required argument: tasks"),
        };
        if tasks.is_empty() {
            return ToolResult::err("tasks array is empty");
        }

        let sem = Arc::new(Semaphore::new(MAX_PARALLEL));
        let mut set = tokio::task::JoinSet::new();

        for (idx, task) in tasks.iter().enumerate() {
            let agent_name = match task["agent_name"].as_str() {
                Some(n) => n.to_string(),
                None => return ToolResult::err(format!("tasks[{idx}] missing 'agent_name'")),
            };
            let prompt = match task["prompt"].as_str() {
                Some(p) => p.to_string(),
                None => return ToolResult::err(format!("tasks[{idx}] missing 'prompt'")),
            };

            let def = match self.registry.get(&agent_name) {
                Some(d) => d,
                None => return ToolResult::err(format!(
                    "tasks[{idx}] unknown sub-agent: '{}'. Available: [{}]",
                    agent_name,
                    self.registry.names().join(", ")
                )),
            };

            let sem = sem.clone();
            let model = self.model.clone();
            let before_hook = self.before_tool_call.clone();
            let after_hook = self.after_tool_call.clone();
            let seq = crate::SUBAGENT_SEQ.fetch_add(1, Ordering::Relaxed);
            let sub_session_id = format!("{}/{}-{}", self.parent_session_id, agent_name, seq);

            set.spawn(async move {
                let _permit = sem.acquire().await.unwrap();

                let mut builder = AgentContextBuilder::new(def.system_prompt.clone(), model)
                    .session_id(sub_session_id)
                    .is_subagent(true);
                for skill in &def.skills {
                    builder = builder.skill_arc(Arc::clone(skill));
                }
                for tool in &def.extra_tools {
                    builder = builder.tool(Arc::clone(tool));
                }
                if let Some(hook) = before_hook {
                    builder = builder.before_tool_call(hook);
                }
                if let Some(hook) = after_hook {
                    builder = builder.after_tool_call(hook);
                }

                let mut sub_ctx = builder.build();
                let result = agent_run(&mut sub_ctx, prompt, None).await;
                (idx, agent_name, result)
            });
        }

        let mut results: Vec<Option<Result<String, String>>> = vec![None; tasks.len()];
        while let Some(join_result) = set.join_next().await {
            if let Ok((idx, _name, result)) = join_result {
                results[idx] = Some(result.map_err(|e| e.to_string()));
            }
        }

        let output = results
            .into_iter()
            .enumerate()
            .map(|(i, r)| match r {
                Some(Ok(text)) => format!("## Task {} Result\n{}", i + 1, text),
                Some(Err(e)) => format!("## Task {} Error\n{}", i + 1, e),
                None => format!("## Task {} Error\ntask did not complete", i + 1),
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        ToolResult::ok(output)
    }
}
