//! End-to-end MCP contract tests against the real `why serve` process.

use open_why::{ExternalDecision, GitRef, Store};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

fn unique_temp_db(label: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "open-why-mcp-{label}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("test.db")
}

struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl Server {
    fn spawn(db_path: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_why"))
            .arg("serve")
            .env("OPEN_WHY_DB", db_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("failed to spawn `why serve`");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin: Some(stdin),
            stdout,
        }
    }

    fn raw(&mut self, line: &str) -> Value {
        writeln!(self.stdin.as_mut().unwrap(), "{line}").unwrap();
        let mut response = String::new();
        self.stdout.read_line(&mut response).unwrap();
        serde_json::from_str(&response).unwrap()
    }

    fn request(&mut self, value: Value) -> Value {
        self.raw(&serde_json::to_string(&value).unwrap())
    }

    fn call(&mut self, id: u64, name: &str, arguments: Value) -> (Value, bool) {
        let response = self.request(json!({
            "jsonrpc":"2.0",
            "id":id,
            "method":"tools/call",
            "params":{"name":name,"arguments":arguments}
        }));
        let is_error = response["result"]["isError"].as_bool().unwrap();
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        (serde_json::from_str(text).unwrap(), is_error)
    }

    fn finish(mut self) {
        drop(self.stdin.take());
        let status = self.child.wait().expect("why serve did not exit cleanly");
        assert!(status.success());
    }
}

fn record(id: &str, content: String, successor: Option<&str>) -> ExternalDecision {
    ExternalDecision {
        id: id.to_owned(),
        kind: "decision".to_owned(),
        title: format!("Rationale {id}"),
        content,
        importance: 0.8,
        source: "synthetic-fixture".to_owned(),
        author: "test-author".to_owned(),
        date: "2026-02-01".to_owned(),
        updated_at: None,
        accessed_count: None,
        times_injected: None,
        effectiveness: None,
        tags: None,
        scope: "scope-a".to_owned(),
        valid_from: Some("2026-01-01T00:00:00Z".to_owned()),
        valid_until: successor.map(|_| "2026-02-01T00:00:00Z".to_owned()),
        superseded_by: successor.map(str::to_owned),
        fact_key: None,
        git_refs: vec![GitRef {
            commit_hash: if successor.is_none() {
                "0123456789abcdef".to_owned()
            } else {
                format!("commit-{id}")
            },
            commit_subject: format!("Apply rationale {id}"),
        }],
    }
}

fn initialize(server: &mut Server, id: u64) -> (String, String) {
    let init = server.request(json!({
        "jsonrpc":"2.0","id":id,"method":"initialize","params":{}
    }));
    assert_eq!(init["result"]["serverInfo"]["name"], "open-why");
    let expected_contracts = json!([
        "open-why.current-rationale/v1",
        "open-why.rationale-history/v1",
        "open-why.commit-links/v1"
    ]);
    let init_metadata = &init["result"]["capabilities"]["experimental"]["openWhy"];
    assert_eq!(init_metadata["contract"], "open-why.current-rationale/v1");
    assert_eq!(init_metadata["contracts"], expected_contracts);
    let init_digest = init["result"]["capabilities"]["experimental"]["openWhy"]["registryDigest"]
        .as_str()
        .unwrap()
        .to_owned();
    let list = server.request(json!({
        "jsonrpc":"2.0","id":id + 1,"method":"tools/list","params":{}
    }));
    let list_digest = list["result"]["_meta"]["registryDigest"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        list["result"]["_meta"]["contract"],
        "open-why.current-rationale/v1"
    );
    assert_eq!(list["result"]["_meta"]["contracts"], expected_contracts);
    assert_eq!(init_digest, list_digest);
    (
        init_digest,
        serde_json::to_string(&list["result"]["tools"]).unwrap(),
    )
}

#[test]
fn exact_id_contract_catalog_callability_and_fresh_process_digest() {
    let db_path = unique_temp_db("contract");
    let full_body = format!("final rationale sentinel\n{}", "body-data-".repeat(2_000));
    {
        let store = Store::open(&db_path).unwrap();
        store
            .import_external(&[
                record(
                    "history-a",
                    "retired rationale α".to_owned(),
                    Some("history-b"),
                ),
                record(
                    "history-b",
                    "second rationale β".to_owned(),
                    Some("history-c"),
                ),
                record(
                    "history-c",
                    "third rationale γ".to_owned(),
                    Some("history-d"),
                ),
                record(
                    "history-d",
                    "fourth rationale δ".to_owned(),
                    Some("history-e"),
                ),
                record("history-e", full_body.clone(), None),
            ])
            .unwrap();
    }

    let mut server = Server::spawn(&db_path);
    let (digest, tools_json) = initialize(&mut server, 1);
    let tools: Vec<Value> = serde_json::from_str(&tools_json).unwrap();
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "open-why_ask",
            "open-why_index",
            "open-why_capture",
            "open-why_import",
            "open-why_search",
            "open-why_get",
            "open-why_history",
            "open-why_commit_links",
            "open-why_link",
            "open-why_feedback",
        ]
    );

    let repo = env!("CARGO_MANIFEST_DIR");
    let valid_calls = [
        ("open-why_index", json!({"repo":repo})),
        (
            "open-why_ask",
            json!({"question":"why evidence","repo":repo}),
        ),
        (
            "open-why_capture",
            json!({"id":"captured-id","title":"Captured","content":"bounded body","scope":"scope-a"}),
        ),
        ("open-why_import", json!({"scope":"scope-a","rows":[]})),
        (
            "open-why_search",
            json!({"query":"final rationale sentinel","scope":"scope-a","limit":10}),
        ),
        (
            "open-why_history",
            json!({"id":"history-a","scope":"scope-a","limit":3}),
        ),
        (
            "open-why_link",
            json!({"commit":"fedcba9876543210","decision":"history-e","scope":"scope-a"}),
        ),
        (
            "open-why_commit_links",
            json!({"commit":"fedcba9876543210","scope":"scope-a","limit":20}),
        ),
        (
            "open-why_feedback",
            json!({"id":"history-e","helpful":true,"scope":"scope-a"}),
        ),
        ("open-why_get", json!({"id":"history-a","scope":"scope-a"})),
    ];
    let mut payloads = Vec::new();
    for (offset, (name, arguments)) in valid_calls.into_iter().enumerate() {
        let (payload, is_error) = server.call(10 + offset as u64, name, arguments);
        assert!(!is_error, "advertised tool {name} failed: {payload}");
        payloads.push((name, payload));
    }

    let search = &payloads[4].1;
    assert_eq!(search["results"][0]["id"], "history-e");
    assert_eq!(search["results"][0]["preview_truncated"], true);
    assert!(search["results"][0]["preview"].as_str().unwrap().len() <= 512);

    let history = &payloads[5].1;
    assert_eq!(history["contract"], "open-why.rationale-history/v1");
    assert_eq!(history["requested_id"], "history-a");
    assert_eq!(history["page_start_id"], "history-a");
    assert_eq!(history["complete"], false);
    assert_eq!(history["next_cursor"], "history-d");
    let first_records = history["records"].as_array().unwrap();
    assert_eq!(first_records.len(), 3);
    assert_eq!(first_records[0]["record"]["content"], "retired rationale α");
    assert_eq!(first_records[1]["record"]["content"], "second rationale β");
    assert_eq!(first_records[2]["record"]["content"], "third rationale γ");
    assert!(first_records.iter().all(|item| item["git_refs"]
        .as_array()
        .is_some_and(|refs| refs.len() == 1)));

    let (history_second, history_second_error) = server.call(
        28,
        "open-why_history",
        json!({"id":"history-a","scope":"scope-a","limit":3,"cursor":"history-d"}),
    );
    assert!(!history_second_error);
    assert_eq!(history_second["page_start_id"], "history-d");
    assert_eq!(history_second["complete"], true);
    assert_eq!(history_second["next_cursor"], Value::Null);
    assert_eq!(
        history_second["records"][0]["record"]["content"],
        "fourth rationale δ"
    );
    assert_eq!(history_second["records"][1]["record"]["content"], full_body);
    assert!(history_second["records"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["git_refs"]
            .as_array()
            .is_some_and(|refs| !refs.is_empty())));
    let reconstructed: Vec<&str> = first_records
        .iter()
        .chain(history_second["records"].as_array().unwrap())
        .map(|item| item["record"]["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        reconstructed,
        [
            "history-a",
            "history-b",
            "history-c",
            "history-d",
            "history-e"
        ]
    );

    let history_tool = tools
        .iter()
        .find(|tool| tool["name"] == "open-why_history")
        .unwrap();
    assert_eq!(
        history_tool["inputSchema"]["required"],
        json!(["id", "scope"])
    );
    assert_eq!(
        history_tool["inputSchema"]["properties"]["limit"]["maximum"],
        3
    );
    assert_eq!(history_tool["inputSchema"]["additionalProperties"], false);

    let (history_repeat, history_repeat_error) = server.call(
        29,
        "open-why_history",
        json!({"id":"history-a","scope":"scope-a","limit":3}),
    );
    assert!(!history_repeat_error);
    let mut first_history_without_clock = history.clone();
    let mut repeat_history_without_clock = history_repeat;
    first_history_without_clock
        .as_object_mut()
        .unwrap()
        .remove("as_of");
    repeat_history_without_clock
        .as_object_mut()
        .unwrap()
        .remove("as_of");
    assert_eq!(first_history_without_clock, repeat_history_without_clock);

    let commit_links = &payloads[7].1;
    assert_eq!(commit_links["contract"], "open-why.commit-links/v1");
    assert_eq!(commit_links["status"], "ok");
    assert_eq!(commit_links["scope"], "scope-a");
    assert_eq!(commit_links["commit"], "fedcba9876543210");
    assert_eq!(commit_links["items"][0]["record_id"], "history-e");
    assert_eq!(commit_links["items"][0]["commit_subject"], "");
    assert_eq!(commit_links["next_cursor"], Value::Null);
    assert!(commit_links.get("content").is_none());

    let commit_links_tool = tools
        .iter()
        .find(|tool| tool["name"] == "open-why_commit_links")
        .unwrap();
    assert_eq!(
        commit_links_tool["inputSchema"]["required"],
        json!(["scope", "commit"])
    );
    assert_eq!(
        commit_links_tool["inputSchema"]["properties"]["limit"]["maximum"],
        20
    );
    assert_eq!(
        commit_links_tool["inputSchema"]["additionalProperties"],
        false
    );

    let (historical_link, historical_link_error) = server.call(
        39,
        "open-why_commit_links",
        json!({"commit":"commit-history-a","scope":"scope-a"}),
    );
    assert!(!historical_link_error);
    assert_eq!(historical_link["items"][0]["record_id"], "history-a");

    let get = &payloads[9].1;
    assert_eq!(get["contract"], "open-why.current-rationale/v1");
    assert_eq!(get["requested_id"], "history-a");
    assert_eq!(get["current_id"], "history-e");
    assert_eq!(get["record"]["content"], full_body);
    assert!(get["record"]["updated_at"].is_string());
    assert!(get["record"]["access_count"].is_number());
    assert!(get["record"]["effectiveness"].is_number());
    assert!(get["git_refs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|git_ref| git_ref["commit_hash"] == "0123456789abcdef"));
    assert_eq!(
        get["supersession_chain"],
        json!([
            "history-a",
            "history-b",
            "history-c",
            "history-d",
            "history-e"
        ])
    );
    assert!(get["as_of"].as_str().unwrap().ends_with('Z'));

    let (repeat, repeat_error) = server.call(
        30,
        "open-why_get",
        json!({"id":"history-a","scope":"scope-a"}),
    );
    assert!(!repeat_error);
    let mut first_without_clock = get.clone();
    let mut repeat_without_clock = repeat.clone();
    first_without_clock.as_object_mut().unwrap().remove("as_of");
    repeat_without_clock
        .as_object_mut()
        .unwrap()
        .remove("as_of");
    assert_eq!(first_without_clock, repeat_without_clock);
    assert!(repeat["as_of"].as_str().unwrap().ends_with('Z'));

    let (wrong_scope, wrong_scope_error) = server.call(
        31,
        "open-why_get",
        json!({"id":"history-a","scope":"scope-b"}),
    );
    assert!(wrong_scope_error);
    assert_eq!(wrong_scope["code"], "not_found");
    assert!(wrong_scope["as_of"].as_str().unwrap().ends_with('Z'));
    let (missing, missing_error) = server.call(
        36,
        "open-why_get",
        json!({"id":"missing-id","scope":"scope-a"}),
    );
    assert!(missing_error);
    assert_eq!(missing["contract"], "open-why.current-rationale/v1");
    assert_eq!(missing["code"], "not_found");
    assert!(missing["as_of"].as_str().unwrap().ends_with('Z'));
    let (invalid_cursor, invalid_cursor_error) = server.call(
        37,
        "open-why_history",
        json!({"id":"history-a","scope":"scope-a","cursor":"captured-id"}),
    );
    assert!(invalid_cursor_error);
    assert_eq!(invalid_cursor["contract"], "open-why.rationale-history/v1");
    assert_eq!(invalid_cursor["code"], "invalid_cursor");
    let (unknown, unknown_error) = server.call(32, "open-why_hidden", json!({}));
    assert!(unknown_error);
    assert_eq!(unknown["code"], "unknown_tool");

    let malformed = server.raw("{bad json}");
    assert_eq!(malformed["error"]["code"], -32700);
    let method = server.request(json!({"jsonrpc":"2.0","id":34,"method":"hidden"}));
    assert_eq!(method["error"]["code"], -32601);
    let (invalid, invalid_error) = server.call(35, "open-why_get", json!({"id":"stale-id"}));
    assert!(invalid_error);
    assert_eq!(invalid["code"], "scope_authority_required");
    server.finish();

    let mut fresh = Server::spawn(&db_path);
    let (fresh_digest, fresh_tools_json) = initialize(&mut fresh, 100);
    assert_eq!(digest, fresh_digest);
    assert_eq!(tools_json, fresh_tools_json);
    fresh.finish();

    std::fs::remove_dir_all(db_path.parent().unwrap()).unwrap();
}
