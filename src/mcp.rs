use crate::answer::{ask, render};
use crate::{db, miner, store};
use anyhow::Result;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

fn write_resp(w: &mut impl Write, v: &Value) -> Result<()> {
    w.write_all(serde_json::to_string(v)?.as_bytes())?;
    w.write_all(b"\n")?;
    w.flush()?;
    Ok(())
}

fn s(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(|v| v.as_str()).map(|x| x.to_string())
}

fn tool(name: &str, description: &str, props: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": props,
            "required": required
        }
    })
}

/// Parse a `type` (or `types`) facet — a comma-separated string or an array of kind names.
fn kinds_from(args: &Value) -> Vec<String> {
    let Some(v) = args.get("type").or_else(|| args.get("types")) else {
        return Vec::new();
    };
    match v {
        Value::String(s) => s
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect(),
        Value::Array(a) => a
            .iter()
            .filter_map(|x| x.as_str())
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

pub fn serve() -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let store = db::Store::open_default()?;
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("").to_string();
        let id = msg.get("id").cloned();

        match method.as_str() {
            "initialize" => {
                let resp = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": { "tools": {} },
                        "serverInfo": { "name": "open-why", "version": env!("CARGO_PKG_VERSION") }
                    }
                });
                write_resp(&mut stdout, &resp)?;
            }
            "tools/list" => {
                let resp = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "tools": [
                            tool("open-why_ask",
                                "Ask why a decision was made in a repository. Returns evidence-bound answers (subject, author, date, commit/file).",
                                json!({
                                    "question": {"type": "string", "description": "The 'why' question to answer"},
                                    "repo": {"type": "string", "description": "Repo path or git URL (default: current directory)"}
                                }),
                                &["question"]),
                            tool("open-why_index",
                                "Index a repository's decision history (commits + ADRs) into the store.",
                                json!({
                                    "repo": {"type": "string", "description": "Repo path or git URL (default: current directory)"}
                                }),
                                &[]),
                            tool("open-why_capture",
                                "Capture a decision into the store (idempotent; supersedes optional).",
                                json!({
                                    "kind": {"type": "string", "description": "decision, fact, reference, pattern, doc, ... (default: decision)"},
                                    "title": {"type": "string"},
                                    "content": {"type": "string"},
                                    "importance": {"type": "number", "description": "0..1 (default 0.5)"},
                                    "scope": {"type": "string", "description": "default: global"},
                                    "id": {"type": "string", "description": "optional externally-minted id (preserved verbatim)"},
                                    "valid_from": {"type": "string", "description": "optional ISO validity start (default: now)"},
                                    "fact_key": {"type": "string", "description": "optional stable key; re-capturing the same key retires the prior current record"},
                                    "supersedes": {"type": "string", "description": "id of an older decision this one supersedes"}
                                }),
                                &["title", "content"]),
                            tool("open-why_import",
                                "Bulk-import externally-minted decisions preserving ids, temporal windows, supersession, and git linkage.",
                                json!({
                                    "rows": {"type": "array", "description": "array of decision records {id, kind, title, content, importance?, source?, author?, date?, scope?, valid_from?, valid_until?, superseded_by?, git_refs?:[{commit_hash, commit_subject}]}"}
                                }),
                                &["rows"]),
                            tool("open-why_search",
                                "Search the decision store across a scope.",
                                json!({
                                    "query": {"type": "string"},
                                    "limit": {"type": "number", "description": "default 10"},
                                    "scope": {"type": "string", "description": "default: global"},
                                    "type": {"type": ["string", "array"], "description": "optional kind facet (decision/fact/reference/project/pattern/doc/observation; comma-separated string or array)"},
                                    "format": {"type": "string", "description": "text (default) or json (structured records with ids and temporal windows)"}
                                }),
                                &["query"]),
                            tool("open-why_get",
                                "Fetch one decision by id, with its linked commits.",
                                json!({
                                    "id": {"type": "string"},
                                    "format": {"type": "string", "description": "text (default) or json (structured record with temporal window)"}
                                }),
                                &["id"]),
                            tool("open-why_link",
                                "Link a git commit to a decision (the 'why' for that commit).",
                                json!({
                                    "commit": {"type": "string"},
                                    "decision": {"type": "string"},
                                    "subject": {"type": "string"}
                                }),
                                &["commit", "decision"])
                        ]
                    }
                });
                write_resp(&mut stdout, &resp)?;
            }
            "tools/call" => {
                let name = msg["params"]["name"].as_str().unwrap_or("");
                let args = &msg["params"]["arguments"];
                let text = call_tool(&store, name, args);
                let resp = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [{"type": "text", "text": text}],
                        "isError": false
                    }
                });
                write_resp(&mut stdout, &resp)?;
            }
            "ping" => {
                let resp = json!({ "jsonrpc": "2.0", "id": id, "result": {} });
                write_resp(&mut stdout, &resp)?;
            }
            "notifications/initialized"
            | "notifications/cancelled"
            | "notifications/roots/list_changed" => {}
            _ => {
                if let Some(id) = id {
                    let resp = json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": {"code": -32601, "message": format!("method not found: {method}")}
                    });
                    write_resp(&mut stdout, &resp)?;
                }
            }
        }
    }
    Ok(())
}

fn call_tool(store: &db::Store, name: &str, args: &Value) -> String {
    match name {
        "open-why_ask" => {
            let question = s(args, "question").unwrap_or_default();
            let repo_arg = s(args, "repo");
            match miner::resolve_repo(repo_arg) {
                Ok(repo) => match ask(&question, &repo, 5) {
                    Ok(t) => t,
                    Err(e) => format!("error: {e:#}"),
                },
                Err(e) => format!("error: {e:#}"),
            }
        }
        "open-why_index" => {
            let repo_arg = s(args, "repo");
            match miner::resolve_repo(repo_arg) {
                Ok(repo) => {
                    let scope = store::scope_for(&repo);
                    match miner::mine(&repo).and_then(|d| store.import_decisions(&scope, &d).map(|_| d.len())) {
                        Ok(len) => format!("indexed {len} decisions (scope: {scope})"),
                        Err(e) => format!("error: {e:#}"),
                    }
                }
                Err(e) => format!("error: {e:#}"),
            }
        }
        "open-why_capture" => {
            let kind = s(args, "kind").unwrap_or_else(|| "decision".to_string());
            let title = s(args, "title");
            let content = s(args, "content");
            let (Some(title), Some(content)) = (title, content) else {
                return "error: title and content are required".to_string();
            };
            let scope = s(args, "scope").unwrap_or_else(|| "global".to_string());
            let importance = args.get("importance").and_then(|v| v.as_f64()).unwrap_or(0.5);
            let id = s(args, "id");
            let valid_from = s(args, "valid_from");
            let fact_key = s(args, "fact_key");
            let supersedes = s(args, "supersedes");
            let d = store::Decision {
                subject: title,
                body: content,
                kind,
                importance,
                source: "capture".to_string(),
                ..store::Decision::default()
            };
            let result = match id.as_deref() {
                Some(id) if !id.is_empty() => {
                    store.capture_external(&d, &scope, id, valid_from.as_deref(), fact_key.as_deref(), supersedes.as_deref())
                }
                _ => store.capture(&d, &scope, supersedes.as_deref()),
            };
            match result {
                Ok(id) => format!("captured decision {id} (scope: {scope})"),
                Err(e) => format!("error: {e:#}"),
            }
        }
        "open-why_import" => {
            let rows: Vec<store::ExternalDecision> = match serde_json::from_value(args.get("rows").cloned().unwrap_or(Value::Null)) {
                Ok(rows) => rows,
                Err(e) => return format!("error: bad rows: {e}"),
            };
            match store.import_external(&rows) {
                Ok(n) => format!("imported {n} decisions"),
                Err(e) => format!("error: {e:#}"),
            }
        }
        "open-why_search" => {
            let query = s(args, "query").unwrap_or_default();
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
            let scope = s(args, "scope").unwrap_or_else(|| "global".to_string());
            let kinds = kinds_from(args);
            if s(args, "format").as_deref() == Some("json") {
                match store.search_records(&query, &[scope.as_str()], &kinds, limit) {
                    Ok(records) => serde_json::to_string(&records).unwrap_or_else(|e| format!("error: {e}")),
                    Err(e) => format!("error: {e:#}"),
                }
            } else {
                match store.search(&query, &[scope.as_str()], &kinds, limit) {
                    Ok(hits) => render(hits),
                    Err(e) => format!("error: {e:#}"),
                }
            }
        }
        "open-why_get" => {
            let id = s(args, "id").unwrap_or_default();
            if s(args, "format").as_deref() == Some("json") {
                match store.get_record(&id) {
                    Ok(Some(r)) => serde_json::to_string(&r).unwrap_or_else(|e| format!("error: {e}")),
                    Ok(None) => "null".to_string(),
                    Err(e) => format!("error: {e:#}"),
                }
            } else {
                match store.get(&id) {
                    Ok(Some(d)) => {
                        let mut t = format!("- {}\n  {} · {} · {}\n  {}", d.subject, d.date, d.author, d.source, d.body);
                        if let Ok(commits) = store.linked_commits(&id) {
                            if !commits.is_empty() {
                                t.push_str("\n\n  linked commits:");
                                for (hash, subj) in commits {
                                    t.push_str(&format!("\n    {} {subj}", &hash[..hash.len().min(8)]));
                                }
                            }
                        }
                        t
                    }
                    Ok(None) => format!("no active decision with id {id}"),
                    Err(e) => format!("error: {e:#}"),
                }
            }
        }
        "open-why_link" => {
            let commit = s(args, "commit").unwrap_or_default();
            let decision = s(args, "decision").unwrap_or_default();
            let subject = s(args, "subject").unwrap_or_default();
            if commit.is_empty() || decision.is_empty() {
                return "error: commit and decision are required".to_string();
            }
            match store.link_git(&decision, &commit, &subject) {
                Ok(()) => format!("linked {commit} -> {decision}"),
                Err(e) => format!("error: {e:#}"),
            }
        }
        _ => format!("unknown tool: {name}"),
    }
}
