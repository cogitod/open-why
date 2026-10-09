use super::catalog::{registry_digest, registry_tools, MCP_CONTRACTS};
use super::common::{
    tool_response, ToolError, MAX_ID_BYTES, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES,
};
use super::handlers::dispatch_tool;
use super::transport::Binding;
use crate::{db, store::CURRENT_RATIONALE_CONTRACT};
use anyhow::Result;
use serde_json::{json, Value};
use std::io::{self, BufRead, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};

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

/// Validate the configured store before selecting direct or daemon transport.
pub(super) fn serve() -> Result<()> {
    let (binding, lexical) = Binding::configured()?;
    if let Some(stream) = binding.connect()? {
        binding.verify(&lexical)?;
        return proxy_stdio(stream);
    }
    drop(lexical);
    let store = Mutex::new(db::Store::open_default()?);
    binding.verify(&store.lock().unwrap())?;
    serve_io_checked(
        &store,
        io::stdin().lock(),
        &mut io::stdout(),
        server_now_epoch,
        Some(&binding),
    )
}

pub(super) fn serve_daemon() -> Result<()> {
    let (binding, lexical) = Binding::configured()?;
    drop(lexical);
    let store = Arc::new(Mutex::new(db::Store::open_default()?));
    binding.verify(&store.lock().unwrap())?;
    let socket_path = binding.socket();
    // Never unlink an existing endpoint: it may be active, stale, or unrelated data.
    let listener = UnixListener::bind(&socket_path)?;
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;
    eprintln!("open-why: serving on {}", socket_path.display());
    for incoming in listener.incoming() {
        let Ok(mut connection) = incoming else {
            continue;
        };
        let store = Arc::clone(&store);
        let binding = binding.clone();
        std::thread::spawn(move || -> Result<()> {
            binding.verify(
                &store
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            )?;
            binding.accept(&mut connection)?;
            let reader = io::BufReader::new(connection.try_clone()?);
            serve_io_checked(
                &store,
                reader,
                &mut connection,
                server_now_epoch,
                Some(&binding),
            )
        });
    }
    Ok(())
}

/// Forward this process's stdio verbatim onto `stream` until either side closes. Used when a
/// `why serve-daemon` is already handling this store, so the client sees an ordinary MCP server
/// with no protocol awareness needed at this layer.
fn proxy_stdio(stream: UnixStream) -> Result<()> {
    let mut upstream = stream.try_clone()?;
    let mut downstream = stream;
    std::thread::spawn(move || {
        let _ = io::copy(&mut io::stdin(), &mut upstream);
        // Half-close so the daemon's reader sees EOF on this connection instead of
        // blocking forever for more input that will never arrive.
        let _ = upstream.shutdown(std::net::Shutdown::Write);
    });
    // Do not join a thread blocked on client stdin after the daemon disconnects.
    // Returning lets the CLI process terminate and the MCP client reconnect.
    io::copy(&mut downstream, &mut io::stdout())?;
    Ok(())
}

#[cfg(test)]
pub(super) fn serve_io(
    store: &Mutex<db::Store>,
    reader: impl BufRead,
    writer: &mut impl Write,
    clock: impl Fn() -> i64,
) -> Result<()> {
    serve_io_checked(store, reader, writer, clock, None)
}

fn serve_io_checked(
    store: &Mutex<db::Store>,
    mut reader: impl BufRead,
    writer: &mut impl Write,
    clock: impl Fn() -> i64,
    binding: Option<&Binding>,
) -> Result<()> {
    loop {
        // Bound allocation before parsing, including unterminated frames. Closing
        // this session avoids draining an attacker-controlled, possibly endless line.
        let mut line = Vec::new();
        let size = (&mut reader)
            .take((MAX_REQUEST_BYTES + 1) as u64)
            .read_until(b'\n', &mut line)?;
        if size == 0 {
            break;
        }
        if size > MAX_REQUEST_BYTES {
            write_resp(
                writer,
                &jsonrpc_error(
                    Value::Null,
                    -32600,
                    "request exceeds the 8 MiB wire byte limit; connection closed",
                ),
            )?;
            break;
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let message: Value = match serde_json::from_slice(&line) {
            Ok(message) => message,
            Err(error) => {
                write_resp(
                    writer,
                    &jsonrpc_error(Value::Null, -32700, format!("parse error: {error}")),
                )?;
                continue;
            }
        };
        let response = {
            let store = store
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(binding) = binding {
                binding.verify(&store)?;
            }
            handle_message(&store, &message, clock())
        };
        if let Some(response) = response {
            write_resp(writer, &response)?;
        }
    }
    Ok(())
}

fn handle_message(store: &db::Store, message: &Value, as_of: i64) -> Option<Value> {
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    if crate::privacy::check_value(&id).is_err() {
        return Some(jsonrpc_error(
            Value::Null,
            -32600,
            "request id rejected by sensitive-data policy",
        ));
    }
    if !matches!(&id, Value::Null | Value::Number(_))
        && !matches!(&id, Value::String(value) if value.len() <= MAX_ID_BYTES)
    {
        return Some(jsonrpc_error(
            Value::Null,
            -32600,
            "id must be null, a number, or a string of at most 512 UTF-8 bytes",
        ));
    }
    let Some(object) = message.as_object() else {
        return Some(jsonrpc_error(id, -32600, "request must be a JSON object"));
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(jsonrpc_error(id, -32600, "jsonrpc must be `2.0`"));
    }
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        return Some(jsonrpc_error(id, -32600, "method must be a string"));
    };
    if crate::privacy::check_text(method).is_err() {
        return Some(jsonrpc_error(
            id,
            -32600,
            "method rejected by sensitive-data policy",
        ));
    }
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
