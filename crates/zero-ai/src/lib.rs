pub mod types;
pub mod provider;
pub mod providers;

pub use types::*;
pub use provider::{LlmProvider, StreamOptions};

use anyhow::Result;
use futures::Stream;
use std::pin::Pin;

pub type BoxStream<T> = Pin<Box<dyn Stream<Item = T> + Send + 'static>>;

/// 流式调用 LLM，返回事件流
pub async fn stream_llm(
    model: &types::Model,
    context: &types::LlmContext,
    options: &StreamOptions,
) -> Result<BoxStream<Result<types::StreamEvent>>> {
    use types::Provider;
    match model.provider {
        Provider::Anthropic => {
            providers::anthropic::AnthropicProvider.stream(model, context, options).await
        }
        Provider::OpenAI => {
            providers::openai::OpenAIProvider.stream(model, context, options).await
        }
    }
}

/// 等待流完成，返回最终的 AssistantMessage
pub async fn complete_llm(
    model: &types::Model,
    context: &types::LlmContext,
    options: &StreamOptions,
) -> Result<types::AssistantMessage> {
    use futures::StreamExt;
    use types::StreamEvent;

    let mut stream = stream_llm(model, context, options).await?;
    let mut final_message = None;

    while let Some(event) = stream.next().await {
        match event? {
            StreamEvent::Done(msg) | StreamEvent::Error(msg) => {
                final_message = Some(msg);
            }
            _ => {}
        }
    }

    final_message.ok_or_else(|| anyhow::anyhow!("stream ended without Done event"))
}
