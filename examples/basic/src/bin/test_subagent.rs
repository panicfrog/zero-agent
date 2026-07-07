/// Example: 测试 sub-agent 调用流程
///
/// 场景：主 agent 注册两个子 agent：
///   - researcher: 负责调研某个话题，返回摘要
///   - analyst: 负责对数据做分析，返回结论
///
/// 测试两个路径：
///   1. spawn_subagent（同步，单个）
///   2. parallel_subagent（并发，多个）

use anyhow::Result;
use tokio::sync::mpsc;
use zero_agent::{AgentContextBuilder, AgentEvent, SubAgentDef, agent_run};
use basic::devpilot_model;

#[tokio::main]
async fn main() -> Result<()> {
    let model = devpilot_model();

    // 定义子 agent：researcher
    let researcher = SubAgentDef::new(
        "researcher",
        "Researches a given topic and returns a concise factual summary.",
        "You are a research specialist. Given a topic, provide a concise, factual summary \
         covering key points, important facts, and relevant context. Keep it under 200 words.",
    );

    // 定义子 agent：analyst
    let analyst = SubAgentDef::new(
        "analyst",
        "Analyzes provided data or information and returns structured conclusions.",
        "You are an analytical specialist. Given information or data, identify patterns, \
         draw conclusions, and provide actionable insights. Structure your response clearly \
         with key findings and recommendations.",
    );

    let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match &event {
                AgentEvent::AgentStart { session_id } => {
                    println!("\n[{}] === Agent Start ===", &session_id[..8]);
                }
                AgentEvent::TurnStart { session_id } => {
                    println!("[{}] --- Turn ---", &session_id[..8]);
                }
                AgentEvent::ToolExecutionStart { session_id, tool_name, args, .. } => {
                    let args_preview = serde_json::to_string(args)
                        .unwrap_or_default()
                        .chars().take(120).collect::<String>();
                    println!("[{}] >> {}: {}", &session_id[..8], tool_name, args_preview);
                }
                AgentEvent::ToolExecutionEnd { session_id, tool_name, result, .. } => {
                    let preview = result.content.lines().take(2).collect::<Vec<_>>().join(" | ");
                    let status = if result.is_error { "✗" } else { "✓" };
                    // session_id 前缀可区分是主 agent 还是子 agent 的事件
                    println!("[{}] {} {}: {}", &session_id[..8], status, tool_name, preview);
                }
                AgentEvent::AgentEnd { session_id, messages } => {
                    println!("[{}] === Agent End ({} messages) ===\n", &session_id[..8], messages.len());
                }
                _ => {}
            }
        }
    });

    let ctx = AgentContextBuilder::new(
        "You are an orchestration agent. Use your registered sub-agents to handle specialized tasks.",
        model,
    )
    .sub_agent(researcher)
    .sub_agent(analyst)
    .build();

    // 测试 parallel_subagent：同时调研两个话题，然后主 agent 汇总
    let prompt = r#"
I need to understand the current state of renewable energy adoption globally.

Please:
1. Have the researcher look into "solar energy growth trends 2020-2024"
2. Have the researcher look into "wind energy growth trends 2020-2024"
3. Once both research results are ready, have the analyst synthesize them into a comparison and outlook

Use parallel sub-agents where possible to save time.
"#;

    println!("=== Test: Sub-Agent (spawn + parallel) ===");
    println!("Prompt: {}\n", prompt.trim());

    let result = agent_run(ctx, prompt, Some(tx)).await?;

    println!("\n========== Final Output ==========");
    println!("{}", result);

    Ok(())
}
