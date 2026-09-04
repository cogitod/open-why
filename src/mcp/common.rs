use serde_json::{json, Value};

pub(super) const MCP_ERROR_CONTRACT: &str = "open-why.mcp-tool-error/v1";
pub(super) const MAX_QUERY_BYTES: usize = 4 * 1024;
pub(super) const MAX_RESULT_COUNT: usize = 100;
pub(super) const MAX_PREVIEW_BYTES: usize = 512;
pub(super) const MAX_ID_BYTES: usize = 512;
pub(super) const MAX_AUTHORITY_BYTES: usize = 4 * 1024;
pub(super) const MAX_TITLE_BYTES: usize = 16 * 1024;
pub(super) const MAX_BODY_BYTES: usize = 1024 * 1024;
pub(super) const MAX_IMPORT_ROWS: usize = 1000;
pub(super) const MAX_IMPORT_BYTES: usize = 2 * 1024 * 1024;
pub(super) const MAX_GIT_REFS: usize = 100;
pub(super) const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_OPERATOR_DIAGNOSTIC_BYTES: usize = 2 * 1024;
#[derive(Debug)]
pub(super) struct ToolError {
    pub(super) payload: Value,
}

impl ToolError {
    pub(super) fn new(code: &str, message: impl Into<String>) -> Self {
        Self::with_retryable(code, message, false)
    }

    pub(super) fn with_retryable(code: &str, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            payload: json!({
                "contract": MCP_ERROR_CONTRACT,
                "status": "error",
                "code": code,
                "message": message.into(),
                "retryable": retryable
            }),
        }
    }

    pub(super) fn internal(error: impl std::fmt::Display) -> Self {
        emit_operator_diagnostic("internal tool failure", error);
        Self::new("internal", "internal tool failure")
    }

    pub(super) fn resolution(payload: Value) -> Self {
        Self { payload }
    }
}

pub(super) type ToolResult = std::result::Result<Value, ToolError>;
pub(super) fn tool_response(id: Value, result: ToolResult) -> Value {
    let (payload, is_error) = match result {
        Ok(payload) => (payload, false),
        Err(error) => (error.payload, true),
    };
    let text = serde_json::to_string(&payload).unwrap_or_else(|error| {
        emit_operator_diagnostic("serialize tool response", error);
        format!(
            "{{\"contract\":\"{MCP_ERROR_CONTRACT}\",\"status\":\"error\",\"code\":\"internal\",\"message\":\"internal tool failure\",\"retryable\":false}}"
        )
    });
    json!({
        "jsonrpc":"2.0",
        "id":id,
        "result":{"content":[{"type":"text","text":text}],"isError":is_error}
    })
}

fn emit_operator_diagnostic(context: &str, error: impl std::fmt::Display) {
    let mut diagnostic = format!("[open-why] {context}: {error}");
    if diagnostic.len() > MAX_OPERATOR_DIAGNOSTIC_BYTES {
        let mut end = MAX_OPERATOR_DIAGNOSTIC_BYTES;
        while !diagnostic.is_char_boundary(end) {
            end -= 1;
        }
        diagnostic.truncate(end);
    }
    eprintln!("{diagnostic}");
}

pub(super) fn tool_wire_size(payload: &Value) -> std::result::Result<usize, ToolError> {
    serde_json::to_vec(&tool_response(Value::Null, Ok(payload.clone())))
        .map(|bytes| bytes.len())
        .map_err(ToolError::internal)
}
