/// 测试从 markdown 文件加载 Skill
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;
use zero_agent::{AgentContextBuilder, AgentEvent, Skill, Tool, ToolResult, agent_run};
use zero_ai::types::Model;

// ---------------------------------------------------------------------------
// Tool 实现：简单计算器
// ---------------------------------------------------------------------------

struct CalculatorTool;

#[async_trait]
impl Tool for CalculatorTool {
    fn name(&self) -> &str { "calculator" }

    fn description(&self) -> &str {
        "Perform basic arithmetic: add, subtract, multiply, divide."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["add", "subtract", "multiply", "divide"]
                },
                "a": { "type": "number" },
                "b": { "type": "number" }
            },
            "required": ["operation", "a", "b"]
        })
    }

    async fn execute(&self, _id: &str, args: Value) -> ToolResult {
        let a = match args["a"].as_f64() {
            Some(v) => v,
            None => return ToolResult::err("missing: a"),
        };
        let b = match args["b"].as_f64() {
            Some(v) => v,
            None => return ToolResult::err("missing: b"),
        };
        let result = match args["operation"].as_str().unwrap_or("") {
            "add"      => a + b,
            "subtract" => a - b,
            "multiply" => a * b,
            "divide"   => {
                if b == 0.0 { return ToolResult::err("division by zero"); }
                a / b
            }
            op => return ToolResult::err(format!("unknown op: {op}")),
        };
        ToolResult::ok(format!("{a} {op} {b} = {result}", op = args["operation"].as_str().unwrap_or("")))
    }
}

// ---------------------------------------------------------------------------
// 工具注册表：按名字查找工具
// ---------------------------------------------------------------------------

fn resolve_tool(name: &str) -> Option<Arc<dyn Tool>> {
    match name {
        "calculator" => Some(Arc::new(CalculatorTool)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let api_key = std::env::var("DEVPILOT_API_KEY").unwrap_or_else(|_| "dummy".to_string());

    let model = Model {
        id: "glm-5-1".to_string(),
        provider: zero_ai::types::Provider::Anthropic,
        api_key,
        base_url: Some(
            "http://devpilot.zhonganonline.com/devpilot/v1/external/direct/cline/v1/messages"
                .to_string(),
        ),
        max_tokens: 2048,
    };

    // 从 skills 目录加载所有 skill（每个子目录含 SKILL.md）
    let skills_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/skills");
    let (defs, diagnostics) = Skill::load_dir(skills_dir);

    for warn in &diagnostics {
        eprintln!("[warn] {warn}");
    }

    let mut builder = AgentContextBuilder::new("You are a helpful assistant.", model);

    for def in defs {
        println!("[skill] name={}", def.skill.name);
        println!("[skill] description={}", def.skill.description);
        println!("[skill] allowed-tools={:?}", def.allowed_tools);
        println!("[skill] instructions={:?}", def.skill.instructions);

        // 根据 allowed_tools 绑定工具
        let mut skill = def.skill;
        for tool_name in &def.allowed_tools {
            match resolve_tool(tool_name) {
                Some(t) => {
                    println!("[bind] tool bound: {tool_name}");
                    skill = skill.with_tool(t);
                }
                None => println!("[bind] tool not found: {tool_name} (skipped)"),
            }
        }
        println!();
        builder = builder.skill(skill);
    }

    let ctx = builder.build();

    println!("[tools] {:?}\n", ctx.tools.iter().map(|t| t.name()).collect::<Vec<_>>());

    let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();

    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::TurnStart => println!("\n--- turn ---"),
                AgentEvent::ToolExecutionStart { tool_name, args, .. } => {
                    println!("[call] {} args={}", tool_name, args);
                }
                AgentEvent::ToolExecutionEnd { tool_name, result, .. } => {
                    println!("[result] {} => {}", tool_name, result.content);
                }
                AgentEvent::MessageDelta {
                    event: zero_ai::types::StreamEvent::TextDelta { delta, .. },
                } => {
                    print!("{delta}");
                    use std::io::Write;
                    std::io::stdout().flush().ok();
                }
                AgentEvent::MessageEnd { message } => {
                    println!();
                    println!("[usage] in={} out={}", message.usage.input_tokens, message.usage.output_tokens);
                }
                _ => {}
            }
        }
    });

    let result = agent_run(
        ctx,
        "请用计算器工具计算 (99 * 88) - (77 / 7) 的结果。",
        Some(tx),
    )
    .await?;

    println!("\n[final] {result}");
    Ok(())
}
