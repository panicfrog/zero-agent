use tokio::sync::mpsc;
use zero_agent::{AgentContextBuilder, AgentEvent, agent_run};
use zero_ai::types::Model;

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

    let ctx = AgentContextBuilder::new(
        "You are a helpful assistant. Answer concisely in the same language as the user.",
        model,
    )
    .build();

    let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();

    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::MessageDelta {
                    event: zero_ai::types::StreamEvent::TextDelta { delta, .. },
                } => {
                    print!("{delta}");
                    use std::io::Write;
                    std::io::stdout().flush().ok();
                }
                AgentEvent::MessageEnd { message } => {
                    println!();
                    println!(
                        "[usage] in={} out={}",
                        message.usage.input_tokens, message.usage.output_tokens
                    );
                }
                _ => {}
            }
        }
    });

    let result = agent_run(ctx, "用一句话介绍一下你自己", Some(tx)).await?;
    println!("\n[final] {result}");

    Ok(())
}
