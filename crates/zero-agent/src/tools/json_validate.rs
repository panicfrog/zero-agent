use crate::tool::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::Value;

/// 内置工具：通过 JSON Schema 校验 JSON 数据的合法性。
pub struct JsonValidateTool;

#[async_trait]
impl Tool for JsonValidateTool {
    fn name(&self) -> &str {
        "json_validate"
    }

    fn description(&self) -> &str {
        "Validate a JSON value against a JSON Schema. \
         Returns a list of validation errors, or confirms the data is valid."
    }

    fn is_concurrency_safe(&self, _: &Value) -> bool { true }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "schema": {
                    "type": "object",
                    "description": "The JSON Schema to validate against."
                },
                "data": {
                    "description": "The JSON value to validate."
                }
            },
            "required": ["schema", "data"]
        })
    }

    async fn execute(&self, _id: &str, args: Value) -> ToolResult {
        let schema = match args.get("schema") {
            Some(s) => s.clone(),
            None => return ToolResult::err("missing required argument: schema"),
        };
        // LLM 有时会把 data 序列化成字符串传入，尝试自动解析
        let data = match args.get("data") {
            Some(Value::String(s)) => match serde_json::from_str(s) {
                Ok(v) => v,
                Err(_) => Value::String(s.clone()),
            },
            Some(d) => d.clone(),
            None => return ToolResult::err("missing required argument: data"),
        };

        let validator = match jsonschema::validator_for(&schema) {
            Ok(v) => v,
            Err(e) => return ToolResult::err(format!("invalid schema: {e}")),
        };

        let errors: Vec<String> = validator
            .iter_errors(&data)
            .map(|e| format!("- {} (path: {})", e, e.instance_path))
            .collect();

        if errors.is_empty() {
            ToolResult::ok("Valid: the data conforms to the schema.")
        } else {
            ToolResult::ok(format!("Invalid: {} error(s) found:\n{}", errors.len(), errors.join("\n")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn run(schema: Value, data: Value) -> ToolResult {
        JsonValidateTool.execute("", serde_json::json!({ "schema": schema, "data": data })).await
    }

    #[tokio::test]
    async fn test_valid() {
        let r = run(
            serde_json::json!({ "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] }),
            serde_json::json!({ "name": "Alice" }),
        ).await;
        assert!(!r.is_error);
        assert!(r.content.contains("Valid"));
    }

    #[tokio::test]
    async fn test_invalid_type() {
        let r = run(
            serde_json::json!({ "type": "object", "properties": { "age": { "type": "integer" } }, "required": ["age"] }),
            serde_json::json!({ "age": "not-a-number" }),
        ).await;
        assert!(!r.is_error);
        assert!(r.content.contains("Invalid"));
    }

    #[tokio::test]
    async fn test_missing_required() {
        let r = run(
            serde_json::json!({ "type": "object", "required": ["id"] }),
            serde_json::json!({}),
        ).await;
        assert!(r.content.contains("Invalid"));
    }

    #[tokio::test]
    async fn test_bad_schema() {
        let r = JsonValidateTool.execute("", serde_json::json!({ "schema": "not-an-object", "data": {} })).await;
        assert!(r.is_error);
    }
}
