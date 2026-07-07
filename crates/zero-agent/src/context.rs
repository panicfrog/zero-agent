use crate::{
    prompts::{MAIN_AGENT_SYSTEM_PROMPT, SUB_AGENT_SYSTEM_PROMPT},
    skill::Skill,
    subagent::{SubAgentDef, SubAgentRegistry},
    tool::Tool,
    tools::{
        json_validate::JsonValidateTool,
        parallel_subagent::ParallelSubAgentTool,
        spawn_subagent::SpawnSubAgentTool,
        todo::TodoTool,
    },
};
use async_trait::async_trait;
use std::sync::Arc;
use zero_ai::types::Model;

// ---------------------------------------------------------------------------
// Hook traits
// ---------------------------------------------------------------------------

#[async_trait]
pub trait BeforeToolCallHook: Send + Sync {
    /// 返回 Some(reason) 表示阻断执行
    async fn before_tool_call(&self, tool_name: &str, args: &serde_json::Value) -> Option<String>;
}

#[async_trait]
pub trait AfterToolCallHook: Send + Sync {
    async fn after_tool_call(
        &self,
        tool_name: &str,
        result: crate::tool::ToolResult,
    ) -> crate::tool::ToolResult;
}

// ---------------------------------------------------------------------------
// ToolExecutionMode
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub enum ToolExecutionMode {
    #[default]
    Parallel,
    Sequential,
}

// ---------------------------------------------------------------------------
// AgentContext
// ---------------------------------------------------------------------------

pub struct AgentContext {
    pub session_id: String,
    pub system_prompt: String,
    pub messages: Vec<zero_ai::types::Message>,
    pub tools: Vec<Arc<dyn Tool>>,
    pub model: Model,
    pub is_subagent: bool,
    pub max_iterations: usize,
    pub tool_execution: ToolExecutionMode,
    pub before_tool_call: Option<Arc<dyn BeforeToolCallHook>>,
    pub after_tool_call: Option<Arc<dyn AfterToolCallHook>>,
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

pub struct AgentContextBuilder {
    session_id: Option<String>,
    base_system_prompt: String,
    model: Model,
    skills: Vec<Arc<Skill>>,
    sub_agents: Vec<Arc<SubAgentDef>>,
    extra_tools: Vec<Arc<dyn Tool>>,
    is_subagent: bool,
    max_iterations: usize,
    tool_execution: ToolExecutionMode,
    before_tool_call: Option<Arc<dyn BeforeToolCallHook>>,
    after_tool_call: Option<Arc<dyn AfterToolCallHook>>,
}

impl AgentContextBuilder {
    pub fn new(base_system_prompt: impl Into<String>, model: Model) -> Self {
        AgentContextBuilder {
            session_id: None,
            base_system_prompt: base_system_prompt.into(),
            model,
            skills: Vec::new(),
            sub_agents: Vec::new(),
            extra_tools: Vec::new(),
            is_subagent: false,
            max_iterations: 50,
            tool_execution: ToolExecutionMode::Parallel,
            before_tool_call: None,
            after_tool_call: None,
        }
    }

    pub fn session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    pub fn skill(mut self, skill: Skill) -> Self {
        self.skills.push(Arc::new(skill));
        self
    }

    pub fn skill_arc(mut self, skill: Arc<Skill>) -> Self {
        self.skills.push(skill);
        self
    }

    pub fn sub_agent(mut self, def: SubAgentDef) -> Self {
        self.sub_agents.push(Arc::new(def));
        self
    }

    pub fn sub_agent_arc(mut self, def: Arc<SubAgentDef>) -> Self {
        self.sub_agents.push(def);
        self
    }

    pub fn tool(mut self, tool: Arc<dyn Tool>) -> Self {
        self.extra_tools.push(tool);
        self
    }

    pub fn is_subagent(mut self, v: bool) -> Self {
        self.is_subagent = v;
        self
    }

    pub fn max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = n;
        self
    }

    pub fn tool_execution(mut self, mode: ToolExecutionMode) -> Self {
        self.tool_execution = mode;
        self
    }

    pub fn before_tool_call(mut self, hook: Arc<dyn BeforeToolCallHook>) -> Self {
        self.before_tool_call = Some(hook);
        self
    }

    pub fn after_tool_call(mut self, hook: Arc<dyn AfterToolCallHook>) -> Self {
        self.after_tool_call = Some(hook);
        self
    }

    pub fn build(self) -> AgentContext {
        let session_id = self.session_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        // 内置 tool：todo + json_validate，所有 agent（主+子）默认集成
        let mut tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(TodoTool::new()),
            Arc::new(JsonValidateTool),
        ];
        tools.extend(self.extra_tools);

        // 处理 skill：注入 instructions + 注册 tools
        let mut skill_sections = String::new();
        for skill in &self.skills {
            let section = skill.to_instructions_section();
            if !section.is_empty() {
                skill_sections.push_str(&section);
            }
            // 子 agent 过滤掉 agent spawner 类工具，防止递归
            tools.extend(
                skill.tools.iter()
                    .filter(|t| !self.is_subagent || !t.is_agent_spawner())
                    .cloned()
            );
        }

        // 构建 SubAgentRegistry
        let mut registry = SubAgentRegistry::new();
        for def in &self.sub_agents {
            registry.register_arc(Arc::clone(def));
        }
        let registry_arc = Arc::new(registry);

        // 生成 XML 标签：注册的 skill 列表（粗粒度：name + description）
        let skills_xml = if self.skills.is_empty() {
            String::new()
        } else {
            let mut lines = vec![
                "The following skills provide specialized instructions and tools for specific tasks.".to_string(),
                String::new(),
                "<available_skills>".to_string(),
            ];
            for skill in &self.skills {
                lines.push("  <skill>".to_string());
                lines.push(format!("    <name>{}</name>", escape_xml(&skill.name)));
                lines.push(format!("    <description>{}</description>", escape_xml(&skill.description)));
                lines.push("  </skill>".to_string());
            }
            lines.push("</available_skills>".to_string());
            lines.join("\n")
        };

        // 生成 XML 标签：注册的 sub_agent 列表（粗粒度：name + description）
        let subagents_xml = if self.sub_agents.is_empty() || self.is_subagent {
            String::new()
        } else {
            let mut lines = vec![
                "The following sub-agents are available for delegation. Use `spawn_subagent` or `parallel_subagent` to dispatch them.".to_string(),
                String::new(),
                "<available_sub_agents>".to_string(),
            ];
            for def in &self.sub_agents {
                lines.push("  <sub_agent>".to_string());
                lines.push(format!("    <name>{}</name>", escape_xml(&def.name)));
                lines.push(format!("    <description>{}</description>", escape_xml(&def.description)));
                lines.push("  </sub_agent>".to_string());
            }
            lines.push("</available_sub_agents>".to_string());
            lines.join("\n")
        };

        // system prompt 拼接顺序（静态在前，有利于缓存命中）：
        // 1. 框架内置 prompt
        // 2. available_skills XML（静态，取决于注册的 skill）
        // 3. available_sub_agents XML（静态，取决于注册的 sub_agent）
        // 4. skill instructions（静态）
        // 5. 用户 system prompt（动态）
        let builtin = if self.is_subagent { SUB_AGENT_SYSTEM_PROMPT } else { MAIN_AGENT_SYSTEM_PROMPT };
        let system_prompt = {
            let mut parts: Vec<&str> = vec![builtin];
            if !skills_xml.is_empty() { parts.push(&skills_xml); }
            if !subagents_xml.is_empty() { parts.push(&subagents_xml); }
            let skill_str;
            if !skill_sections.is_empty() {
                skill_str = skill_sections;
                parts.push(skill_str.trim_start());
            }
            if !self.base_system_prompt.is_empty() {
                parts.push(&self.base_system_prompt);
            }
            parts.join("\n\n")
        };

        // spawn_subagent + parallel_subagent 仅父 agent 注册（depth-1 cap）
        if !self.is_subagent {
            tools.push(Arc::new(SpawnSubAgentTool::new(
                Arc::clone(&registry_arc),
                self.model.clone(),
                session_id.clone(),
                self.before_tool_call.clone(),
                self.after_tool_call.clone(),
            )));
            tools.push(Arc::new(ParallelSubAgentTool::new(
                registry_arc,
                self.model.clone(),
                session_id.clone(),
                self.before_tool_call.clone(),
                self.after_tool_call.clone(),
            )));
        }

        AgentContext {
            session_id,
            system_prompt,
            messages: Vec::new(),
            tools,
            model: self.model,
            is_subagent: self.is_subagent,
            max_iterations: self.max_iterations,
            tool_execution: self.tool_execution,
            before_tool_call: self.before_tool_call,
            after_tool_call: self.after_tool_call,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
