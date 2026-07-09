use crate::tool::{Tool, ToolResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// 数据结构
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

impl std::fmt::Display for TodoStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TodoStatus::Pending => write!(f, "pending"),
            TodoStatus::InProgress => write!(f, "in_progress"),
            TodoStatus::Completed => write!(f, "completed"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: usize,
    pub title: String,
    pub status: TodoStatus,
    /// 该任务是否可与其他 parallel=true 的任务并发执行
    pub parallel: bool,
    /// 依赖的任务 ID 列表，这些任务完成后本任务才能开始
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub depends_on: Vec<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

// ---------------------------------------------------------------------------
// TodoTool
// ---------------------------------------------------------------------------

/// 内置工具：任务规划列表。
/// 主 agent 用此工具将复杂任务拆解为步骤，标记并发关系和依赖关系，跟踪执行进度。
#[derive(Clone)]
pub struct TodoTool {
    items: Arc<Mutex<Vec<TodoItem>>>,
}

impl TodoTool {
    pub fn new() -> Self {
        TodoTool {
            items: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Default for TodoTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TodoTool {
    fn name(&self) -> &str {
        "todo"
    }

    fn description(&self) -> &str {
        "Manage a task planning list. Break down complex tasks into steps, mark parallelism and \
         dependencies, and track execution progress."
    }

    fn is_concurrency_safe(&self, _: &Value) -> bool { true }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["create", "update", "list", "complete"],
                    "description": "Operation to perform:\n- create: add a new todo item\n- update: update status, notes, or dependency info\n- list: list all items with their status and dependencies\n- complete: mark an item as completed"
                },
                "title": {
                    "type": "string",
                    "description": "Title of the todo item (required for 'create')"
                },
                "id": {
                    "type": "integer",
                    "description": "ID of the todo item. For 'create': optional — specify a planned id (e.g. 1,2,3) so depends_on can reference it before creation; if omitted or conflicting, auto-assigned. Required for 'update' and 'complete'."
                },
                "status": {
                    "type": "string",
                    "enum": ["pending", "in_progress", "completed"],
                    "description": "New status (used with 'update')"
                },
                "parallel": {
                    "type": "boolean",
                    "description": "Whether this task can run concurrently with other parallel=true tasks. Default: false. Use true when the task is independent and can be dispatched to parallel_subagent."
                },
                "depends_on": {
                    "type": "array",
                    "items": { "type": "integer" },
                    "description": "IDs of tasks that must be completed before this task can start. Declarative — you can reference ids that haven't been created yet (useful when creating all todos in parallel upfront)."
                },
                "notes": {
                    "type": "string",
                    "description": "Optional notes to attach to the item (used with 'create' or 'update')"
                }
            },
            "required": ["operation"]
        })
    }

    async fn execute(&self, _id: &str, args: Value) -> ToolResult {
        let op = match args["operation"].as_str() {
            Some(o) => o,
            None => return ToolResult::err("missing required argument: operation"),
        };

        match op {
            "create" => {
                let title = match args["title"].as_str() {
                    Some(t) => t.to_string(),
                    None => return ToolResult::err("'create' requires 'title'"),
                };
                let parallel = args["parallel"].as_bool().unwrap_or(false);
                let depends_on: Vec<usize> = args["depends_on"]
                    .as_array()
                    .map(|arr| arr.iter().filter_map(|v| v.as_u64().map(|n| n as usize)).collect())
                    .unwrap_or_default();
                let notes = args["notes"].as_str().map(str::to_string);

                let mut items = self.items.lock().unwrap();

                // 支持 LLM 指定 id；若未指定或已被占用，自动分配
                let requested_id = args["id"].as_u64().map(|n| n as usize);
                let id = match requested_id {
                    Some(rid) if !items.iter().any(|i| i.id == rid) => rid,
                    _ => {
                        // 自动分配：取当前最大 id + 1，保证不冲突
                        items.iter().map(|i| i.id).max().unwrap_or(0) + 1
                    }
                };

                items.push(TodoItem {
                    id,
                    title: title.clone(),
                    status: TodoStatus::Pending,
                    parallel,
                    depends_on: depends_on.clone(),
                    notes,
                });
                drop(items);

                let dep_str = if depends_on.is_empty() {
                    String::new()
                } else {
                    format!(", depends_on={:?}", depends_on)
                };
                ToolResult::ok(format!("Created todo #{id}: {title} [parallel={parallel}{dep_str}]"))
            }

            "list" => {
                let items = self.items.lock().unwrap();
                if items.is_empty() {
                    return ToolResult::ok("No todo items.");
                }
                let lines: Vec<String> = items.iter().map(|item| {
                    let mut meta = format!("parallel={}", item.parallel);
                    if !item.depends_on.is_empty() {
                        meta.push_str(&format!(", depends_on={:?}", item.depends_on));
                    }
                    let notes = item.notes.as_deref().map(|n| format!(" — {n}")).unwrap_or_default();
                    format!("[{}] #{}: {} ({}){}",
                        item.status, item.id, item.title, meta, notes)
                }).collect();
                ToolResult::ok(lines.join("\n"))
            }

            "update" => {
                let id = match args["id"].as_u64() {
                    Some(i) => i as usize,
                    None => return ToolResult::err("'update' requires 'id'"),
                };
                let mut items = self.items.lock().unwrap();
                match items.iter_mut().find(|i| i.id == id) {
                    None => ToolResult::err(format!("todo #{id} not found")),
                    Some(item) => {
                        if let Some(s) = args["status"].as_str() {
                            item.status = match s {
                                "pending" => TodoStatus::Pending,
                                "in_progress" => TodoStatus::InProgress,
                                "completed" => TodoStatus::Completed,
                                _ => return ToolResult::err(format!("unknown status: {s}")),
                            };
                        }
                        if let Some(p) = args["parallel"].as_bool() {
                            item.parallel = p;
                        }
                        if let Some(arr) = args["depends_on"].as_array() {
                            item.depends_on = arr.iter()
                                .filter_map(|v| v.as_u64().map(|n| n as usize))
                                .collect();
                        }
                        if let Some(n) = args["notes"].as_str() {
                            item.notes = Some(n.to_string());
                        }
                        ToolResult::ok(format!(
                            "Updated todo #{id}: status={}, parallel={}, depends_on={:?}",
                            item.status, item.parallel, item.depends_on
                        ))
                    }
                }
            }

            "complete" => {
                let id = match args["id"].as_u64() {
                    Some(i) => i as usize,
                    None => return ToolResult::err("'complete' requires 'id'"),
                };
                let mut items = self.items.lock().unwrap();
                match items.iter_mut().find(|i| i.id == id) {
                    None => ToolResult::err(format!("todo #{id} not found")),
                    Some(item) => {
                        item.status = TodoStatus::Completed;
                        ToolResult::ok(format!("Completed todo #{}: {}", item.id, item.title))
                    }
                }
            }

            _ => ToolResult::err(format!("unknown operation: {op}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn exec(tool: &TodoTool, args: Value) -> ToolResult {
        tool.execute("", args).await
    }

    #[tokio::test]
    async fn test_create_and_list() {
        let t = TodoTool::new();
        exec(&t, serde_json::json!({"operation": "create", "title": "Step 1", "parallel": false})).await;
        exec(&t, serde_json::json!({"operation": "create", "title": "Step 2", "parallel": true, "notes": "can run concurrently"})).await;
        let r = exec(&t, serde_json::json!({"operation": "list"})).await;
        assert!(r.content.contains("Step 1"));
        assert!(r.content.contains("Step 2"));
        assert!(r.content.contains("parallel=true"));
    }

    #[tokio::test]
    async fn test_depends_on() {
        let t = TodoTool::new();
        exec(&t, serde_json::json!({"operation": "create", "title": "Task A"})).await;
        exec(&t, serde_json::json!({"operation": "create", "title": "Task B"})).await;
        let r = exec(&t, serde_json::json!({"operation": "create", "title": "Task C", "depends_on": [1, 2]})).await;
        assert!(r.content.contains("depends_on"));
        let list = exec(&t, serde_json::json!({"operation": "list"})).await;
        assert!(list.content.contains("depends_on=[1, 2]"));
    }

    #[tokio::test]
    async fn test_forward_depends_on() {
        // depends_on 允许前向引用（被依赖方尚未创建），create 不应报错
        let t = TodoTool::new();
        let r = exec(&t, serde_json::json!({"operation": "create", "title": "Task X", "depends_on": [99]})).await;
        assert!(!r.is_error, "forward depends_on should be allowed at create time");
    }


    #[tokio::test]
    async fn test_update_parallel_and_deps() {
        let t = TodoTool::new();
        exec(&t, serde_json::json!({"operation": "create", "title": "Task A"})).await;
        exec(&t, serde_json::json!({"operation": "create", "title": "Task B"})).await;
        exec(&t, serde_json::json!({"operation": "update", "id": 2, "parallel": true, "depends_on": [1]})).await;
        let r = exec(&t, serde_json::json!({"operation": "list"})).await;
        assert!(r.content.contains("parallel=true"));
        assert!(r.content.contains("depends_on=[1]"));
    }

    #[tokio::test]
    async fn test_complete() {
        let t = TodoTool::new();
        exec(&t, serde_json::json!({"operation": "create", "title": "Task B"})).await;
        exec(&t, serde_json::json!({"operation": "complete", "id": 1})).await;
        let r = exec(&t, serde_json::json!({"operation": "list"})).await;
        assert!(r.content.contains("completed"));
    }

    #[tokio::test]
    async fn test_not_found() {
        let t = TodoTool::new();
        let r = exec(&t, serde_json::json!({"operation": "complete", "id": 99})).await;
        assert!(r.is_error);
    }
}
