/// 测试从目录加载 llm-ops skills 并运行
use std::sync::Arc;
use tokio::sync::mpsc;
use zero_agent::{AgentContextBuilder, AgentEvent, JsonValidateTool, Skill, agent_run};
use zero_ai::types::Model;

fn resolve_tool(name: &str) -> Option<Arc<dyn zero_agent::Tool>> {
    match name {
        "json_validate" => Some(Arc::new(JsonValidateTool)),
        _ => None,
    }
}

fn load_skill(skills_dir: &str, skill_name: &str) -> Option<Skill> {
    let (defs, _) = Skill::load_dir(skills_dir);
    for def in defs {
        if def.skill.name == skill_name {
            let mut skill = def.skill;
            for tool_name in &def.allowed_tools {
                if let Some(t) = resolve_tool(tool_name) {
                    skill = skill.with_tool(t);
                }
            }
            return Some(skill);
        }
    }
    None
}

fn build_model(api_key: &str) -> Model {
    Model {
        id: "glm-5-1".to_string(),
        provider: zero_ai::types::Provider::Anthropic,
        api_key: api_key.to_string(),
        base_url: Some(
            "http://devpilot.zhonganonline.com/devpilot/v1/external/direct/cline/v1/messages"
                .to_string(),
        ),
        max_tokens: 2048,
    }
}

async fn run_case(label: &str, skill: Skill, user_input: &str, api_key: &str) {
    println!("\n========== {} ==========", label);
    println!("用户: {}", user_input);

    let ctx = AgentContextBuilder::new(
        "你是一个意图识别助手，根据用户输入使用对应的 skill 提取参数并输出结构化 JSON。",
        build_model(api_key),
    )
    .skill(skill)
    .build();

    let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();

    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::ToolExecutionStart { tool_name, args, .. } => {
                    println!("[call] {} args={}", tool_name, args);
                }
                AgentEvent::ToolExecutionEnd { tool_name, result, .. } => {
                    println!("[result] {} => {}", tool_name, result.content);
                }
                AgentEvent::MessageDelta {
                    event: zero_ai::types::StreamEvent::TextDelta { delta, .. },
                    ..
                } => {
                    print!("{delta}");
                    use std::io::Write;
                    std::io::stdout().flush().ok();
                }
                AgentEvent::MessageEnd { message , ..} => {
                    println!();
                    println!("[usage] in={} out={}", message.usage.input_tokens, message.usage.output_tokens);
                }
                _ => {}
            }
        }
    });

    match agent_run(ctx, user_input, Some(tx)).await {
        Ok(result) => println!("[output] {}", result),
        Err(e) => println!("[error] {}", e),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let api_key = std::env::var("DEVPILOT_API_KEY").unwrap_or_else(|_| "dummy".to_string());
    let skills_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/skills");

    // 打印已加载的 skill 列表
    let (defs, diagnostics) = Skill::load_dir(skills_dir);
    for warn in &diagnostics {
        eprintln!("[warn] {warn}");
    }
    println!("已加载 skill ({} 个)：", defs.len());
    for def in &defs {
        println!("  - {} | allowed-tools: {:?}", def.skill.name, def.allowed_tools);
    }

    // 测试用例：(label, skill_name, user_input)
    let cases = vec![
        ("Android 打包 - 参数完整",    "app-build-android-packaging-api", "帮我打包 Android Staging 环境，分支 feature/login-v2，sit 环境"),
        ("Android 打包 - 缺少环境",    "app-build-android-packaging-api", "帮我打包 Android feature/login-v2 分支"),
        ("iOS 打包 - 参数完整",        "app-build-ios-packaging-api",     "打包 Release 版本，master 分支，同时发 adhoc 和 adhoc2，uat 环境"),
        ("iOS 打包 - 缺少分发渠道",    "app-build-ios-packaging-api",     "帮我打包 sit 环境的 feature/test 分支"),
        ("开关查询 - 正常",            "app-switch-query-api",            "查一个开关 ZAAppV3LiquidButton"),
        ("开关查询 - 缺少开关名",      "app-switch-query-api",            "帮我查一下开关"),
        ("多仓库建分支 - 有默认模式",  "multi-repo-feature-branch-api",   "从 develop 拉一个 zeroclaw_develop_test 分支"),
        ("工单转发 - 参数完整",        "ticket-forwarding-api",           "把工单 T20250317001 转给 bob@za.com"),
        ("工单转发 - 邮箱格式错误",    "ticket-forwarding-api",           "把工单 T20250317001 转给 bob"),
    ];

    for (label, skill_name, user_input) in cases {
        match load_skill(skills_dir, skill_name) {
            Some(skill) => run_case(label, skill, user_input, &api_key).await,
            None => println!("\n[skip] skill not found: {skill_name}"),
        }
    }

    Ok(())
}
