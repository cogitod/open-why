use crate::store::{
    self, CommitLinksErrorCode, CommitLinksResolution, CurrentRecordErrorCode,
    CurrentRecordResolution, ExternalDecision, RationaleHistoryErrorCode,
    RationaleHistoryResolution, Record, RecordIdentityConflict, COMMIT_LINKS_CONTRACT,
    CURRENT_RATIONALE_CONTRACT, MAX_COMMIT_LINKS_PAGE_RECORDS, MAX_HISTORY_PAGE_RECORDS,
    MAX_SUPERSESSION_CHAIN, MAX_TEMPORAL_VALUE_BYTES, RATIONALE_HISTORY_CONTRACT,
    RATIONALE_IMPORT_CONTRACT,
};
use crate::{db, miner};
use anyhow::Result;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::Path;

const MCP_ERROR_CONTRACT: &str = "open-why.mcp-tool-error/v1";
const MCP_CONTRACTS: &[&str] = &[
    CURRENT_RATIONALE_CONTRACT,
    RATIONALE_HISTORY_CONTRACT,
    COMMIT_LINKS_CONTRACT,
    RATIONALE_IMPORT_CONTRACT,
];
const MAX_QUERY_BYTES: usize = 4 * 1024;
const MAX_RESULT_COUNT: usize = 100;
const MAX_PREVIEW_BYTES: usize = 512;
const MAX_ID_BYTES: usize = 512;
const MAX_AUTHORITY_BYTES: usize = 4 * 1024;
const MAX_TITLE_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 1024 * 1024;
const MAX_IMPORT_ROWS: usize = 1000;
const MAX_IMPORT_BYTES: usize = 2 * 1024 * 1024;
const MAX_GIT_REFS: usize = 100;
const MAX_GIT_HASH_BYTES: usize = 128;
const MAX_GIT_SUBJECT_BYTES: usize = 4 * 1024;
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolKind {
    Ask,
    Index,
    Capture,
    Import,
    Search,
    Get,
    History,
    CommitLinks,
    Link,
    Feedback,
}

#[derive(Clone, Copy)]
struct ToolSpec {
    name: &'static str,
    description: &'static str,
    kind: ToolKind,
}

const TOOL_SPECS: &[ToolSpec] = &[
    ToolSpec {
        name: "open-why_ask",
        description: "Ask why a decision was made in an explicitly identified repository.",
        kind: ToolKind::Ask,
    },
    ToolSpec {
        name: "open-why_index",
        description: "Index an explicitly identified repository's decision history.",
        kind: ToolKind::Index,
    },
    ToolSpec {
        name: "open-why_capture",
        description: "Capture a bounded decision in an explicit scope.",
        kind: ToolKind::Capture,
    },
    ToolSpec {
        name: "open-why_import",
        description: "Import bounded records into one explicit scope.",
        kind: ToolKind::Import,
    },
    ToolSpec {
        name: "open-why_search",
        description: "Return bounded stable-ID previews from an explicit scope.",
        kind: ToolKind::Search,
    },
    ToolSpec {
        name: "open-why_get",
        description: "Resolve an exact stable ID to its complete current rationale and evidence.",
        kind: ToolKind::Get,
    },
    ToolSpec {
        name: "open-why_history",
        description: "Page one exact supersession chain with complete records and Git evidence.",
        kind: ToolKind::History,
    },
    ToolSpec {
        name: "open-why_commit_links",
        description: "Page exact direct rationale links for one Git commit hash and scope.",
        kind: ToolKind::CommitLinks,
    },
    ToolSpec {
        name: "open-why_link",
        description: "Link a Git commit to a decision in an explicit scope.",
        kind: ToolKind::Link,
    },
    ToolSpec {
        name: "open-why_feedback",
        description: "Record retrieval feedback for a decision in an explicit scope.",
        kind: ToolKind::Feedback,
    },
];

#[derive(Debug)]
struct ToolError {
    payload: Value,
}

impl ToolError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            payload: json!({
                "contract": MCP_ERROR_CONTRACT,
                "status": "error",
                "code": code,
                "message": message.into(),
                "retryable": false
            }),
        }
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        Self::new("internal", format!("internal tool failure: {error}"))
    }

    fn resolution(payload: Value) -> Self {
        Self { payload }
    }
}

type ToolResult = std::result::Result<Value, ToolError>;

fn object_schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn import_row_schema() -> Value {
    object_schema(
        json!({
            "id":{"type":"string","maxLength":MAX_ID_BYTES},
            "kind":{"type":"string","maxLength":128},
            "title":{"type":"string","maxLength":MAX_TITLE_BYTES},
            "content":{"type":"string","maxLength":MAX_BODY_BYTES},
            "importance":{"type":"number","minimum":0,"maximum":1},
            "source":{"type":"string","maxLength":MAX_AUTHORITY_BYTES},
            "author":{"type":"string","maxLength":MAX_TITLE_BYTES},
            "date":{"type":"string","maxLength":MAX_TEMPORAL_VALUE_BYTES},
            "updated_at":{"type":["string","null"],"maxLength":MAX_TEMPORAL_VALUE_BYTES},
            "accessed_count":{"type":["integer","null"]},
            "times_injected":{"type":["integer","null"]},
            "effectiveness":{"type":["number","null"],"minimum":0,"maximum":1},
            "tags":{"type":["string","null"],"maxLength":MAX_BODY_BYTES},
            "scope":{"type":"string","maxLength":MAX_AUTHORITY_BYTES},
            "valid_from":{"type":["string","null"],"maxLength":MAX_TEMPORAL_VALUE_BYTES},
            "valid_until":{"type":["string","null"],"maxLength":MAX_TEMPORAL_VALUE_BYTES},
            "superseded_by":{"type":["string","null"],"maxLength":MAX_ID_BYTES},
            "fact_key":{"type":["string","null"],"maxLength":MAX_ID_BYTES},
            "git_refs":{"type":"array","maxItems":MAX_GIT_REFS,"items":object_schema(
                json!({
                    "commit_hash":{"type":"string","maxLength":MAX_GIT_HASH_BYTES},
                    "commit_subject":{"type":"string","maxLength":MAX_GIT_SUBJECT_BYTES}
                }),
                &["commit_hash", "commit_subject"]
            )}
        }),
        &["id", "kind", "title", "content", "scope"],
    )
}

fn input_schema(kind: ToolKind) -> Value {
    match kind {
        ToolKind::Ask => object_schema(
            json!({
                "question": {"type":"string","maxLength":MAX_QUERY_BYTES},
                "repo": {"type":"string","maxLength":MAX_AUTHORITY_BYTES,"description":"Absolute repository path"}
            }),
            &["question", "repo"],
        ),
        ToolKind::Index => object_schema(
            json!({"repo":{"type":"string","maxLength":MAX_AUTHORITY_BYTES,"description":"Absolute repository path"}}),
            &["repo"],
        ),
        ToolKind::Capture => object_schema(
            json!({
                "kind":{"type":"string","maxLength":128},
                "title":{"type":"string","maxLength":MAX_TITLE_BYTES},
                "content":{"type":"string","maxLength":MAX_BODY_BYTES},
                "importance":{"type":"number","minimum":0,"maximum":1},
                "scope":{"type":"string","maxLength":MAX_AUTHORITY_BYTES},
                "id":{"type":"string","maxLength":MAX_ID_BYTES},
                "valid_from":{"type":"string","maxLength":MAX_TEMPORAL_VALUE_BYTES},
                "fact_key":{"type":"string","maxLength":MAX_ID_BYTES},
                "supersedes":{"type":"string","maxLength":MAX_ID_BYTES}
            }),
            &["title", "content", "scope"],
        ),
        ToolKind::Import => object_schema(
            json!({
                "scope":{"type":"string","maxLength":MAX_AUTHORITY_BYTES},
                "rows":{"type":"array","maxItems":MAX_IMPORT_ROWS,"items":import_row_schema()}
            }),
            &["scope", "rows"],
        ),
        ToolKind::Search => object_schema(
            json!({
                "query":{"type":"string","maxLength":MAX_QUERY_BYTES},
                "limit":{"type":"integer","minimum":1,"maximum":MAX_RESULT_COUNT},
                "scope":{"type":"string","maxLength":MAX_AUTHORITY_BYTES},
                "type":{"oneOf":[{"type":"string"},{"type":"array","items":{"type":"string"}}]},
                "historical":{"type":"boolean"}
            }),
            &["query", "scope"],
        ),
        ToolKind::Get => object_schema(
            json!({
                "id":{"type":"string","maxLength":MAX_ID_BYTES},
                "scope":{"type":"string","maxLength":MAX_AUTHORITY_BYTES}
            }),
            &["id", "scope"],
        ),
        ToolKind::History => object_schema(
            json!({
                "id":{"type":"string","maxLength":MAX_ID_BYTES},
                "scope":{"type":"string","maxLength":MAX_AUTHORITY_BYTES},
                "limit":{"type":"integer","minimum":1,"maximum":MAX_HISTORY_PAGE_RECORDS},
                "cursor":{"type":"string","maxLength":MAX_ID_BYTES}
            }),
            &["id", "scope"],
        ),
        ToolKind::CommitLinks => object_schema(
            json!({
                "scope":{"type":"string","maxLength":MAX_AUTHORITY_BYTES},
                "commit":{"type":"string","maxLength":MAX_GIT_HASH_BYTES},
                "limit":{"type":"integer","minimum":1,"maximum":MAX_COMMIT_LINKS_PAGE_RECORDS},
                "cursor":{"type":"string","maxLength":MAX_ID_BYTES}
            }),
            &["scope", "commit"],
        ),
        ToolKind::Link => object_schema(
            json!({
                "commit":{"type":"string","maxLength":MAX_GIT_HASH_BYTES},
                "decision":{"type":"string","maxLength":MAX_ID_BYTES},
                "subject":{"type":"string","maxLength":MAX_GIT_SUBJECT_BYTES},
                "scope":{"type":"string","maxLength":MAX_AUTHORITY_BYTES}
            }),
            &["commit", "decision", "scope"],
        ),
        ToolKind::Feedback => object_schema(
            json!({
                "id":{"type":"string","maxLength":MAX_ID_BYTES},
                "helpful":{"type":"boolean"},
                "scope":{"type":"string","maxLength":MAX_AUTHORITY_BYTES}
            }),
            &["id", "helpful", "scope"],
        ),
    }
}

fn registry_tools() -> Vec<Value> {
    TOOL_SPECS
        .iter()
        .map(|spec| {
            let mut tool = json!({
                "name": spec.name,
                "description": spec.description,
                "inputSchema": input_schema(spec.kind)
            });
            if spec.kind == ToolKind::Import {
                tool["_meta"] = json!({"contract":RATIONALE_IMPORT_CONTRACT});
            }
            tool
        })
        .collect()
}

fn registry_digest() -> String {
    let bytes = serde_json::to_vec(&registry_tools()).expect("tool registry serializes");
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn jsonrpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message.into()}})
}

fn tool_response(id: Value, result: ToolResult) -> Value {
    let (payload, is_error) = match result {
        Ok(payload) => (payload, false),
        Err(error) => (error.payload, true),
    };
    let text = serde_json::to_string(&payload).unwrap_or_else(|error| {
        format!(
            "{{\"contract\":\"{MCP_ERROR_CONTRACT}\",\"status\":\"error\",\"code\":\"internal\",\"message\":\"serialize tool response: {error}\",\"retryable\":false}}"
        )
    });
    json!({
        "jsonrpc":"2.0",
        "id":id,
        "result":{"content":[{"type":"text","text":text}],"isError":is_error}
    })
}

fn tool_wire_size(payload: &Value) -> std::result::Result<usize, ToolError> {
    serde_json::to_vec(&tool_response(Value::Null, Ok(payload.clone())))
        .map(|bytes| bytes.len())
        .map_err(ToolError::internal)
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

pub fn serve() -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let store = db::Store::open_default()?;
    serve_io(&store, stdin.lock(), &mut stdout, server_now_epoch)
}

fn serve_io(
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

fn dispatch_tool(store: &db::Store, name: &str, args: &Value, as_of: i64) -> ToolResult {
    let Some(spec) = TOOL_SPECS.iter().find(|spec| spec.name == name) else {
        return Err(ToolError::new(
            "unknown_tool",
            format!("unknown tool `{name}`"),
        ));
    };
    let schema = input_schema(spec.kind);
    let allowed = schema["properties"]
        .as_object()
        .expect("registry properties are objects");
    for key in args
        .as_object()
        .expect("dispatch receives object arguments")
        .keys()
    {
        if !allowed.contains_key(key) {
            return Err(ToolError::new(
                "invalid_arguments",
                format!("unknown argument `{key}` for tool `{name}`"),
            ));
        }
    }
    match spec.kind {
        ToolKind::Ask => ask_tool(store, args),
        ToolKind::Index => index_tool(store, args),
        ToolKind::Capture => capture_tool(store, args),
        ToolKind::Import => import_tool(store, args),
        ToolKind::Search => search_tool(store, args),
        ToolKind::Get => with_as_of(get_tool(store, args, as_of), as_of),
        ToolKind::History => with_as_of(history_tool(store, args, as_of), as_of),
        ToolKind::CommitLinks => commit_links_tool(store, args),
        ToolKind::Link => link_tool(store, args),
        ToolKind::Feedback => feedback_tool(store, args),
    }
}

fn with_as_of(mut result: ToolResult, as_of: i64) -> ToolResult {
    if let Err(error) = &mut result {
        if let Some(object) = error.payload.as_object_mut() {
            object
                .entry("as_of")
                .or_insert_with(|| Value::String(db::epoch_to_iso(as_of)));
        }
    }
    result
}

fn required_string<'a>(
    args: &'a Value,
    key: &str,
    max_bytes: usize,
) -> std::result::Result<&'a str, ToolError> {
    let Some(value) = args.get(key).and_then(Value::as_str) else {
        return Err(ToolError::new(
            "invalid_arguments",
            format!("`{key}` is required and must be a string"),
        ));
    };
    if value.is_empty() {
        return Err(ToolError::new(
            "invalid_arguments",
            format!("`{key}` must not be empty"),
        ));
    }
    if value.len() > max_bytes {
        return Err(ToolError::new(
            "limit_exceeded",
            format!("`{key}` exceeds {max_bytes} UTF-8 bytes"),
        ));
    }
    Ok(value)
}

fn required_exact_non_blank_string<'a>(
    args: &'a Value,
    key: &str,
    max_bytes: usize,
) -> std::result::Result<&'a str, ToolError> {
    let Some(value) = args.get(key).and_then(Value::as_str) else {
        return Err(ToolError::new(
            "invalid_arguments",
            format!("`{key}` is required and must be a string"),
        ));
    };
    if value.len() > max_bytes {
        return Err(ToolError::new(
            "limit_exceeded",
            format!("`{key}` exceeds {max_bytes} UTF-8 bytes"),
        ));
    }
    if value.trim().is_empty() {
        return Err(ToolError::new(
            "invalid_arguments",
            format!("`{key}` must not be empty"),
        ));
    }
    Ok(value)
}

fn optional_string<'a>(
    args: &'a Value,
    key: &str,
    max_bytes: usize,
) -> std::result::Result<Option<&'a str>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.len() <= max_bytes => Ok(Some(value)),
        Some(Value::String(_)) => Err(ToolError::new(
            "limit_exceeded",
            format!("`{key}` exceeds {max_bytes} UTF-8 bytes"),
        )),
        Some(_) => Err(ToolError::new(
            "invalid_arguments",
            format!("`{key}` must be a string"),
        )),
    }
}

fn explicit_repo(args: &Value) -> std::result::Result<&str, ToolError> {
    let repo = required_string(args, "repo", MAX_AUTHORITY_BYTES)?;
    if !Path::new(repo).is_absolute() {
        return Err(ToolError::new(
            "repository_authority_required",
            "`repo` must be an explicit absolute path",
        ));
    }
    Ok(repo)
}

fn explicit_scope(args: &Value) -> std::result::Result<&str, ToolError> {
    required_string(args, "scope", MAX_AUTHORITY_BYTES).map_err(|error| {
        if error.payload["code"] == "invalid_arguments" {
            ToolError::new(
                "scope_authority_required",
                "an explicit non-empty `scope` is required",
            )
        } else {
            error
        }
    })
}

fn kinds_from(args: &Value) -> std::result::Result<Vec<String>, ToolError> {
    let Some(value) = args.get("type") else {
        return Ok(Vec::new());
    };
    let kinds = match value {
        Value::String(value) => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect(),
        Value::Array(values) => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                let Some(value) = value.as_str() else {
                    return Err(ToolError::new(
                        "invalid_arguments",
                        "every `type` array entry must be a string",
                    ));
                };
                if !value.trim().is_empty() {
                    out.push(value.trim().to_owned());
                }
            }
            out
        }
        _ => {
            return Err(ToolError::new(
                "invalid_arguments",
                "`type` must be a string or string array",
            ))
        }
    };
    Ok(kinds)
}

fn ask_tool(store: &db::Store, args: &Value) -> ToolResult {
    let question = required_string(args, "question", MAX_QUERY_BYTES)?;
    let repo = explicit_repo(args)?;
    let repo = miner::resolve_repo(Some(repo.to_owned())).map_err(ToolError::internal)?;
    let scope = store::scope_for(&repo);
    if store.count_for_scope(&scope).map_err(ToolError::internal)? == 0 {
        let decisions = miner::mine(&repo).map_err(ToolError::internal)?;
        store
            .import_decisions(&scope, &decisions)
            .map_err(ToolError::internal)?;
    }
    let records = store
        .search_records(question, &[scope.as_str()], &[], 5)
        .map_err(ToolError::internal)?;
    Ok(json!({"status":"ok","scope":scope,"results":previews(records)}))
}

fn index_tool(store: &db::Store, args: &Value) -> ToolResult {
    let repo = explicit_repo(args)?;
    let repo = miner::resolve_repo(Some(repo.to_owned())).map_err(ToolError::internal)?;
    let scope = store::scope_for(&repo);
    let decisions = miner::mine(&repo).map_err(ToolError::internal)?;
    let count = decisions.len();
    store
        .import_decisions(&scope, &decisions)
        .map_err(ToolError::internal)?;
    Ok(json!({"status":"ok","scope":scope,"indexed":count}))
}

fn capture_tool(store: &db::Store, args: &Value) -> ToolResult {
    let scope = explicit_scope(args)?;
    let title = required_string(args, "title", MAX_TITLE_BYTES)?;
    let content = required_string(args, "content", MAX_BODY_BYTES)?;
    let kind = optional_string(args, "kind", 128)?.unwrap_or("decision");
    let importance = match args.get("importance") {
        None => 0.5,
        Some(value) => value
            .as_f64()
            .filter(|value| (0.0..=1.0).contains(value))
            .ok_or_else(|| {
                ToolError::new(
                    "invalid_arguments",
                    "`importance` must be a finite number from 0 to 1",
                )
            })?,
    };
    let id = optional_string(args, "id", MAX_ID_BYTES)?;
    let valid_from = optional_string(args, "valid_from", MAX_TEMPORAL_VALUE_BYTES)?;
    let fact_key = optional_string(args, "fact_key", MAX_ID_BYTES)?;
    let supersedes = optional_string(args, "supersedes", MAX_ID_BYTES)?;
    if let Some(valid_from) = valid_from {
        if !store
            .temporal_value_is_valid(valid_from)
            .map_err(ToolError::internal)?
        {
            return Err(ToolError::new(
                "invalid_arguments",
                "`valid_from` must be a valid ISO timestamp",
            ));
        }
    }
    if let Some(supersedes) = supersedes.filter(|value| !value.is_empty()) {
        if !store
            .record_belongs_to_scope(supersedes, scope)
            .map_err(ToolError::internal)?
        {
            return Err(ToolError::new(
                "scope_mismatch",
                format!("superseded record `{supersedes}` is not in scope `{scope}`"),
            ));
        }
    }
    let decision = store::Decision {
        subject: title.to_owned(),
        body: content.to_owned(),
        kind: kind.to_owned(),
        importance,
        source: "capture".to_owned(),
        ..store::Decision::default()
    };
    let id = match id.filter(|value| !value.is_empty()) {
        Some(id) => store.capture_external(&decision, scope, id, valid_from, fact_key, supersedes),
        None => store.capture(&decision, scope, supersedes),
    }
    .map_err(|error| {
        if error.downcast_ref::<CurrentRecordErrorCode>()
            == Some(&CurrentRecordErrorCode::InvalidTemporalData)
        {
            ToolError::new(
                "invalid_arguments",
                "supersession predecessor has invalid temporal data",
            )
        } else {
            ToolError::internal(error)
        }
    })?;
    Ok(json!({"status":"ok","id":id,"scope":scope}))
}

fn validate_import_row(
    store: &db::Store,
    row: &ExternalDecision,
    scope: &str,
) -> std::result::Result<(), ToolError> {
    if row.scope != scope {
        return Err(ToolError::new(
            "scope_mismatch",
            format!(
                "record `{}` does not belong to explicit scope `{scope}`",
                row.id
            ),
        ));
    }
    for (field, value, limit) in [
        ("id", row.id.as_str(), MAX_ID_BYTES),
        ("kind", row.kind.as_str(), 128),
        ("title", row.title.as_str(), MAX_TITLE_BYTES),
        ("content", row.content.as_str(), MAX_BODY_BYTES),
        ("source", row.source.as_str(), MAX_AUTHORITY_BYTES),
        ("author", row.author.as_str(), MAX_TITLE_BYTES),
        ("date", row.date.as_str(), MAX_TEMPORAL_VALUE_BYTES),
    ] {
        if value.len() > limit {
            return Err(ToolError::new(
                "limit_exceeded",
                format!(
                    "record `{}` field `{field}` exceeds {limit} UTF-8 bytes",
                    row.id
                ),
            ));
        }
    }
    if row.id.is_empty() || row.title.is_empty() || row.content.is_empty() {
        return Err(ToolError::new(
            "invalid_arguments",
            "import record id, title, and content must not be empty",
        ));
    }
    if row.kind.is_empty() {
        return Err(ToolError::new(
            "invalid_arguments",
            format!("record `{}` kind must not be empty", row.id),
        ));
    }
    if !row.importance.is_finite() || !(0.0..=1.0).contains(&row.importance) {
        return Err(ToolError::new(
            "invalid_arguments",
            format!("record `{}` has invalid importance", row.id),
        ));
    }
    if row
        .effectiveness
        .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
    {
        return Err(ToolError::new(
            "invalid_arguments",
            format!("record `{}` has invalid effectiveness", row.id),
        ));
    }
    for (field, value, limit) in [
        (
            "updated_at",
            row.updated_at.as_deref(),
            MAX_TEMPORAL_VALUE_BYTES,
        ),
        ("tags", row.tags.as_deref(), MAX_BODY_BYTES),
        (
            "valid_from",
            row.valid_from.as_deref(),
            MAX_TEMPORAL_VALUE_BYTES,
        ),
        (
            "valid_until",
            row.valid_until.as_deref(),
            MAX_TEMPORAL_VALUE_BYTES,
        ),
        ("superseded_by", row.superseded_by.as_deref(), MAX_ID_BYTES),
        ("fact_key", row.fact_key.as_deref(), MAX_ID_BYTES),
    ] {
        if value.is_some_and(|value| value.len() > limit) {
            return Err(ToolError::new(
                "limit_exceeded",
                format!(
                    "record `{}` field `{field}` exceeds {limit} UTF-8 bytes",
                    row.id
                ),
            ));
        }
    }
    if row.git_refs.len() > MAX_GIT_REFS {
        return Err(ToolError::new(
            "limit_exceeded",
            format!("record `{}` exceeds {MAX_GIT_REFS} Git references", row.id),
        ));
    }
    for git_ref in &row.git_refs {
        if git_ref.commit_hash.len() > MAX_GIT_HASH_BYTES
            || git_ref.commit_subject.len() > MAX_GIT_SUBJECT_BYTES
        {
            return Err(ToolError::new(
                "limit_exceeded",
                format!("record `{}` contains an oversized Git reference", row.id),
            ));
        }
    }
    for (field, value) in [
        ("valid_from", row.valid_from.as_deref()),
        ("valid_until", row.valid_until.as_deref()),
    ] {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            if !store
                .temporal_value_is_valid(value)
                .map_err(ToolError::internal)?
            {
                return Err(ToolError::new(
                    "invalid_arguments",
                    format!("record `{}` has invalid `{field}`", row.id),
                ));
            }
        }
    }
    if let (Some(valid_from), Some(valid_until)) = (
        row.valid_from.as_deref().filter(|value| !value.is_empty()),
        row.valid_until.as_deref().filter(|value| !value.is_empty()),
    ) {
        let from: Option<i64> = store
            .temporal_epoch(valid_from)
            .map_err(ToolError::internal)?;
        let until: Option<i64> = store
            .temporal_epoch(valid_until)
            .map_err(ToolError::internal)?;
        if from.zip(until).is_some_and(|(from, until)| from >= until) {
            return Err(ToolError::new(
                "invalid_arguments",
                format!("record `{}` has a non-positive validity interval", row.id),
            ));
        }
    }
    Ok(())
}

fn validate_import_shape(rows: &Value) -> std::result::Result<(), ToolError> {
    let Some(rows) = rows.as_array() else {
        return Ok(());
    };
    let schema = import_row_schema();
    let allowed = schema["properties"]
        .as_object()
        .expect("import row properties are objects");
    let git_ref_allowed = schema["properties"]["git_refs"]["items"]["properties"]
        .as_object()
        .expect("Git reference properties are objects");
    for (index, row) in rows.iter().enumerate() {
        let Some(row) = row.as_object() else {
            continue;
        };
        for key in row.keys() {
            if !allowed.contains_key(key) {
                return Err(ToolError::new(
                    "invalid_arguments",
                    format!("unknown field `{key}` in import row {index}"),
                ));
            }
        }
        if let Some(git_refs) = row.get("git_refs").and_then(Value::as_array) {
            for (ref_index, git_ref) in git_refs.iter().enumerate() {
                let Some(git_ref) = git_ref.as_object() else {
                    continue;
                };
                for key in git_ref.keys() {
                    if !git_ref_allowed.contains_key(key) {
                        return Err(ToolError::new(
                            "invalid_arguments",
                            format!(
                                "unknown field `{key}` in import row {index} Git reference {ref_index}"
                            ),
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

fn import_tool(store: &db::Store, args: &Value) -> ToolResult {
    let scope = explicit_scope(args)?;
    let rows_value = args
        .get("rows")
        .ok_or_else(|| ToolError::new("invalid_arguments", "`rows` is required"))?;
    let aggregate = serde_json::to_vec(args).map_err(ToolError::internal)?;
    if aggregate.len() > MAX_IMPORT_BYTES {
        return Err(ToolError::new(
            "limit_exceeded",
            format!("aggregate import exceeds {MAX_IMPORT_BYTES} UTF-8 bytes"),
        ));
    }
    validate_import_shape(rows_value)?;
    let rows: Vec<ExternalDecision> = serde_json::from_value(rows_value.clone())
        .map_err(|error| ToolError::new("invalid_arguments", format!("invalid rows: {error}")))?;
    if rows.len() > MAX_IMPORT_ROWS {
        return Err(ToolError::new(
            "limit_exceeded",
            format!("import exceeds {MAX_IMPORT_ROWS} records"),
        ));
    }
    for row in &rows {
        validate_import_row(store, row, scope)?;
    }
    let imported = store.import_external(&rows).map_err(|error| {
        if error.downcast_ref::<RecordIdentityConflict>().is_some() {
            ToolError::resolution(json!({
                "contract":RATIONALE_IMPORT_CONTRACT,
                "status":"error",
                "code":"identity_conflict",
                "message":"record identity conflicts with sealed evidence",
                "retryable":false
            }))
        } else {
            ToolError::internal(error)
        }
    })?;
    Ok(json!({
        "contract":RATIONALE_IMPORT_CONTRACT,
        "status":"ok",
        "scope":scope,
        "imported":imported
    }))
}

fn search_tool(store: &db::Store, args: &Value) -> ToolResult {
    let scope = explicit_scope(args)?;
    let query = required_string(args, "query", MAX_QUERY_BYTES)?;
    let limit = match args.get("limit") {
        None => 10,
        Some(value) => value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| {
                ToolError::new("invalid_arguments", "`limit` must be a positive integer")
            })?,
    };
    if !(1..=MAX_RESULT_COUNT).contains(&limit) {
        return Err(ToolError::new(
            "limit_exceeded",
            format!("`limit` must be from 1 to {MAX_RESULT_COUNT}"),
        ));
    }
    let historical = match args.get("historical") {
        None => false,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| ToolError::new("invalid_arguments", "`historical` must be a boolean"))?,
    };
    let kinds = kinds_from(args)?;
    let records = store
        .search_records_with(query, &[scope], &kinds, limit, historical)
        .map_err(ToolError::internal)?;
    Ok(json!({"status":"ok","scope":scope,"results":previews(records)}))
}

fn bounded_preview(content: &str) -> (&str, bool) {
    if content.len() <= MAX_PREVIEW_BYTES {
        return (content, false);
    }
    let mut end = MAX_PREVIEW_BYTES;
    while end > 0 && !content.is_char_boundary(end) {
        end -= 1;
    }
    (&content[..end], true)
}

fn previews(records: Vec<Record>) -> Vec<Value> {
    records
        .into_iter()
        .map(|record| {
            let (preview, preview_truncated) = bounded_preview(&record.content);
            json!({
                "id":record.id,
                "kind":record.kind,
                "title":record.title,
                "preview":preview,
                "preview_truncated":preview_truncated,
                "source":record.source,
                "author":record.author,
                "date":record.date
            })
        })
        .collect()
}

fn get_tool(store: &db::Store, args: &Value, as_of: i64) -> ToolResult {
    let scope = explicit_scope(args)?;
    let id = required_string(args, "id", MAX_ID_BYTES)?;
    let resolution = store
        .get_current_evidence_in_scope_at(id, scope, as_of, MAX_SUPERSESSION_CHAIN)
        .map_err(ToolError::internal)?;
    let payload = serde_json::to_value(&resolution).map_err(ToolError::internal)?;
    if tool_wire_size(&payload)? > MAX_RESPONSE_BYTES {
        return Err(ToolError::new(
            "response_too_large",
            "exact record cannot be returned within the response byte limit",
        ));
    }
    match resolution {
        CurrentRecordResolution::Ok { .. } => Ok(payload),
        CurrentRecordResolution::Error { .. } => Err(ToolError::resolution(payload)),
    }
}

fn history_tool(store: &db::Store, args: &Value, as_of: i64) -> ToolResult {
    let scope = explicit_scope(args)?;
    let id = required_string(args, "id", MAX_ID_BYTES)?;
    let cursor = optional_string(args, "cursor", MAX_ID_BYTES)?;
    let limit = match args.get("limit") {
        None => MAX_HISTORY_PAGE_RECORDS,
        Some(value) => {
            let Some(limit) = value.as_u64().and_then(|value| usize::try_from(value).ok()) else {
                return Err(ToolError::new(
                    "invalid_arguments",
                    "`limit` must be an integer",
                ));
            };
            if !(1..=MAX_HISTORY_PAGE_RECORDS).contains(&limit) {
                return Err(ToolError::new(
                    "limit_exceeded",
                    format!("`limit` must be from 1 to {MAX_HISTORY_PAGE_RECORDS}"),
                ));
            }
            limit
        }
    };

    let resolution = store
        .get_rationale_history_at(id, scope, cursor, limit, as_of, MAX_SUPERSESSION_CHAIN)
        .map_err(ToolError::internal)?;
    let payload = serde_json::to_value(&resolution).map_err(ToolError::internal)?;
    if tool_wire_size(&payload)? > MAX_RESPONSE_BYTES {
        let oversized = RationaleHistoryResolution::Error {
            contract: RATIONALE_HISTORY_CONTRACT,
            as_of: db::epoch_to_iso(as_of),
            requested_id: id.to_owned(),
            code: RationaleHistoryErrorCode::ResponseTooLarge,
            message: "exact history page cannot be returned within the response byte limit"
                .to_owned(),
            retryable: false,
        };
        return Err(ToolError::resolution(
            serde_json::to_value(oversized).map_err(ToolError::internal)?,
        ));
    }
    match resolution {
        RationaleHistoryResolution::Ok { .. } => Ok(payload),
        RationaleHistoryResolution::Error { .. } => Err(ToolError::resolution(payload)),
    }
}

fn commit_links_tool(store: &db::Store, args: &Value) -> ToolResult {
    let scope = required_exact_non_blank_string(args, "scope", MAX_AUTHORITY_BYTES)?;
    let commit = required_exact_non_blank_string(args, "commit", MAX_GIT_HASH_BYTES)?;
    let cursor = match optional_string(args, "cursor", MAX_ID_BYTES)? {
        Some(cursor) if cursor.trim().is_empty() => {
            return Err(ToolError::new(
                "invalid_arguments",
                "`cursor` must not be empty",
            ));
        }
        Some(cursor) => Some(cursor),
        None => None,
    };
    let limit = match args.get("limit") {
        None => MAX_COMMIT_LINKS_PAGE_RECORDS,
        Some(value) => {
            let Some(limit) = value.as_u64().and_then(|value| usize::try_from(value).ok()) else {
                return Err(ToolError::new(
                    "invalid_arguments",
                    "`limit` must be an integer",
                ));
            };
            if !(1..=MAX_COMMIT_LINKS_PAGE_RECORDS).contains(&limit) {
                return Err(ToolError::new(
                    "limit_exceeded",
                    format!("`limit` must be from 1 to {MAX_COMMIT_LINKS_PAGE_RECORDS}"),
                ));
            }
            limit
        }
    };

    let resolution = store
        .get_commit_links(scope, commit, cursor, limit)
        .map_err(ToolError::internal)?;
    let payload = serde_json::to_value(&resolution).map_err(ToolError::internal)?;
    if tool_wire_size(&payload)? > MAX_RESPONSE_BYTES {
        let oversized = CommitLinksResolution::Error {
            contract: COMMIT_LINKS_CONTRACT,
            code: CommitLinksErrorCode::ResponseTooLarge,
            message: "commit links response cannot be returned within the response byte limit"
                .to_owned(),
            retryable: false,
        };
        return Err(ToolError::resolution(
            serde_json::to_value(oversized).map_err(ToolError::internal)?,
        ));
    }
    match resolution {
        CommitLinksResolution::Ok { .. } => Ok(payload),
        CommitLinksResolution::Error { .. } => Err(ToolError::resolution(payload)),
    }
}

fn link_tool(store: &db::Store, args: &Value) -> ToolResult {
    let scope = explicit_scope(args)?;
    let decision = required_string(args, "decision", MAX_ID_BYTES)?;
    let commit = required_string(args, "commit", MAX_GIT_HASH_BYTES)?;
    let subject = optional_string(args, "subject", MAX_GIT_SUBJECT_BYTES)?.unwrap_or("");
    if !store
        .record_belongs_to_scope(decision, scope)
        .map_err(ToolError::internal)?
    {
        return Err(ToolError::new(
            "not_found",
            format!("record `{decision}` was not found in scope `{scope}`"),
        ));
    }
    store
        .link_git(decision, commit, subject)
        .map_err(ToolError::internal)?;
    Ok(json!({"status":"ok","scope":scope,"decision":decision,"commit":commit}))
}

fn feedback_tool(store: &db::Store, args: &Value) -> ToolResult {
    let scope = explicit_scope(args)?;
    let id = required_string(args, "id", MAX_ID_BYTES)?;
    let helpful = args
        .get("helpful")
        .and_then(Value::as_bool)
        .ok_or_else(|| ToolError::new("invalid_arguments", "`helpful` must be a boolean"))?;
    if !store
        .record_belongs_to_scope(id, scope)
        .map_err(ToolError::internal)?
    {
        return Err(ToolError::new(
            "not_found",
            format!("record `{id}` was not found in scope `{scope}`"),
        ));
    }
    let effectiveness = store
        .feedback(id, helpful)
        .map_err(ToolError::internal)?
        .ok_or_else(|| ToolError::new("not_current", format!("record `{id}` is not current")))?;
    Ok(json!({
        "status":"ok",
        "scope":scope,
        "id":id,
        "helpful":helpful,
        "effectiveness":effectiveness
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> db::Store {
        let path = std::env::temp_dir().join(format!(
            "open-why-mcp-unit-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        db::Store::open_with_store_instance_id(&path, "provider:mcp-unit").unwrap()
    }

    #[test]
    fn registry_names_are_unique_and_dispatch_uses_the_same_registry() {
        let mut names: Vec<&str> = TOOL_SPECS.iter().map(|spec| spec.name).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count);

        let store = temp_store();
        for spec in TOOL_SPECS {
            let result = dispatch_tool(&store, spec.name, &json!({}), 0);
            let error = result.expect_err("empty arguments must be typed errors");
            assert_ne!(error.payload["code"], "unknown_tool");
        }
        let unknown = dispatch_tool(&store, "open-why_hidden", &json!({}), 0).unwrap_err();
        assert_eq!(unknown.payload["code"], "unknown_tool");
    }

    #[test]
    fn temporal_schema_and_runtime_share_the_canonical_byte_limit() {
        let expected = json!(MAX_TEMPORAL_VALUE_BYTES);
        let capture = input_schema(ToolKind::Capture);
        assert_eq!(capture["properties"]["valid_from"]["maxLength"], expected);
        let import = import_row_schema();
        for field in ["date", "updated_at", "valid_from", "valid_until"] {
            assert_eq!(import["properties"][field]["maxLength"], expected);
        }
        assert_eq!(registry_digest(), "b37082e4965e9009");

        let store = temp_store();
        let boundary = format!("2026-01-01T00:00:00.{}Z", "1".repeat(107));
        assert_eq!(boundary.len(), MAX_TEMPORAL_VALUE_BYTES);
        assert!(dispatch_tool(
            &store,
            "open-why_capture",
            &json!({
                "id":"mcp-time-boundary",
                "title":"MCP time boundary",
                "content":"canonical boundary",
                "scope":"scope-a",
                "valid_from":boundary
            }),
            0,
        )
        .is_ok());

        let over_bound = format!("2026-01-01T00:00:00.{}Z", "1".repeat(108));
        let oversized = dispatch_tool(
            &store,
            "open-why_capture",
            &json!({
                "id":"mcp-time-over-bound",
                "title":"MCP time over bound",
                "content":"must not persist",
                "scope":"scope-a",
                "valid_from":over_bound
            }),
            0,
        )
        .unwrap_err();
        assert_eq!(oversized.payload["code"], "limit_exceeded");

        let non_ascii = "é".repeat(MAX_TEMPORAL_VALUE_BYTES / 2);
        assert_eq!(non_ascii.len(), MAX_TEMPORAL_VALUE_BYTES);
        let noncanonical = dispatch_tool(
            &store,
            "open-why_capture",
            &json!({
                "id":"mcp-time-non-ascii",
                "title":"MCP time non ASCII",
                "content":"must not persist",
                "scope":"scope-a",
                "valid_from":non_ascii
            }),
            0,
        )
        .unwrap_err();
        assert_eq!(noncanonical.payload["code"], "invalid_arguments");
        assert_eq!(store.count_for_scope("scope-a").unwrap(), 1);
    }

    #[test]
    fn malformed_json_and_invalid_arguments_are_protocol_or_tool_errors() {
        let store = temp_store();
        let input = b"{bad json}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"open-why_get\",\"arguments\":{}}}\n";
        let mut output = Vec::new();
        serve_io(&store, &input[..], &mut output, || 1_700_000_000).unwrap();
        let lines: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines[0]["error"]["code"], -32700);
        assert_eq!(lines[1]["result"]["isError"], true);
        let payload: Value =
            serde_json::from_str(lines[1]["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(payload["code"], "scope_authority_required");
    }

    #[test]
    fn server_clock_is_evaluated_for_each_request() {
        let store = temp_store();
        store
            .import_external(&[ExternalDecision {
                id: "clock-record".to_owned(),
                kind: "decision".to_owned(),
                title: "clock record".to_owned(),
                content: "complete rationale".to_owned(),
                importance: 0.5,
                source: "synthetic".to_owned(),
                author: "tester".to_owned(),
                date: "2026-01-01".to_owned(),
                updated_at: None,
                accessed_count: None,
                times_injected: None,
                effectiveness: None,
                tags: None,
                scope: "scope-a".to_owned(),
                valid_from: None,
                valid_until: None,
                superseded_by: None,
                fact_key: None,
                git_refs: Vec::new(),
            }])
            .unwrap();
        let request = |id| {
            json!({
                "jsonrpc":"2.0",
                "id":id,
                "method":"tools/call",
                "params":{
                    "name":"open-why_get",
                    "arguments":{"id":"clock-record","scope":"scope-a"}
                }
            })
        };
        let input = format!("{}\n{}\n", request(1), request(2));
        let now = std::cell::Cell::new(1_700_000_000_i64);
        let mut output = Vec::new();
        serve_io(&store, input.as_bytes(), &mut output, || {
            let instant = now.get();
            now.set(instant + 60);
            instant
        })
        .unwrap();

        let payloads: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| {
                let response: Value = serde_json::from_str(line).unwrap();
                serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                    .unwrap()
            })
            .collect();
        assert_eq!(payloads[0]["as_of"], "2023-11-14T22:13:20Z");
        assert_eq!(payloads[1]["as_of"], "2023-11-14T22:14:20Z");
    }

    #[test]
    fn exact_get_matches_scoped_store_resolution_for_a_foreign_successor() {
        let store = temp_store();
        let row = |id: &str, successor: Option<&str>, scope: &str| ExternalDecision {
            id: id.to_owned(),
            kind: "decision".to_owned(),
            title: format!("record {id}"),
            content: format!("private body for {id}"),
            importance: 0.5,
            source: "synthetic".to_owned(),
            author: "tester".to_owned(),
            date: "2026-01-01".to_owned(),
            updated_at: None,
            accessed_count: None,
            times_injected: None,
            effectiveness: None,
            tags: None,
            scope: scope.to_owned(),
            valid_from: Some("2026-01-01T00:00:00Z".to_owned()),
            valid_until: successor.map(|_| "2026-02-01T00:00:00Z".to_owned()),
            superseded_by: successor.map(str::to_owned),
            fact_key: None,
            git_refs: Vec::new(),
        };
        store
            .import_external(&[
                row("root", Some("middle"), "scope-a"),
                row("middle", Some("foreign-successor"), "scope-a"),
                row("foreign-successor", None, "scope-b"),
            ])
            .unwrap();
        let as_of = 1_772_323_200;
        let direct = serde_json::to_value(
            store
                .get_current_evidence_in_scope_at("root", "scope-a", as_of, 64)
                .unwrap(),
        )
        .unwrap();
        let error = dispatch_tool(
            &store,
            "open-why_get",
            &json!({"id":"root","scope":"scope-a"}),
            as_of,
        )
        .unwrap_err();

        assert_eq!(error.payload, direct);
        assert_eq!(error.payload["code"], "broken_chain");
        assert_eq!(
            error.payload["message"],
            "supersession chain is unavailable in the requested scope"
        );
        assert!(!serde_json::to_string(&error.payload)
            .unwrap()
            .contains("foreign-successor"));
    }

    #[test]
    fn preview_is_utf8_safe_bounded_and_contains_stable_id() {
        let content = format!("{}x", "é".repeat(MAX_PREVIEW_BYTES));
        let record = Record {
            id: "stable-id".into(),
            kind: "decision".into(),
            title: "Unicode".into(),
            content,
            importance: 0.5,
            source: "synthetic".into(),
            author: "tester".into(),
            date: "2026-01-01".into(),
            commit_sha: String::new(),
            scope: "scope-a".into(),
            superseded_by: None,
            valid_from: None,
            valid_until: None,
            updated_at: String::new(),
            access_count: 0,
            effectiveness: 0.5,
            embedding: None,
        };
        let preview = previews(vec![record]).remove(0);
        assert_eq!(preview["id"], "stable-id");
        assert!(preview["preview"].as_str().unwrap().len() <= MAX_PREVIEW_BYTES);
        assert_eq!(preview["preview_truncated"], true);
    }

    #[test]
    fn integration_bounds_and_unknown_arguments_fail_before_effects() {
        let store = temp_store();
        let oversized_query = "q".repeat(MAX_QUERY_BYTES + 1);
        let query_error = dispatch_tool(
            &store,
            "open-why_search",
            &json!({"query":oversized_query,"scope":"scope-a"}),
            0,
        )
        .unwrap_err();
        assert_eq!(query_error.payload["code"], "limit_exceeded");

        let limit_error = dispatch_tool(
            &store,
            "open-why_search",
            &json!({"query":"bounded","scope":"scope-a","limit":101}),
            0,
        )
        .unwrap_err();
        assert_eq!(limit_error.payload["code"], "limit_exceeded");

        let authority_error = dispatch_tool(
            &store,
            "open-why_index",
            &json!({"repo":"relative/path"}),
            0,
        )
        .unwrap_err();
        assert_eq!(
            authority_error.payload["code"],
            "repository_authority_required"
        );

        let unknown_argument = dispatch_tool(
            &store,
            "open-why_get",
            &json!({"id":"id","scope":"scope-a","unexpected_field":"forbidden"}),
            0,
        )
        .unwrap_err();
        assert_eq!(unknown_argument.payload["code"], "invalid_arguments");

        let oversized_body = "x".repeat(MAX_BODY_BYTES + 1);
        let capture_error = dispatch_tool(
            &store,
            "open-why_capture",
            &json!({"title":"title","content":oversized_body,"scope":"scope-a"}),
            0,
        )
        .unwrap_err();
        assert_eq!(capture_error.payload["code"], "limit_exceeded");
        assert_eq!(store.count_for_scope("scope-a").unwrap(), 0);

        let oversized_import = "x".repeat(MAX_IMPORT_BYTES);
        let import_error = dispatch_tool(
            &store,
            "open-why_import",
            &json!({"scope":"scope-a","rows":[{
                "id":"oversized-import","kind":"decision","title":"title",
                "content":oversized_import,"importance":0.5,"source":"test",
                "author":"tester","date":"2026-01-01","scope":"scope-a",
                "git_refs":[]
            }]}),
            0,
        )
        .unwrap_err();
        assert_eq!(import_error.payload["code"], "limit_exceeded");
        assert_eq!(store.count_for_scope("scope-a").unwrap(), 0);

        let git_refs: Vec<Value> = (0..=MAX_GIT_REFS)
            .map(|index| json!({"commit_hash":format!("{index:040x}"),"commit_subject":"subject"}))
            .collect();
        let refs_error = dispatch_tool(
            &store,
            "open-why_import",
            &json!({"scope":"scope-a","rows":[{
                "id":"too-many-refs","kind":"decision","title":"title",
                "content":"body","importance":0.5,"source":"test",
                "author":"tester","date":"2026-01-01","scope":"scope-a",
                "git_refs":git_refs
            }]}),
            0,
        )
        .unwrap_err();
        assert_eq!(refs_error.payload["code"], "limit_exceeded");
        assert_eq!(store.count_for_scope("scope-a").unwrap(), 0);

        let temporal_error = dispatch_tool(
            &store,
            "open-why_import",
            &json!({"scope":"scope-a","rows":[{
                "id":"inverted","kind":"decision","title":"title","content":"body",
                "scope":"scope-a","valid_from":"2026-02-01T00:00:00Z",
                "valid_until":"2026-01-01T00:00:00Z"
            }]}),
            0,
        )
        .unwrap_err();
        assert_eq!(temporal_error.payload["code"], "invalid_arguments");
        assert_eq!(store.count_for_scope("scope-a").unwrap(), 0);

        let unknown_row_field = dispatch_tool(
            &store,
            "open-why_import",
            &json!({"scope":"scope-a","rows":[{
                "id":"unknown-field","kind":"decision","title":"title","content":"body",
                "scope":"scope-a","unexpected_field":"forbidden"
            }]}),
            0,
        )
        .unwrap_err();
        assert_eq!(unknown_row_field.payload["code"], "invalid_arguments");
        assert_eq!(store.count_for_scope("scope-a").unwrap(), 0);
    }

    #[test]
    fn public_import_reports_exact_replay_and_typed_identity_conflict() {
        let store = temp_store();
        let original = json!({
            "id":"stable-import","kind":"decision","title":"stable title",
            "content":"stable body","importance":0.5,"source":"test",
            "author":"tester","date":"2026-01-01","scope":"scope-a",
            "git_refs":[{"commit_hash":"original","commit_subject":"Original"}]
        });
        let args = json!({"scope":"scope-a","rows":[original.clone()]});
        assert_eq!(
            dispatch_tool(&store, "open-why_import", &args, 0).unwrap()["imported"],
            1
        );
        assert_eq!(
            dispatch_tool(&store, "open-why_import", &args, 0).unwrap()["imported"],
            0
        );

        let mut conflict = original;
        conflict["content"] = json!("changed body");
        conflict["git_refs"] = json!([{
            "commit_hash":"must-not-append","commit_subject":"Must not append"
        }]);
        let error = dispatch_tool(
            &store,
            "open-why_import",
            &json!({"scope":"scope-a","rows":[conflict]}),
            0,
        )
        .unwrap_err();
        assert_eq!(error.payload["contract"], RATIONALE_IMPORT_CONTRACT);
        assert_eq!(error.payload["code"], "identity_conflict");
        assert_eq!(
            error.payload["message"],
            "record identity conflicts with sealed evidence"
        );
        assert!(!error.payload["message"]
            .as_str()
            .unwrap()
            .contains("changed body"));
        assert_eq!(store.linked_commits("stable-import").unwrap().len(), 1);
    }

    #[test]
    fn exact_get_fails_typed_instead_of_truncating_an_oversized_wire_response() {
        let store = temp_store();
        let record = ExternalDecision {
            id: "large-record".to_owned(),
            kind: "decision".to_owned(),
            title: "large record".to_owned(),
            content: "\0".repeat(750_000),
            importance: 0.5,
            source: "synthetic".to_owned(),
            author: "tester".to_owned(),
            date: "2026-01-01".to_owned(),
            updated_at: None,
            accessed_count: None,
            times_injected: None,
            effectiveness: None,
            tags: None,
            scope: "scope-a".to_owned(),
            valid_from: None,
            valid_until: None,
            superseded_by: None,
            fact_key: None,
            git_refs: Vec::new(),
        };
        store.import_external(&[record]).unwrap();

        let error = dispatch_tool(
            &store,
            "open-why_get",
            &json!({"id":"large-record","scope":"scope-a"}),
            1_700_000_000,
        )
        .unwrap_err();
        assert_eq!(error.payload["contract"], MCP_ERROR_CONTRACT);
        assert_eq!(error.payload["code"], "response_too_large");
        assert_eq!(error.payload["as_of"], "2023-11-14T22:13:20Z");
    }

    #[test]
    fn exact_history_enforces_page_bounds_and_returns_typed_oversize_failure() {
        let store = temp_store();
        let record = ExternalDecision {
            id: "large-history-record".to_owned(),
            kind: "decision".to_owned(),
            title: "large history record".to_owned(),
            content: "\0".repeat(750_000),
            importance: 0.5,
            source: "synthetic".to_owned(),
            author: "tester".to_owned(),
            date: "2026-01-01".to_owned(),
            updated_at: None,
            accessed_count: None,
            times_injected: None,
            effectiveness: None,
            tags: None,
            scope: "scope-a".to_owned(),
            valid_from: None,
            valid_until: None,
            superseded_by: None,
            fact_key: None,
            git_refs: Vec::new(),
        };
        store.import_external(&[record]).unwrap();

        for invalid_limit in [json!(0), json!(4), json!("3")] {
            let error = dispatch_tool(
                &store,
                "open-why_history",
                &json!({
                    "id":"large-history-record",
                    "scope":"scope-a",
                    "limit":invalid_limit
                }),
                1_700_000_000,
            )
            .unwrap_err();
            assert!(matches!(
                error.payload["code"].as_str(),
                Some("limit_exceeded" | "invalid_arguments")
            ));
        }
        let unknown = dispatch_tool(
            &store,
            "open-why_history",
            &json!({
                "id":"large-history-record",
                "scope":"scope-a",
                "unexpected":"forbidden"
            }),
            1_700_000_000,
        )
        .unwrap_err();
        assert_eq!(unknown.payload["code"], "invalid_arguments");

        let oversized = dispatch_tool(
            &store,
            "open-why_history",
            &json!({"id":"large-history-record","scope":"scope-a"}),
            1_700_000_000,
        )
        .unwrap_err();
        assert_eq!(oversized.payload["contract"], RATIONALE_HISTORY_CONTRACT);
        assert_eq!(oversized.payload["code"], "response_too_large");
        assert_eq!(oversized.payload["as_of"], "2023-11-14T22:13:20Z");
    }

    #[test]
    fn exact_commit_links_preserve_whitespace_bearing_identities() {
        let store = temp_store();
        let scope = " scope-a ";
        let commit = " exact-hash ";
        for (id, subject) in [(" record-a ", "first"), (" record-b ", "second")] {
            store
                .import_external(&[ExternalDecision {
                    id: id.to_owned(),
                    kind: "decision".to_owned(),
                    title: id.to_owned(),
                    content: "body must not be returned".to_owned(),
                    importance: 0.5,
                    source: "synthetic".to_owned(),
                    author: "tester".to_owned(),
                    date: "2026-01-01".to_owned(),
                    updated_at: None,
                    accessed_count: None,
                    times_injected: None,
                    effectiveness: None,
                    tags: None,
                    scope: scope.to_owned(),
                    valid_from: None,
                    valid_until: None,
                    superseded_by: None,
                    fact_key: None,
                    git_refs: vec![store::GitRef {
                        commit_hash: commit.to_owned(),
                        commit_subject: subject.to_owned(),
                    }],
                }])
                .unwrap();
        }

        let first = dispatch_tool(
            &store,
            "open-why_commit_links",
            &json!({"scope":scope,"commit":commit,"limit":1}),
            0,
        )
        .unwrap();
        assert_eq!(first["scope"], scope);
        assert_eq!(first["commit"], commit);
        assert_eq!(first["items"][0]["record_id"], " record-a ");
        assert_eq!(first["next_cursor"], " record-b ");

        for arguments in [
            json!({"scope":"scope-a","commit":commit}),
            json!({"scope":scope,"commit":"exact-hash"}),
        ] {
            let error = dispatch_tool(&store, "open-why_commit_links", &arguments, 0).unwrap_err();
            assert_eq!(error.payload["code"], "not_found");
        }

        let cursor = first["next_cursor"].as_str().unwrap();
        let second = dispatch_tool(
            &store,
            "open-why_commit_links",
            &json!({"scope":scope,"commit":commit,"limit":1,"cursor":cursor}),
            0,
        )
        .unwrap();
        assert_eq!(second["items"][0]["record_id"], " record-b ");
        assert_eq!(second["next_cursor"], Value::Null);

        let trimmed_cursor = dispatch_tool(
            &store,
            "open-why_commit_links",
            &json!({"scope":scope,"commit":commit,"cursor":"record-b"}),
            0,
        )
        .unwrap_err();
        assert_eq!(trimmed_cursor.payload["code"], "invalid_cursor");
    }

    #[test]
    fn exact_commit_links_validate_bounds_and_fail_closed_on_oversize() {
        let store = temp_store();
        let record = ExternalDecision {
            id: "linked-record".to_owned(),
            kind: "decision".to_owned(),
            title: "linked record".to_owned(),
            content: "body must not be returned".to_owned(),
            importance: 0.5,
            source: "synthetic".to_owned(),
            author: "tester".to_owned(),
            date: "2026-01-01".to_owned(),
            updated_at: None,
            accessed_count: None,
            times_injected: None,
            effectiveness: None,
            tags: None,
            scope: "scope-a".to_owned(),
            valid_from: None,
            valid_until: None,
            superseded_by: None,
            fact_key: None,
            git_refs: vec![store::GitRef {
                commit_hash: "exact-hash".to_owned(),
                commit_subject: "s".repeat(MAX_GIT_SUBJECT_BYTES + 1),
            }],
        };
        store.import_external(&[record]).unwrap();

        for invalid_limit in [json!(0), json!(21), json!("20")] {
            let error = dispatch_tool(
                &store,
                "open-why_commit_links",
                &json!({"scope":"scope-a","commit":"exact-hash","limit":invalid_limit}),
                0,
            )
            .unwrap_err();
            assert!(matches!(
                error.payload["code"].as_str(),
                Some("limit_exceeded" | "invalid_arguments")
            ));
        }
        for (scope, commit) in [("   ", "exact-hash"), ("scope-a", "   ")] {
            let error = dispatch_tool(
                &store,
                "open-why_commit_links",
                &json!({"scope":scope,"commit":commit}),
                0,
            )
            .unwrap_err();
            assert_eq!(error.payload["code"], "invalid_arguments");
        }
        for arguments in [
            json!({"scope":" ".repeat(MAX_AUTHORITY_BYTES + 1),"commit":"exact-hash"}),
            json!({"scope":"scope-a","commit":" ".repeat(MAX_GIT_HASH_BYTES + 1)}),
            json!({"scope":"scope-a","commit":"exact-hash","cursor":" ".repeat(MAX_ID_BYTES + 1)}),
        ] {
            let error = dispatch_tool(&store, "open-why_commit_links", &arguments, 0).unwrap_err();
            assert_eq!(error.payload["code"], "limit_exceeded");
        }
        let unknown = dispatch_tool(
            &store,
            "open-why_commit_links",
            &json!({"scope":"scope-a","commit":"exact-hash","unexpected":true}),
            0,
        )
        .unwrap_err();
        assert_eq!(unknown.payload["code"], "invalid_arguments");

        let oversized = dispatch_tool(
            &store,
            "open-why_commit_links",
            &json!({"scope":"scope-a","commit":"exact-hash"}),
            0,
        )
        .unwrap_err();
        assert_eq!(oversized.payload["contract"], COMMIT_LINKS_CONTRACT);
        assert_eq!(oversized.payload["code"], "response_too_large");
        assert!(oversized.payload.get("content").is_none());
    }
}
