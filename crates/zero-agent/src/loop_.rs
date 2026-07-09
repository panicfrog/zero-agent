use crate::{
    context::AgentContext,
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
    before_hook: Option<Arc<dyn crate::context::BeforeToolCallHook>>,
    after_hook: Option<Arc<dyn crate::context::AfterToolCallHook>>,
    session_id: String,
    emit: Emit,
) -> Vec<ToolResultMessage> {
    let mut results: Vec<ToolResultMessage> = Vec::with_capacity(tool_calls.len());
    for batch in partition_tool_calls(tool_calls, tools) {
        if batch.safe && batch.calls.len() > 1 {
            results.extend(
                run_batch_concurrent(
                    &batch.calls,
                    tools,
                    &before_hook,
                    &after_hook,
                    session_id.clone(),
                    emit.clone(),
                )
                .await,
            );
        } else {
            // singleton safe call OR non-safe barrier: run serially, reusing
            // run_one_tool (which emits Start/End + runs hooks).
            for tc in &batch.calls {
                results.push(
                    run_one_tool(
                        tc,
                        tools,
                        &before_hook,
                        &after_hook,
                        session_id.clone(),
                        emit.clone(),
                    )
                    .await,
                );
            }
        }
    }
    results
}

/// A run of tool calls with uniform concurrency-safety. Consecutive
/// concurrency-safe calls merge into one `safe` batch (run concurrently);
/// any non-safe call starts its own batch and acts as a serial barrier.
struct ToolBatch {
    safe: bool,
    calls: Vec<ToolCall>,
}

/// Partition tool calls into batches: consecutive concurrency-safe calls
/// merge into one concurrent batch; any non-safe call (or a safe call
/// following a non-safe one) starts a new batch. Batches run sequentially,
/// so a non-safe batch is a barrier. Mirrors Claude Code's
/// `toolOrchestration.ts` partition.
fn partition_tool_calls(
    tool_calls: &[ToolCall],
    tools: &[Arc<dyn Tool>],
) -> Vec<ToolBatch> {
    let mut batches: Vec<ToolBatch> = Vec::new();
    for tc in tool_calls {
        let safe = tools
            .iter()
            .find(|t| t.name() == tc.name)
            .map(|t| t.is_concurrency_safe(&tc.arguments))
            .unwrap_or(false); // unknown tool → not safe (serial path returns err)
        match batches.last_mut() {
            Some(b) if b.safe && safe => b.calls.push(tc.clone()),
            _ => batches.push(ToolBatch { safe, calls: vec![tc.clone()] }),
        }
    }
    batches
}

/// Run a batch of concurrency-safe calls concurrently, bounded by
/// `MAX_PARALLEL_TOOLS`. Results are returned in the original `calls` order
/// (recovered via `ToolCall.id`), fixing the prior completion-order bug.
async fn run_batch_concurrent(
    calls: &[ToolCall],
    tools: &[Arc<dyn Tool>],
    before_hook: &Option<Arc<dyn crate::context::BeforeToolCallHook>>,
    after_hook: &Option<Arc<dyn crate::context::AfterToolCallHook>>,
    session_id: String,
    emit: Emit,
) -> Vec<ToolResultMessage> {
    let sem = Arc::new(Semaphore::new(MAX_PARALLEL_TOOLS));
    let mut set = tokio::task::JoinSet::new();

    for tc in calls {
        emit(AgentEvent::ToolExecutionStart {
            session_id: session_id.clone(),
            tool_call_id: tc.id.clone(),
            tool_name: tc.name.clone(),
            args: tc.arguments.clone(),
        });
        let tc = tc.clone();
        let tools: Vec<Arc<dyn Tool>> = tools.to_vec();
        let sem = sem.clone();
        let before_hook = before_hook.clone();
        let after_hook = after_hook.clone();
        set.spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            let result = call_tool_with_hooks(&tc, &tools, &before_hook, &after_hook).await;
            (tc, result)
        });
    }

    // collect by original index to preserve tool_result order
    let mut indexed: Vec<Option<(ToolCall, ToolResult)>> =
        (0..calls.len()).map(|_| None).collect();
    while let Some(join_result) = set.join_next().await {
        if let Ok((tc, result)) = join_result {
            let idx = calls
                .iter()
                .position(|c| c.id == tc.id)
                .expect("spawned tc came from calls");
            emit(AgentEvent::ToolExecutionEnd {
                session_id: session_id.clone(),
                tool_call_id: tc.id.clone(),
                tool_name: tc.name.clone(),
                result: result.clone(),
            });
            indexed[idx] = Some((tc, result));
        }
    }
    indexed
        .into_iter()
        .map(|o| {
            let (tc, result) = o.expect("every spawned task completes");
            make_tool_result_message(&tc, result)
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::todo::TodoTool;
    use async_trait::async_trait;
    use parking_lot::Mutex;
    use serde_json::Value;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// Records `(tool_name, start_instant)` for each executed call.
    type Trace = Arc<Mutex<Vec<(String, Instant)>>>;

    /// A stub tool whose safety and simulated work duration are configurable.
    struct StubTool {
        tool_name: String,
        safe: bool,
        work_ms: u64,
        trace: Trace,
    }

    fn mk_call_args(id: &str, name: &str, args: Value) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: args,
        }
    }

    #[async_trait]
    impl Tool for StubTool {
        fn name(&self) -> &str { &self.tool_name }
        fn description(&self) -> &str { "stub" }
        fn parameters_schema(&self) -> Value { serde_json::json!({}) }

        async fn execute(&self, _id: &str, _args: Value) -> ToolResult {
            let start = Instant::now();
            self.trace.lock().push((self.tool_name.clone(), start));
            tokio::time::sleep(Duration::from_millis(self.work_ms)).await;
            ToolResult::ok(self.tool_name.clone())
        }

        fn is_concurrency_safe(&self, _: &Value) -> bool { self.safe }
    }

    fn emit_noop() -> Emit { Arc::new(|_| {}) }

    fn mk_call(id: &str, name: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: Value::Null,
        }
    }

    /// Two concurrency-safe calls issued together must start concurrently
    /// (within ~20ms of each other) and results must preserve input order.
    #[tokio::test]
    async fn safe_calls_run_concurrently_and_preserve_order() {
        let trace: Trace = Arc::new(Mutex::new(Vec::new()));
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(StubTool { tool_name: "safe_a".into(), safe: true, work_ms: 60, trace: trace.clone() }),
            Arc::new(StubTool { tool_name: "safe_b".into(), safe: true, work_ms: 60, trace: trace.clone() }),
        ];
        let calls = vec![mk_call("1", "safe_a"), mk_call("2", "safe_b")];

        let results = execute_tool_calls(
            &calls, &tools, None, None, "s".into(), emit_noop(),
        )
        .await;

        // order preserved relative to tool_use order
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].tool_call_id, "1");
        assert_eq!(results[1].tool_call_id, "2");

        // both started — concurrent: starts within 20ms of each other
        let t = trace.lock();
        assert_eq!(t.len(), 2, "both tools should have executed");
        let delta = if t[0].1 >= t[1].1 { t[0].1 - t[1].1 } else { t[1].1 - t[0].1 };
        assert!(
            delta < Duration::from_millis(20),
            "safe calls should start concurrently, delta = {delta:?}"
        );
    }

    /// A non-safe call acts as a serial barrier: with safe then non-safe in
    /// the same turn, the non-safe call must start strictly after the safe
    /// call completes.
    #[tokio::test]
    async fn unsafe_call_is_a_serial_barrier() {
        let trace: Trace = Arc::new(Mutex::new(Vec::new()));
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(StubTool { tool_name: "safe_x".into(), safe: true, work_ms: 60, trace: trace.clone() }),
            Arc::new(StubTool { tool_name: "unsafe_y".into(), safe: false, work_ms: 10, trace: trace.clone() }),
        ];
        let calls = vec![mk_call("1", "safe_x"), mk_call("2", "unsafe_y")];

        let results = execute_tool_calls(
            &calls, &tools, None, None, "s".into(), emit_noop(),
        )
        .await;

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].tool_call_id, "1");
        assert_eq!(results[1].tool_call_id, "2");

        let t = trace.lock();
        assert_eq!(t.len(), 2);
        let (name0, start0) = &t[0];
        let (name1, start1) = &t[1];
        // the safe call runs first; the non-safe call starts after it would
        // have finished its 60ms of work (barrier).
        assert_eq!(name0, "safe_x");
        assert_eq!(name1, "unsafe_y");
        assert!(
            *start1 >= *start0 + Duration::from_millis(60),
            "non-safe call must start after the safe call completes (barrier); \
             start0={start0:?} start1={start1:?}"
        );
    }

    /// A safe call following a non-safe call starts a fresh batch and runs
    /// strictly after the non-safe call completes.
    #[tokio::test]
    async fn partition_breaks_on_unsafe_then_reserializes() {
        let trace: Trace = Arc::new(Mutex::new(Vec::new()));
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(StubTool { tool_name: "u".into(), safe: false, work_ms: 40, trace: trace.clone() }),
            Arc::new(StubTool { tool_name: "s".into(), safe: true, work_ms: 5, trace: trace.clone() }),
        ];
        let calls = vec![mk_call("a", "u"), mk_call("b", "s")];

        let results = execute_tool_calls(
            &calls, &tools, None, None, "s".into(), emit_noop(),
        )
        .await;

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].tool_call_id, "a");
        assert_eq!(results[1].tool_call_id, "b");

        let t = trace.lock();
        assert_eq!(t.len(), 2);
        assert!(t[1].1 >= t[0].1 + Duration::from_millis(40));
    }

    // ---- TODO tool under the new concurrency-safe dispatch model ----

    /// The core change: TodoTool must report concurrency-safe so it enters
    /// the concurrent batch path. Without this, multiple todo calls in one
    /// turn would serialize unnecessarily.
    #[tokio::test]
    async fn todo_is_marked_concurrency_safe() {
        let t = TodoTool::new();
        assert!(t.is_concurrency_safe(&Value::Null));
        // safety holds regardless of args (e.g. a create op)
        assert!(t.is_concurrency_safe(&serde_json::json!({"operation":"create","title":"x"})));
    }

    /// Multiple todo operations issued in one turn run through the
    /// concurrent batch. Despite sharing one TodoTool (Arc<Mutex<Vec>>),
    /// the lock keeps state consistent, and results preserve tool_use
    /// order. Explicit ids make the concurrent creates deterministic.
    #[tokio::test]
    async fn todo_concurrent_dispatch_stays_consistent_and_ordered() {
        let todo = Arc::new(TodoTool::new()) as Arc<dyn Tool>;
        let tools: Vec<Arc<dyn Tool>> = vec![todo.clone()];

        // three creates with explicit ids, issued together → one concurrent batch
        let calls = vec![
            mk_call_args("c1", "todo", serde_json::json!({"operation":"create","id":1,"title":"A"})),
            mk_call_args("c2", "todo", serde_json::json!({"operation":"create","id":2,"title":"B"})),
            mk_call_args("c3", "todo", serde_json::json!({"operation":"create","id":3,"title":"C"})),
        ];
        let results = execute_tool_calls(
            &calls, &tools, None, None, "s".into(), emit_noop(),
        ).await;

        // order preserved
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].tool_call_id, "c1");
        assert_eq!(results[1].tool_call_id, "c2");
        assert_eq!(results[2].tool_call_id, "c3");
        // all succeeded
        for r in &results {
            assert!(!r.is_error, "create failed: {:?}", r.content);
            let txt = r.content[0].as_text().unwrap();
            assert!(txt.contains("Created todo"), "unexpected: {txt}");
        }

        // shared state consistent: list sees exactly A, B, C, each pending
        let list = execute_tool_calls(
            &[mk_call_args("l", "todo", serde_json::json!({"operation":"list"}))],
            &tools, None, None, "s".into(), emit_noop(),
        ).await;
        let list_txt = list[0].content[0].as_text().unwrap();
        assert!(list_txt.contains("#1: A") && list_txt.contains("pending"));
        assert!(list_txt.contains("#2: B"));
        assert!(list_txt.contains("#3: C"));
    }

    /// Full lifecycle (create → update in_progress → complete) through
    /// dispatch, including a non-safe tool interleaved in the same turn to
    /// confirm the barrier runs and does not corrupt todo state.
    #[tokio::test]
    async fn todo_full_lifecycle_through_dispatch_with_barrier() {
        let todo = Arc::new(TodoTool::new()) as Arc<dyn Tool>;
        let trace: Trace = Arc::new(Mutex::new(Vec::new()));
        let unsafe_tool: Arc<dyn Tool> = Arc::new(StubTool {
            tool_name: "writer".into(), safe: false, work_ms: 5, trace: trace.clone(),
        });
        let tools: Vec<Arc<dyn Tool>> = vec![todo.clone(), unsafe_tool];

        // create #1 (safe) then a non-safe writer in the same turn:
        // partition → [safe create][unsafe writer]. Both run, in order.
        let r1 = execute_tool_calls(
            &[
                mk_call_args("a", "todo", serde_json::json!({"operation":"create","id":1,"title":"Task 1"})),
                mk_call("b", "writer"),
            ],
            &tools, None, None, "s".into(), emit_noop(),
        ).await;
        assert_eq!(r1.len(), 2);
        assert_eq!(r1[0].tool_call_id, "a");
        assert_eq!(r1[1].tool_call_id, "b");
        assert!(!r1[0].is_error);
        // the non-safe writer actually ran (barrier respected, not skipped)
        assert_eq!(trace.lock().len(), 1);

        // mark in_progress then complete, via dispatch
        let r2 = execute_tool_calls(
            &[mk_call_args("u", "todo", serde_json::json!({"operation":"update","id":1,"status":"in_progress"}))],
            &tools, None, None, "s".into(), emit_noop(),
        ).await;
        assert!(r2[0].content[0].as_text().unwrap().contains("in_progress"));

        let r3 = execute_tool_calls(
            &[mk_call_args("d", "todo", serde_json::json!({"operation":"complete","id":1}))],
            &tools, None, None, "s".into(), emit_noop(),
        ).await;
        assert!(r3[0].content[0].as_text().unwrap().contains("Completed todo #1"));

        let list = execute_tool_calls(
            &[mk_call_args("l", "todo", serde_json::json!({"operation":"list"}))],
            &tools, None, None, "s".into(), emit_noop(),
        ).await;
        assert!(list[0].content[0].as_text().unwrap().contains("completed"));
    }

    /// Concurrency is driven by is_concurrency_safe, not the `parallel`
    /// flag. A parallel=false todo issued with another safe call still
    /// dispatches concurrently; the flag is now only a planning hint.
    #[tokio::test]
    async fn todo_parallel_false_still_dispatches_concurrently_when_safe() {
        let todo = Arc::new(TodoTool::new()) as Arc<dyn Tool>;
        let tools: Vec<Arc<dyn Tool>> = vec![todo];

        let calls = vec![
            mk_call_args("p1", "todo", serde_json::json!({"operation":"create","id":1,"title":"P-false","parallel":false})),
            mk_call_args("p2", "todo", serde_json::json!({"operation":"create","id":2,"title":"P-true","parallel":true})),
        ];
        let results = execute_tool_calls(
            &calls, &tools, None, None, "s".into(), emit_noop(),
        ).await;

        // both safe (todo), regardless of `parallel` flag → one concurrent batch
        assert_eq!(results.len(), 2);
        assert!(!results[0].is_error && !results[1].is_error);
        assert_eq!(results[0].tool_call_id, "p1");
        assert_eq!(results[1].tool_call_id, "p2");

        let list = execute_tool_calls(
            &[mk_call_args("l", "todo", serde_json::json!({"operation":"list"}))],
            &tools, None, None, "s".into(), emit_noop(),
        ).await;
        let txt = list[0].content[0].as_text().unwrap();
        assert!(txt.contains("parallel=false"));
        assert!(txt.contains("parallel=true"));
    }
}
