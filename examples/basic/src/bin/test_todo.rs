/// Example: 测试 todo 工具的基本规划流程
use anyhow::Result;
use tokio::sync::mpsc;
use zero_agent::{AgentContextBuilder, AgentEvent, agent_run};
use basic::devpilot_model;

#[tokio::main]
async fn main() -> Result<()> {
    let model = devpilot_model();

    let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();

    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match &event {
                AgentEvent::AgentStart { session_id } => {
                    println!("\n[{}] === Agent Start ===", &session_id[..8]);
                }
                AgentEvent::TurnStart { session_id } => {
                    println!("\n[{}] --- Turn ---", &session_id[..8]);
                }
                AgentEvent::ToolExecutionStart { session_id, tool_name, args, .. } => {
                    println!(
                        "[{}] >> {}: {}",
                        &session_id[..8],
                        tool_name,
                        serde_json::to_string(args).unwrap_or_default()
                    );
                }
                AgentEvent::ToolExecutionEnd { session_id, tool_name, result, .. } => {
                    let preview = result.content.lines().take(3).collect::<Vec<_>>().join(" | ");
                    let status = if result.is_error { "✗" } else { "✓" };
                    println!("[{}] {} {}: {}", &session_id[..8], status, tool_name, preview);
                }
                AgentEvent::AgentEnd { session_id, messages } => {
                    println!("\n[{}] === Agent End ({} messages) ===", &session_id[..8], messages.len());
                }
                _ => {}
            }
        }
    });

    let ctx = AgentContextBuilder::new(
        "You are a helpful assistant. When tasks have 3 or more steps, always use the `todo` tool to plan and track them.",
        model,
    )
    .build();

    let prompt = r#"
Help me plan a birthday party. I need recommendations on the guest list, food & drinks menu, and decorations theme. Once all three are decided, create a full day timeline that incorporates all of them.
"#;

    println!("=== Test: Todo Planning ===");
    println!("Prompt: {}\n", prompt.trim());

    let result = agent_run(ctx, prompt, Some(tx)).await?;

    println!("\n========== Final Output ==========");
    println!("{}", result);

    Ok(())
}
