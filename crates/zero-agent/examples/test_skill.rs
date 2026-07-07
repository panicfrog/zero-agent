/// 测试 Skill 加载与执行
///
/// 场景：父 agent 注册两个 skill，然后让 LLM 决定是否委派子 agent。
/// - math_skill：提供计算器工具
/// - greeting_skill：提供问候工具（纯 instructions，无 tool）
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

    // Skill 1：数学计算能力
    let math_skill = Skill::new(
        "math_assistant",
        "Provides arithmetic calculation capability using the calculator tool.",
    )
    .with_instruction(
        "You have access to a `calculator` tool. \
         Always use it for numerical computation instead of computing mentally.",
    )
    .with_tool(Arc::new(CalculatorTool));

    // Skill 2：纯 instructions skill（无工具，只注入 prompt）
    let greeting_skill = Skill::new(
        "greeting_expert",
        "Knows how to write warm, culturally appropriate greetings.",
    )
    .with_instruction(
        "When writing greetings, always include the person's name, \
         a warm opening, and a specific compliment.",
    );

    // 构建父 agent，注册两个 skill
    let ctx = AgentContextBuilder::new(
        "You are a helpful coordinator. \
         For tasks requiring calculation or greeting writing, \
         delegate to a sub-agent with the appropriate skill.",
        model,
    )
    .skill(math_skill)
    .skill(greeting_skill)
    .build();

    // 打印已注册工具（应包含 calculator + spawn_subagent，主 agent 直接可用）
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
                    let preview = result.content.char_indices().nth(120).map(|(i, _)| &result.content[..i]).unwrap_or(&result.content);
                    println!("[result] {} => {}", tool_name, preview);
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

    // 测试：让主 agent 直接调用 calculator 工具完成，不走子 agent
    let result = agent_run(
        ctx,
        "请直接使用 calculator 工具帮我计算 (123 * 456) + (789 / 3) 的结果，不要委派给子 agent。",
        Some(tx),
    )
    .await?;

    println!("\n[final] {result}");
    Ok(())
}
