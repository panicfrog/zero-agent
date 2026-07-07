use crate::{
    context::{AgentContext, ToolExecutionMode},
    event::AgentEvent,
    tool::{Tool, ToolResult},
};
use anyhow::{Result, anyhow};
use futures::StreamExt;
use std::sync::Arc;
use tokio::sync::{Semaphore, mpsc};
use zero_ai::{
    provider::StreamOptions,
    stream_llm,
    types::{
        AssistantMessage, ContentBlock, LlmContext, Message, StopReason, StreamEvent,
        ToolCall, ToolResultMessage, UserMessage,
    },
};

const MAX_PARALLEL_TOOLS: usize = 8;

type Emit = Arc<dyn Fn(AgentEvent) + Send + Sync>;

// ---------------------------------------------------------------------------
// 公开入口
// ---------------------------------------------------------------------------

/// 运行 agent，返回最终文本输出。
///
/// - `initial_prompt`：用户初始消息文本
/// - `event_tx`：可选的事件通道，用于观察 agent 内部状态（含 session_id）
pub async fn agent_run(
    mut ctx: AgentContext,
    initial_prompt: impl Into<String>,
    event_tx: Option<mpsc::UnboundedSender<AgentEvent>>,
) -> Result<String> {
    let session_id = ctx.session_id.clone();
    let emit = make_emitter(event_tx);

    // 追加初始用户消息
    ctx.messages.push(Message::User(UserMessage::new(initial_prompt)));

    emit(AgentEvent::AgentStart { session_id: session_id.clone() });

    let result = run_loop(&mut ctx, emit.clone()).await;

    let final_messages = ctx.messages.clone();
    emit(AgentEvent::AgentEnd { session_id, messages: final_messages });

    result
}

// ---------------------------------------------------------------------------
// 主循环
// ---------------------------------------------------------------------------

async fn run_loop(ctx: &mut AgentContext, emit: Emit) -> Result<String> {
    let options = StreamOptions::default();
    let session_id = ctx.session_id.clone();

    for _iter in 0..ctx.max_iterations {
        emit(AgentEvent::TurnStart { session_id: session_id.clone() });

        // 构建 LLM 上下文
        let llm_ctx = LlmContext {
            system_prompt: Some(ctx.system_prompt.clone()),
            messages: ctx.messages.clone(),
            tools: ctx.tools.iter().map(|t| t.to_spec()).collect(),
        };

        // 流式调用 LLM
        let assistant_msg =
            stream_response(&ctx.model, &llm_ctx, &options, session_id.clone(), emit.clone())
                .await?;

        let tool_calls = assistant_msg.tool_calls();

        // 没有工具调用 → 对话结束
        if tool_calls.is_empty() {
            let text = assistant_msg.text_output();
            ctx.messages.push(Message::Assistant(assistant_msg.clone()));
            emit(AgentEvent::TurnEnd {
                session_id: session_id.clone(),
                message: assistant_msg,
                tool_results: vec![],
            });
            return Ok(text);
        }

        // 错误/中止
        if matches!(
            assistant_msg.stop_reason,
            StopReason::Error | StopReason::Aborted
        ) {
            return Err(anyhow!(
                "LLM stopped with {:?}: {}",
                assistant_msg.stop_reason,
                assistant_msg.error_message.as_deref().unwrap_or("")
            ));
        }

        // 执行工具
        let tool_calls_owned: Vec<ToolCall> = tool_calls.iter().map(|tc| (*tc).clone()).collect();
        let tool_results = execute_tool_calls(
            &tool_calls_owned,
            &ctx.tools,
            &ctx.tool_execution,
            ctx.before_tool_call.clone(),
            ctx.after_tool_call.clone(),
            session_id.clone(),
            emit.clone(),
        )
        .await;

        // 追加 assistant message + tool results
        ctx.messages.push(Message::Assistant(assistant_msg.clone()));
        for tr in &tool_results {
            ctx.messages.push(Message::ToolResult(tr.clone()));
        }

        emit(AgentEvent::TurnEnd {
            session_id: session_id.clone(),
            message: assistant_msg,
            tool_results,
        });
    }

    Err(anyhow!("agent reached max_iterations ({})", ctx.max_iterations))
}

// ---------------------------------------------------------------------------
// 流式响应消费
// ---------------------------------------------------------------------------

async fn stream_response(
    model: &zero_ai::types::Model,
    llm_ctx: &LlmContext,
    options: &StreamOptions,
    session_id: String,
    emit: Emit,
) -> Result<AssistantMessage> {
    let mut stream = stream_llm(model, llm_ctx, options).await?;
    let mut final_msg: Option<AssistantMessage> = None;

    while let Some(event) = stream.next().await {
        let event = event?;
        match &event {
            StreamEvent::Start(msg) => {
                emit(AgentEvent::MessageStart {
                    session_id: session_id.clone(),
                    message: msg.clone(),
                });
            }
            StreamEvent::Done(msg) => {
                final_msg = Some(msg.clone());
                emit(AgentEvent::MessageEnd {
                    session_id: session_id.clone(),
                    message: msg.clone(),
                });
            }
            StreamEvent::Error(msg) => {
                final_msg = Some(msg.clone());
                emit(AgentEvent::MessageEnd {
                    session_id: session_id.clone(),
                    message: msg.clone(),
                });
            }
            _ => {}
        }
        emit(AgentEvent::MessageDelta {
            session_id: session_id.clone(),
            event,
        });
    }

    final_msg.ok_or_else(|| anyhow!("stream ended without Done event"))
}

// ---------------------------------------------------------------------------
// 工具执行
// ---------------------------------------------------------------------------

async fn execute_tool_calls(
    tool_calls: &[ToolCall],
    tools: &[Arc<dyn Tool>],
    mode: &ToolExecutionMode,
    before_hook: Option<Arc<dyn crate::context::BeforeToolCallHook>>,
    after_hook: Option<Arc<dyn crate::context::AfterToolCallHook>>,
    session_id: String,
    emit: Emit,
) -> Vec<ToolResultMessage> {
    match mode {
        ToolExecutionMode::Sequential => {
            let mut results = Vec::new();
            for tc in tool_calls {
                let r = run_one_tool(
                    tc,
                    tools,
                    &before_hook,
                    &after_hook,
                    session_id.clone(),
                    emit.clone(),
                )
                .await;
                results.push(r);
            }
            results
        }
        ToolExecutionMode::Parallel => {
            let sem = Arc::new(Semaphore::new(MAX_PARALLEL_TOOLS));
            let mut set = tokio::task::JoinSet::new();

            for tc in tool_calls {
                let tc = tc.clone();
                let tools: Vec<Arc<dyn Tool>> = tools.to_vec();
                let sem = sem.clone();
                let before_hook = before_hook.clone();
                let after_hook = after_hook.clone();
                let session_id = session_id.clone();
                let emit = emit.clone();

                emit(AgentEvent::ToolExecutionStart {
                    session_id: session_id.clone(),
                    tool_call_id: tc.id.clone(),
                    tool_name: tc.name.clone(),
                    args: tc.arguments.clone(),
                });

                set.spawn(async move {
                    let _permit = sem.acquire().await.unwrap();
                    let result =
                        call_tool_with_hooks(&tc, &tools, &before_hook, &after_hook).await;
                    (tc, result, session_id, emit)
                });
            }

            let mut results = Vec::new();
            while let Some(join_result) = set.join_next().await {
                if let Ok((tc, tool_result, session_id, emit)) = join_result {
                    emit(AgentEvent::ToolExecutionEnd {
                        session_id,
                        tool_call_id: tc.id.clone(),
                        tool_name: tc.name.clone(),
                        result: tool_result.clone(),
                    });
                    results.push(make_tool_result_message(&tc, tool_result));
                }
            }
            results
        }
    }
}

async fn run_one_tool(
    tc: &ToolCall,
    tools: &[Arc<dyn Tool>],
    before_hook: &Option<Arc<dyn crate::context::BeforeToolCallHook>>,
    after_hook: &Option<Arc<dyn crate::context::AfterToolCallHook>>,
    session_id: String,
    emit: Emit,
) -> ToolResultMessage {
    emit(AgentEvent::ToolExecutionStart {
        session_id: session_id.clone(),
        tool_call_id: tc.id.clone(),
        tool_name: tc.name.clone(),
        args: tc.arguments.clone(),
    });

    let result = call_tool_with_hooks(tc, tools, before_hook, after_hook).await;

    emit(AgentEvent::ToolExecutionEnd {
        session_id,
        tool_call_id: tc.id.clone(),
        tool_name: tc.name.clone(),
        result: result.clone(),
    });

    make_tool_result_message(tc, result)
}

async fn call_tool_with_hooks(
    tc: &ToolCall,
    tools: &[Arc<dyn Tool>],
    before_hook: &Option<Arc<dyn crate::context::BeforeToolCallHook>>,
    after_hook: &Option<Arc<dyn crate::context::AfterToolCallHook>>,
) -> ToolResult {
    if let Some(hook) = before_hook {
        if let Some(reason) = hook.before_tool_call(&tc.name, &tc.arguments).await {
            return ToolResult::err(format!("tool call blocked: {reason}"));
        }
    }

    let result = dispatch_tool(tc, tools).await;

    if let Some(hook) = after_hook {
        hook.after_tool_call(&tc.name, result).await
    } else {
        result
    }
}

async fn dispatch_tool(tc: &ToolCall, tools: &[Arc<dyn Tool>]) -> ToolResult {
    match tools.iter().find(|t| t.name() == tc.name) {
        Some(tool) => tool.execute(&tc.id, tc.arguments.clone()).await,
        None => ToolResult::err(format!("unknown tool: {}", tc.name)),
    }
}

fn make_tool_result_message(tc: &ToolCall, result: ToolResult) -> ToolResultMessage {
    ToolResultMessage {
        tool_call_id: tc.id.clone(),
        tool_name: tc.name.clone(),
        content: vec![ContentBlock::text(&result.content)],
        is_error: result.is_error,
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
    }
}

// ---------------------------------------------------------------------------
// 辅助：构建 emit 闭包
// ---------------------------------------------------------------------------

fn make_emitter(tx: Option<mpsc::UnboundedSender<AgentEvent>>) -> Emit {
    Arc::new(move |event| {
        if let Some(tx) = &tx {
            let _ = tx.send(event);
        }
    })
}
