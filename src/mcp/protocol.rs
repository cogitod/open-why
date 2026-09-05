use super::catalog::{registry_digest, registry_tools, MCP_CONTRACTS};
use super::common::{tool_response, ToolError, MAX_RESPONSE_BYTES};
use super::handlers::dispatch_tool;
use crate::{db, store::CURRENT_RATIONALE_CONTRACT};
use anyhow::Result;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
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

/// Path to the Unix-domain socket a `why serve-daemon` instance listens on for the store a
/// plain `why serve` would open. One socket per resolved store, so unrelated stores (and
/// isolated per-test temp stores) never share a daemon.
fn daemon_socket_path() -> PathBuf {
    db::default_path().with_extension("sock")
}

/// Run as an MCP stdio server for one client. If a `why serve-daemon` is already listening for
/// this store, this process becomes a thin byte proxy onto it (sharing its loaded embedder and
/// avoiding a redundant model load); otherwise it serves the request itself, exactly as if no
/// daemon existed. Never blocks waiting for a daemon that may never appear.
pub(super) fn serve() -> Result<()> {
    let socket_path = daemon_socket_path();
    if let Ok(stream) = UnixStream::connect(&socket_path) {
        return proxy_stdio(stream);
    }
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let store = Mutex::new(db::Store::open_default()?);
    serve_io(&store, stdin.lock(), &mut stdout, server_now_epoch)
}

/// Run as a long-lived, client-independent MCP server: load the store once and accept any
/// number of concurrent connections on a Unix-domain socket, each served by `serve_io` exactly
/// as a direct stdio client would be. Meant to run under a supervisor (e.g. launchd) so its
/// lifetime never depends on any one MCP client's session.
pub(super) fn serve_daemon() -> Result<()> {
    let socket_path = daemon_socket_path();
    // Best-effort: clear a stale socket file left by a daemon that did not shut down cleanly.
    // A live daemon still holding this path fails the following bind, which is the desired
    // "only one daemon" behavior.
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path)?;
    eprintln!(
        "open-why: serving {} on {}",
        db::default_path().display(),
        socket_path.display()
    );
    let store = Arc::new(Mutex::new(db::Store::open_default()?));
    for incoming in listener.incoming() {
        let Ok(connection) = incoming else { continue };
        let store = Arc::clone(&store);
        std::thread::spawn(move || {
            let Ok(reader) = connection.try_clone() else {
                return;
            };
            let mut writer = connection;
            let _ = serve_io(
                &store,
                io::BufReader::new(reader),
                &mut writer,
                server_now_epoch,
            );
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
    let relay_stdin = std::thread::spawn(move || {
        let _ = io::copy(&mut io::stdin(), &mut upstream);
        // Half-close so the daemon's reader sees EOF on this connection instead of
        // blocking forever for more input that will never arrive.
        let _ = upstream.shutdown(std::net::Shutdown::Write);
    });
    let _ = io::copy(&mut downstream, &mut io::stdout());
    let _ = relay_stdin.join();
    Ok(())
}

pub(super) fn serve_io(
    store: &Mutex<db::Store>,
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
        let response = {
            let store = store
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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
