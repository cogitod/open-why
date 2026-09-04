use super::catalog::{registry_digest, registry_tools, MCP_CONTRACTS};
use super::common::{tool_response, ToolError, MAX_RESPONSE_BYTES};
use super::handlers::dispatch_tool;
use crate::{db, store::CURRENT_RATIONALE_CONTRACT};
use anyhow::Result;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

fn jsonrpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message.into()}})
}
fn write_resp(writer: &mut impl Write, value: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        let id = value.get("id").cloned().unwrap_or(Value::Null);
        bytes = serde_json::to_vec(&jsonrpc_error(
            id,
            -32603,
            "response exceeds the configured byte limit",
        ))?;
    }
    writer.write_all(&bytes)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn server_now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

pub(super) fn serve() -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let store = db::Store::open_default()?;
    serve_io(&store, stdin.lock(), &mut stdout, server_now_epoch)
}

pub(super) fn serve_io(
    store: &db::Store,
    reader: impl BufRead,
    writer: &mut impl Write,
    clock: impl Fn() -> i64,
) -> Result<()> {
    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(error) => {
                write_resp(
                    writer,
                    &jsonrpc_error(Value::Null, -32700, format!("parse error: {error}")),
                )?;
                continue;
            }
        };
        if let Some(response) = handle_message(store, &message, clock()) {
            write_resp(writer, &response)?;
        }
    }
    Ok(())
}

fn handle_message(store: &db::Store, message: &Value, as_of: i64) -> Option<Value> {
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    let Some(object) = message.as_object() else {
        return Some(jsonrpc_error(id, -32600, "request must be a JSON object"));
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(jsonrpc_error(id, -32600, "jsonrpc must be `2.0`"));
    }
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        return Some(jsonrpc_error(id, -32600, "method must be a string"));
    };
    match method {
        "initialize" => Some(json!({
            "jsonrpc":"2.0",
            "id":id,
            "result":{
                "protocolVersion":"2024-11-05",
                "capabilities":{
                    "tools":{"listChanged":false},
                    "experimental":{"openWhy":{
                        "contract":CURRENT_RATIONALE_CONTRACT,
                        "contracts":MCP_CONTRACTS,
                        "registryDigest":registry_digest()
                    }}
                },
                "serverInfo":{"name":"open-why","version":env!("CARGO_PKG_VERSION")}
            }
        })),
        "tools/list" => Some(json!({
            "jsonrpc":"2.0",
            "id":id,
            "result":{"tools":registry_tools(),"_meta":{
                "contract":CURRENT_RATIONALE_CONTRACT,
                "contracts":MCP_CONTRACTS,
                "registryDigest":registry_digest()
            }}
        })),
        "tools/call" => {
            let Some(params) = object.get("params").and_then(Value::as_object) else {
                return Some(tool_response(
                    id,
                    Err(ToolError::new(
                        "invalid_arguments",
                        "params must be an object",
                    )),
                ));
            };
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(tool_response(
                    id,
                    Err(ToolError::new(
                        "invalid_arguments",
                        "tool name must be a string",
                    )),
                ));
            };
            let arguments = params.get("arguments").unwrap_or(&Value::Null);
            if !arguments.is_object() {
                return Some(tool_response(
                    id,
                    Err(ToolError::new(
                        "invalid_arguments",
                        "tool arguments must be an object",
                    )),
                ));
            }
            Some(tool_response(
                id,
                dispatch_tool(store, name, arguments, as_of),
            ))
        }
        "ping" => Some(json!({"jsonrpc":"2.0","id":id,"result":{}})),
        "notifications/initialized"
        | "notifications/cancelled"
        | "notifications/roots/list_changed" => None,
        _ => Some(jsonrpc_error(
            id,
            -32601,
            format!("method not found: {method}"),
        )),
    }
}
