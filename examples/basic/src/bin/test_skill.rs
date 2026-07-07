/// Example: 测试 skill 加载和调用流程
use anyhow::Result;
use std::{path::PathBuf, sync::Arc};
use tokio::sync::mpsc;
use zero_agent::{AgentContextBuilder, AgentEvent, JsonValidateTool, Skill, agent_run};
use basic::devpilot_model;

#[tokio::main]
async fn main() -> Result<()> {
    let model = devpilot_model();

    // 加载 skills 目录
    let skills_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("skills");

    println!("Loading skills from: {}", skills_dir.display());

    let (defs, diagnostics) = Skill::load_dir(&skills_dir);
    if !diagnostics.is_empty() {
        eprintln!("Skill load warnings:");
        for d in &diagnostics {
            eprintln!("  - {}", d);
        }
    }

    println!("Loaded {} skill(s):", defs.len());
    for d in &defs {
        println!("  - {} : {}", d.skill.name, d.skill.description);
    }
    println!();

    let json_validate_tool = Arc::new(JsonValidateTool);

    let mut builder = AgentContextBuilder::new(
        "You are an intelligent assistant with specialized skills. \
         Use the appropriate skill's tools and follow its instructions when handling tasks. \
         Always validate structured JSON output with json_validate before returning.",
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
                    println!("[{}] === Agent Start ===", &session_id[..8]);
                }
                AgentEvent::TurnStart { session_id } => {
                    println!("[{}] --- Turn ---", &session_id[..8]);
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
                    let preview = if result.content.len() > 200 {
                        format!("{}...", &result.content[..200])
                    } else {
                        result.content.clone()
                    };
                    let status = if result.is_error { "✗" } else { "✓" };
                    println!("[{}] {} {}: {}", &session_id[..8], status, tool_name, preview);
                }
                AgentEvent::AgentEnd { session_id, .. } => {
                    println!("[{}] === Agent End ===", &session_id[..8]);
                }
                _ => {}
            }
        }
    });

    let ctx = builder.build();

    let prompt = r#"
I have two tasks for you:

**Task 1 (Math):** Calculate the total cost of a shopping cart:
- 3 apples at $0.75 each
- 2 books at $12.50 each
- 1 backpack at $34.99

Apply a 10% discount on the total, then add 8% sales tax.
Return the result as JSON: {"result": <final amount>, "steps": [...], "explanation": "..."}
Validate with json_validate using schema: {"type":"object","properties":{"result":{"type":"number"},"steps":{"type":"array"},"explanation":{"type":"string"}},"required":["result","steps","explanation"]}

**Task 2 (Text Analysis):** Analyze this text:
"The new product launch exceeded all expectations. Customer feedback has been overwhelmingly positive,
with many users praising the intuitive design and fast performance. Sales figures for the first week
surpassed our targets by 40%, making this our most successful launch to date."

Return as JSON: {"word_count": <n>, "sentence_count": <n>, "sentiment": "positive"|"neutral"|"negative", "key_topics": [...], "summary": "..."}
Validate with json_validate using schema: {"type":"object","properties":{"word_count":{"type":"number"},"sentence_count":{"type":"number"},"sentiment":{"type":"string"},"key_topics":{"type":"array"},"summary":{"type":"string"}},"required":["word_count","sentence_count","sentiment","key_topics","summary"]}
"#;

    println!("=== Test: Skill Usage ===");
    println!("Prompt: {}\n", prompt.trim());

    let result = agent_run(ctx, prompt, Some(tx)).await?;

    println!("\n========== Final Output ==========");
    println!("{}", result);

    Ok(())
}
