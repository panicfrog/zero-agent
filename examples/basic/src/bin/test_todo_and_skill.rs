/// Example: 测试 todo + skill 结合使用
use anyhow::Result;
use std::{path::PathBuf, sync::Arc};
use tokio::sync::mpsc;
use zero_agent::{AgentContextBuilder, AgentEvent, JsonValidateTool, Skill, agent_run};
use basic::devpilot_model;

#[tokio::main]
async fn main() -> Result<()> {
    let model = devpilot_model();

    let skills_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("skills");

    let (defs, _) = Skill::load_dir(&skills_dir);
    let json_validate_tool = Arc::new(JsonValidateTool);

    let mut builder = AgentContextBuilder::new(
        "You are a capable assistant that can plan complex multi-step tasks using todo, \
         and execute each step using available skills. \
         For tasks with 3 or more steps, always create a todo plan first.",
        model,
    );

    for def in defs {
        let mut skill = def.skill;
        for tool_name in &def.allowed_tools {
            if tool_name == "json_validate" {
                skill = skill.with_tool(json_validate_tool.clone());
            }
        }
        builder = builder.skill(skill);
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match &event {
                AgentEvent::AgentStart { session_id } => {
                    println!("\n╔══ Agent Start [{}] ══╗", &session_id[..8]);
                }
                AgentEvent::TurnStart { session_id } => {
                    println!("├── Turn [{}]", &session_id[..8]);
                }
                AgentEvent::ToolExecutionStart { session_id, tool_name, args, .. } => {
                    let args_preview = serde_json::to_string(args)
                        .unwrap_or_default()
                        .chars()
                        .take(120)
                        .collect::<String>();
                    println!("│   >> {} [{}]: {}", tool_name, &session_id[..8], args_preview);
                }
                AgentEvent::ToolExecutionEnd { session_id, tool_name, result, .. } => {
                    let preview = result.content.lines().take(2).collect::<Vec<_>>().join(" | ");
                    let status = if result.is_error { "✗" } else { "✓" };
                    println!("│   {} {} [{}]: {}", status, tool_name, &session_id[..8], preview);
                }
                AgentEvent::AgentEnd { session_id, messages } => {
                    println!("╚══ Agent End [{}] ({} messages) ══╝", &session_id[..8], messages.len());
                }
                _ => {}
            }
        }
    });

    let ctx = builder.build();

    let prompt = r#"
I need a complete analysis report for a small business. Please handle the following 3 steps:

**Step 1 (Financial calculation):** Monthly revenues: $45,000, $52,000, $38,000 over last 3 months.
Calculate: total revenue, average monthly revenue, projected next month (5% growth).
Return as JSON: {"result": <projected>, "steps": [...], "explanation": "..."}
Validate with schema: {"type":"object","required":["result","steps","explanation"],"properties":{"result":{"type":"number"},"steps":{"type":"array","items":{"type":"string"}},"explanation":{"type":"string"}}}

**Step 2 (Review analysis):** Analyze this review:
"Our experience was fantastic! The staff was incredibly helpful and the product quality is top-notch.
We've been customers for 3 years and have always been satisfied. Highly recommend to everyone."
Return as JSON: {"word_count": <n>, "sentence_count": <n>, "sentiment": "positive", "key_topics": [...], "summary": "..."}
Validate with schema: {"type":"object","required":["word_count","sentence_count","sentiment","key_topics","summary"],"properties":{"word_count":{"type":"number"},"sentence_count":{"type":"number"},"sentiment":{"type":"string"},"key_topics":{"type":"array","items":{"type":"string"}},"summary":{"type":"string"}}}

**Step 3 (Summary):** Based on the financial data and review sentiment, write a brief 2-3 sentence business health assessment.

Use todo to plan these 3 steps first, then execute each one in order.
"#;

    println!("=== Test: Todo + Skill Combined ===");
    println!("Prompt: {}\n", prompt.trim());

    let result = agent_run(ctx, prompt, Some(tx)).await?;

    println!("\n========== Final Output ==========");
    println!("{}", result);

    Ok(())
}
