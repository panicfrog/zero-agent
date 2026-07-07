pub mod tool;
pub mod prompts;
pub mod skill;
pub mod subagent;
pub mod context;
pub mod event;
pub mod loop_;
pub mod tools;

/// 全局子 agent 序列号，跨 spawn_subagent / parallel_subagent 共享，保证唯一。
pub(crate) static SUBAGENT_SEQ: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(1);

pub use tool::{Tool, ToolResult};
pub use skill::{Skill, SkillDef, SkillRegistry};
pub use subagent::{SubAgentDef, SubAgentRegistry};
pub use context::{AgentContext, AgentContextBuilder, ToolExecutionMode, BeforeToolCallHook, AfterToolCallHook};
pub use event::AgentEvent;
pub use loop_::agent_run;
pub use tools::json_validate::JsonValidateTool;
pub use tools::todo::TodoTool;
