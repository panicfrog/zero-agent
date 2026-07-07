use crate::{BoxStream, types::{LlmContext, Model, StreamEvent}};
use anyhow::Result;
use async_trait::async_trait;

#[derive(Debug, Clone, Default)]
pub struct StreamOptions {
    pub temperature: Option<f32>,
    /// 覆盖 model.max_tokens
    pub max_tokens: Option<u32>,
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn stream(
        &self,
        model: &Model,
        context: &LlmContext,
        options: &StreamOptions,
    ) -> Result<BoxStream<Result<StreamEvent>>>;
}
