use crate::{skill::Skill, tool::Tool};
use std::collections::HashMap;
use std::sync::Arc;

/// 子 agent 定义。预先声明子 agent 的能力（system prompt + skills + tools），
/// 运行时由 spawn_subagent / parallel_subagent 按名字取出并启动隔离执行。
pub struct SubAgentDef {
    pub name: String,
    pub description: String,
    /// 子 agent 专属 system prompt（会追加在框架内置 SUB_AGENT_SYSTEM_PROMPT 之后）
    pub system_prompt: String,
    /// 配备的 skill 列表
    pub skills: Vec<Arc<Skill>>,
    /// 额外的自定义 tool
    pub extra_tools: Vec<Arc<dyn Tool>>,
}

impl SubAgentDef {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        system_prompt: impl Into<String>,
    ) -> Self {
        SubAgentDef {
            name: name.into(),
            description: description.into(),
            system_prompt: system_prompt.into(),
            skills: Vec::new(),
            extra_tools: Vec::new(),
        }
    }

    pub fn with_skill(mut self, skill: Skill) -> Self {
        self.skills.push(Arc::new(skill));
        self
    }

    pub fn with_skill_arc(mut self, skill: Arc<Skill>) -> Self {
        self.skills.push(skill);
        self
    }

    pub fn with_tool(mut self, tool: Arc<dyn Tool>) -> Self {
        self.extra_tools.push(tool);
        self
    }
}

// ---------------------------------------------------------------------------
// SubAgentRegistry
// ---------------------------------------------------------------------------

/// 子 agent 注册表，供 spawn_subagent / parallel_subagent 按名字查找定义。
#[derive(Clone, Default)]
pub struct SubAgentRegistry {
    pub(crate) agents: HashMap<String, Arc<SubAgentDef>>,
}

impl SubAgentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, def: SubAgentDef) -> &mut Self {
        self.agents.insert(def.name.clone(), Arc::new(def));
        self
    }

    pub fn register_arc(&mut self, def: Arc<SubAgentDef>) -> &mut Self {
        self.agents.insert(def.name.clone(), def);
        self
    }

    pub fn get(&self, name: &str) -> Option<Arc<SubAgentDef>> {
        self.agents.get(name).cloned()
    }

    pub fn names(&self) -> Vec<&str> {
        self.agents.keys().map(String::as_str).collect()
    }

    pub fn descriptions(&self) -> Vec<String> {
        let mut entries: Vec<_> = self.agents.values().collect();
        entries.sort_by_key(|d| &d.name);
        entries.iter().map(|d| format!("  - {}: {}", d.name, d.description)).collect()
    }
}
