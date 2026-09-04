//! End-to-end MCP contract tests against the real `why serve` process.

use open_why::{ExternalDecision, GitRef, Store};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};

fn unique_temp_db(label: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!(
            "open-why-mcp-{label}-{}-{nanos}",
            std::process::id()
        ));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("test.db")
}

fn provider_id_for(db_path: &std::path::Path) -> String {
    format!(
        "mcp-test:{}",
        db_path
            .parent()
            .and_then(|value| value.file_name())
            .and_then(|value| value.to_str())
            .unwrap_or("store")
    )
}

struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    stderr: Option<ChildStderr>,
}

impl Server {
    fn spawn(db_path: &std::path::Path) -> Self {
        let provider_id = provider_id_for(db_path);
        let mut child = Command::new(env!("CARGO_BIN_EXE_why"))
            .arg("serve")
            .env("OPEN_WHY_DB", db_path)
            .env("OPEN_WHY_STORE_INSTANCE_ID", provider_id)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn `why serve`");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let stderr = child.stderr.take();
        Self {
            child,
            stdin: Some(stdin),
            stdout,
            stderr,
        }
    }

    fn spawn_default(home: &std::path::Path, provider_id: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_why"))
            .arg("serve")
            .env_remove("OPEN_WHY_DB")
            .env_remove("OPEN_WHY_EMBED_MODEL_PATH")
            .env_remove("OPEN_WHY_EMBED_URL")
            .env("HOME", home)
            .env("OPEN_WHY_STORE_INSTANCE_ID", provider_id)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn documented `why serve` configuration");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let stderr = child.stderr.take();
        Self {
            child,
            stdin: Some(stdin),
            stdout,
            stderr,
        }
    }

    fn spawn_relative(
        working_directory: &std::path::Path,
        db_path: &std::path::Path,
        provider_id: &str,
    ) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_why"))
            .arg("serve")
            .current_dir(working_directory)
            .env("OPEN_WHY_DB", db_path)
            .env_remove("OPEN_WHY_EMBED_MODEL_PATH")
            .env_remove("OPEN_WHY_EMBED_URL")
            .env("OPEN_WHY_STORE_INSTANCE_ID", provider_id)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn `why serve` with a relative store path");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let stderr = child.stderr.take();
        Self {
            child,
            stdin: Some(stdin),
            stdout,
            stderr,
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

    fn finish(mut self) -> String {
        drop(self.stdin.take());
        let status = self.child.wait().expect("why serve did not exit cleanly");
        assert!(status.success());
        let mut output = String::new();
        self.stderr
            .take()
            .unwrap()
            .read_to_string(&mut output)
            .unwrap();
        output
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
        "open-why.commit-links/v1",
        "open-why.rationale-import/v1"
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

fn assert_canonical_utc(value: &str) {
    let bytes = value.as_bytes();
    assert_eq!(bytes.len(), 20, "expected second-resolution UTC: {value}");
    assert_eq!(bytes[4], b'-');
    assert_eq!(bytes[7], b'-');
    assert_eq!(bytes[10], b'T');
    assert_eq!(bytes[13], b':');
    assert_eq!(bytes[16], b':');
    assert_eq!(bytes[19], b'Z');
    assert!(bytes
        .iter()
        .enumerate()
        .filter(|(index, _)| ![4, 7, 10, 13, 16, 19].contains(index))
        .all(|(_, byte)| byte.is_ascii_digit()));
}

#[test]
fn documented_first_launch_starts_with_an_explicit_store_identity() {
    let fixture_path = unique_temp_db("documented-first-launch");
    let root = fixture_path.parent().unwrap();
    let home = root.join("home");
    let db_path = home.join(".cache").join("open-why").join("open-why.db");
    assert!(!db_path.exists());

    let provider_id = "documented-client:open-why:001";
    let mut server = Server::spawn_default(&home, provider_id);
    initialize(&mut server, 1);
    let ping = server.request(json!({"jsonrpc":"2.0","id":3,"method":"ping","params":{}}));
    assert_eq!(ping, json!({"jsonrpc":"2.0","id":3,"result":{}}));
    let diagnostics = server.finish();
    assert!(!diagnostics.contains("internal tool failure"));

    let identity = Store::open(&db_path).unwrap().store_identity().unwrap();
    assert_eq!(identity.store_instance_id, provider_id);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn fresh_nested_relative_store_path_starts_privately() {
    use std::os::unix::fs::PermissionsExt;

    let fixture_path = unique_temp_db("relative-first-launch");
    let root = fixture_path.parent().unwrap();
    let relative_path = std::path::Path::new("relative/nested/store.db");
    let provider_id = "documented-client:relative-open-why:001";
    let mut server = Server::spawn_relative(root, relative_path, provider_id);
    initialize(&mut server, 1);
    let ping = server.request(json!({"jsonrpc":"2.0","id":3,"method":"ping","params":{}}));
    assert_eq!(ping, json!({"jsonrpc":"2.0","id":3,"result":{}}));
    assert!(server.finish().is_empty());

    let absolute_path = root.join(relative_path);
    let mode =
        |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&root.join("relative")), 0o700);
    assert_eq!(mode(&root.join("relative/nested")), 0o700);
    assert_eq!(mode(&absolute_path), 0o600);
    let identity = Store::open(&absolute_path)
        .unwrap()
        .store_identity()
        .unwrap();
    assert_eq!(identity.store_instance_id, provider_id);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn real_process_rejects_a_symlinked_fresh_store_parent() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let fixture_path = unique_temp_db("symlinked-parent");
    let root = fixture_path.parent().unwrap();
    let outside = root.join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o751)).unwrap();
    let link = root.join("link");
    symlink(&outside, &link).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_why"))
        .arg("serve")
        .env("OPEN_WHY_DB", link.join("store.db"))
        .env("OPEN_WHY_STORE_INSTANCE_ID", "provider:symlinked-parent")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!outside.join("store.db").exists());
    assert_eq!(
        std::fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
        0o751
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn repeated_feedback_is_durable_atomic_and_redacts_backend_errors() {
    let db_path = unique_temp_db("feedback-durability");
    let store = Store::open_with_store_instance_id(&db_path, &provider_id_for(&db_path)).unwrap();
    let id = store
        .capture(
            &open_why::Decision {
                subject: "Durable feedback".to_owned(),
                body: "Each verdict is recorded exactly once.".to_owned(),
                kind: "decision".to_owned(),
                source: "synthetic-fixture".to_owned(),
                importance: 0.5,
                ..open_why::Decision::default()
            },
            "scope-a",
            None,
        )
        .unwrap();
    store
        .import_external(&[
            record(
                "feedback-superseded-private",
                "superseded feedback body".to_owned(),
                Some("feedback-current-private"),
            ),
            record(
                "feedback-current-private",
                "current feedback body".to_owned(),
                None,
            ),
        ])
        .unwrap();
    drop(store);

    let mut server = Server::spawn(&db_path);
    initialize(&mut server, 1);
    let arguments = json!({"id":id,"helpful":true,"scope":"scope-a"});
    let (first, first_error) = server.call(10, "open-why_feedback", arguments.clone());
    let (second, second_error) = server.call(11, "open-why_feedback", arguments);
    assert!(!first_error);
    assert!(!second_error);
    assert!((first["effectiveness"].as_f64().unwrap() - 0.55).abs() < 1e-9);
    assert!((second["effectiveness"].as_f64().unwrap() - 0.6).abs() < 1e-9);

    let (missing, missing_error) = server.call(
        12,
        "open-why_feedback",
        json!({"id":"feedback-missing-private","helpful":true,"scope":"scope-a"}),
    );
    let (wrong_scope, wrong_scope_error) = server.call(
        13,
        "open-why_feedback",
        json!({"id":id,"helpful":true,"scope":"feedback-private-scope"}),
    );
    let (superseded, superseded_error) = server.call(
        14,
        "open-why_feedback",
        json!({
            "id":"feedback-superseded-private",
            "helpful":true,
            "scope":"scope-a"
        }),
    );
    assert!(missing_error);
    assert!(wrong_scope_error);
    assert!(superseded_error);
    assert_eq!(missing, wrong_scope);
    assert_eq!(missing["code"], "not_found");
    assert_eq!(superseded["code"], "not_current");
    assert_eq!(
        missing["message"],
        "record is unavailable in the requested scope"
    );
    assert_eq!(superseded["message"], missing["message"]);
    let private_values = [
        "feedback-missing-private",
        "feedback-superseded-private",
        "feedback-current-private",
        "feedback-private-scope",
        "scope-a",
        id.as_str(),
    ];
    for response in [&missing, &wrong_scope, &superseded] {
        let wire = serde_json::to_string(response).unwrap();
        for private in private_values {
            assert!(!wire.contains(private));
        }
    }
    let diagnostics = server.finish();
    assert!(!diagnostics.contains("internal tool failure"));

    let observer = Connection::open(&db_path).unwrap();
    let feedback_rows = observer
        .prepare("SELECT id,created_at FROM feedback_log WHERE memory_id=?1 ORDER BY rowid")
        .unwrap()
        .query_map([&id], |record| {
            Ok((record.get::<_, String>(0)?, record.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(feedback_rows.len(), 2);
    assert_ne!(feedback_rows[0].0, feedback_rows[1].0);
    feedback_rows
        .iter()
        .for_each(|(_, created_at)| assert_canonical_utc(created_at));
    let updated_at: String = observer
        .query_row(
            "SELECT updated_at FROM decisions WHERE id=?1",
            [&id],
            |record| record.get(0),
        )
        .unwrap();
    assert_canonical_utc(&updated_at);

    let mut reopened = Server::spawn(&db_path);
    initialize(&mut reopened, 20);
    let (current, current_error) =
        reopened.call(22, "open-why_get", json!({"id":id,"scope":"scope-a"}));
    assert!(!current_error);
    assert!((current["record"]["effectiveness"].as_f64().unwrap() - 0.6).abs() < 1e-9);
    assert_eq!(current["record"]["updated_at"], updated_at);
    let diagnostics = reopened.finish();
    assert!(!diagnostics.contains("internal tool failure"));

    let mut failing = Server::spawn(&db_path);
    initialize(&mut failing, 30);
    let backend_detail = format!("sensitive sqlite feedback detail {}", "x".repeat(4096));
    observer
        .execute_batch(&format!(
            "CREATE TRIGGER reject_mcp_feedback BEFORE INSERT ON feedback_log
             BEGIN SELECT RAISE(ABORT, '{backend_detail}'); END;"
        ))
        .unwrap();
    let before: (f64, i64, String, i64) = observer
        .query_row(
            "SELECT effectiveness,times_helpful,updated_at,
                    (SELECT count(*) FROM feedback_log WHERE memory_id=?1)
             FROM decisions WHERE id=?1",
            [&id],
            |record| {
                Ok((
                    record.get(0)?,
                    record.get(1)?,
                    record.get(2)?,
                    record.get(3)?,
                ))
            },
        )
        .unwrap();
    let (failure, is_error) = failing.call(
        32,
        "open-why_feedback",
        json!({"id":id,"helpful":true,"scope":"scope-a"}),
    );
    assert!(is_error);
    assert_eq!(
        failure,
        json!({
            "contract":"open-why.mcp-tool-error/v1",
            "status":"error",
            "code":"internal",
            "message":"internal tool failure",
            "retryable":false
        })
    );
    let wire = serde_json::to_string(&failure).unwrap();
    let provider_id = provider_id_for(&db_path);
    for hidden in [
        "feedback_log",
        "UNIQUE",
        backend_detail.as_str(),
        id.as_str(),
        "scope-a",
        provider_id.as_str(),
    ] {
        assert!(!wire.contains(hidden));
    }
    let diagnostics = failing.finish();
    assert!(diagnostics.contains("sensitive sqlite feedback detail"));
    assert!(!diagnostics.contains(&backend_detail));
    assert!(diagnostics.len() <= 2 * 1024 + 1);

    let after: (f64, i64, String, i64) = observer
        .query_row(
            "SELECT effectiveness,times_helpful,updated_at,
                    (SELECT count(*) FROM feedback_log WHERE memory_id=?1)
             FROM decisions WHERE id=?1",
            [&id],
            |record| {
                Ok((
                    record.get(0)?,
                    record.get(1)?,
                    record.get(2)?,
                    record.get(3)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(after, before);

    drop(observer);
    std::fs::remove_dir_all(db_path.parent().unwrap()).unwrap();
}

#[test]
fn exact_get_real_process_hides_cross_scope_identity_and_existence() {
    let foreign_path = unique_temp_db("scoped-current-foreign");
    let absent_path = unique_temp_db("scoped-current-absent");
    let scoped_record =
        |id: &str, successor: Option<&str>, scope: &str, content: &str| ExternalDecision {
            id: id.to_owned(),
            kind: "decision".to_owned(),
            title: format!("Rationale {id}"),
            content: content.to_owned(),
            importance: 0.8,
            source: "synthetic-fixture".to_owned(),
            author: "test-author".to_owned(),
            date: "2026-02-01".to_owned(),
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
            git_refs: vec![GitRef {
                commit_hash: format!("private-commit-{id}"),
                commit_subject: format!("Private evidence {id}"),
            }],
        };
    for path in [&foreign_path, &absent_path] {
        Store::open_with_store_instance_id(path, &provider_id_for(path))
            .unwrap()
            .import_external(&[
                scoped_record("cross-root", Some("middle"), "scope-a", "root body"),
                scoped_record("middle", Some("hidden-successor"), "scope-a", "middle body"),
            ])
            .unwrap();
    }
    Store::open_with_store_instance_id(&foreign_path, &provider_id_for(&foreign_path))
        .unwrap()
        .import_external(&[
            scoped_record(
                "hidden-successor",
                None,
                "scope-b",
                "foreign body 2099-01-01T00:00:00Z",
            ),
            scoped_record("same-root", None, "scope-b", "wrong-scope root body"),
        ])
        .unwrap();
    let corrupt = Connection::open(&foreign_path).unwrap();
    let identity_guard: String = corrupt
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type='trigger' AND name='decisions_identity_update_guard'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    corrupt
        .execute_batch("DROP TRIGGER decisions_identity_update_guard;")
        .unwrap();
    corrupt
        .execute(
            "UPDATE decisions
             SET content=CAST(zeroblob(8388608) AS TEXT), importance=X'FF', author=X'FF',
                 superseded_by=X'FF', valid_from=X'FF', valid_until=42
             WHERE id IN ('hidden-successor','same-root')",
            [],
        )
        .unwrap();
    corrupt.execute_batch(&identity_guard).unwrap();
    drop(corrupt);

    let foreign_watch = Connection::open(&foreign_path).unwrap();
    let absent_watch = Connection::open(&absent_path).unwrap();
    let data_version = |connection: &Connection| {
        connection
            .query_row("PRAGMA data_version", [], |row| row.get::<_, i64>(0))
            .unwrap()
    };
    let mut foreign_server = Server::spawn(&foreign_path);
    let mut absent_server = Server::spawn(&absent_path);
    initialize(&mut foreign_server, 100);
    initialize(&mut absent_server, 200);
    let foreign_version = data_version(&foreign_watch);
    let absent_version = data_version(&absent_watch);

    let normalize_as_of = |mut value: Value| {
        value["as_of"] = Value::String("normalized".to_owned());
        value
    };
    for (id, expected_code) in [("same-root", "not_found"), ("cross-root", "broken_chain")] {
        let (foreign, foreign_error) =
            foreign_server.call(300, "open-why_get", json!({"id":id,"scope":"scope-a"}));
        let (absent, absent_error) =
            absent_server.call(400, "open-why_get", json!({"id":id,"scope":"scope-a"}));
        assert!(foreign_error && absent_error);
        assert_eq!(foreign["contract"], "open-why.current-rationale/v1");
        assert_eq!(foreign["code"], expected_code);
        assert_eq!(normalize_as_of(foreign.clone()), normalize_as_of(absent));
        let wire = serde_json::to_string(&foreign).unwrap();
        for secret in [
            "hidden-successor",
            "2099-01-01T00:00:00Z",
            "foreign body",
            "private-commit-hidden-successor",
            "Private evidence hidden-successor",
            "wrong-scope root body",
        ] {
            assert!(!wire.contains(secret), "MCP error leaked {secret}");
        }
    }
    assert_eq!(data_version(&foreign_watch), foreign_version);
    assert_eq!(data_version(&absent_watch), absent_version);

    foreign_server.finish();
    absent_server.finish();
    std::fs::remove_dir_all(foreign_path.parent().unwrap()).unwrap();
    std::fs::remove_dir_all(absent_path.parent().unwrap()).unwrap();
}

#[test]
fn exact_id_contract_catalog_callability_and_fresh_process_digest() {
    let db_path = unique_temp_db("contract");
    let full_body = format!("final rationale sentinel\n{}", "body-data-".repeat(2_000));
    {
        let store =
            Store::open_with_store_instance_id(&db_path, &provider_id_for(&db_path)).unwrap();
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
    assert_eq!(digest, "b37082e4965e9009");
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
    let tool = |name: &str| {
        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .expect("catalog contains named tool")
    };
    assert_eq!(
        tool("open-why_capture")["inputSchema"]["properties"]["valid_from"]["maxLength"],
        128
    );
    let import_properties =
        &tool("open-why_import")["inputSchema"]["properties"]["rows"]["items"]["properties"];
    for field in ["date", "updated_at", "valid_from", "valid_until"] {
        assert_eq!(import_properties[field]["maxLength"], 128);
    }

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

    let import_tool = tools
        .iter()
        .find(|tool| tool["name"] == "open-why_import")
        .unwrap();
    assert_eq!(
        import_tool["_meta"]["contract"],
        "open-why.rationale-import/v1"
    );
    assert!(tools
        .iter()
        .filter(|tool| tool["name"] != "open-why_import")
        .all(|tool| tool.get("_meta").is_none()));
    assert_eq!(payloads[3].1["contract"], "open-why.rationale-import/v1");
    assert_eq!(payloads[3].1["imported"], 0);

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

    let import_row = json!({
        "id":"mcp-import","kind":"decision","title":"MCP import",
        "content":"sealed MCP body","importance":0.5,"source":"test",
        "author":"tester","date":"2026-01-01","scope":"scope-a",
        "git_refs":[{"commit_hash":"mcp-original","commit_subject":"Original"}]
    });
    let import_args = json!({"scope":"scope-a","rows":[import_row.clone()]});
    let (created_import, created_import_error) =
        server.call(50, "open-why_import", import_args.clone());
    assert!(!created_import_error);
    assert_eq!(created_import["contract"], "open-why.rationale-import/v1");
    assert_eq!(created_import["imported"], 1);
    let (replayed_import, replayed_import_error) = server.call(51, "open-why_import", import_args);
    assert!(!replayed_import_error);
    assert_eq!(replayed_import["imported"], 0);

    let mut conflict = import_row;
    conflict["content"] = json!("changed secret body");
    conflict["git_refs"] = json!([{
        "commit_hash":"must-not-append","commit_subject":"Must not append"
    }]);
    let new_row = json!({
        "id":"mcp-must-not-insert","kind":"decision","title":"Must not insert",
        "content":"must not insert","importance":0.5,"source":"test",
        "author":"tester","date":"2026-01-01","scope":"scope-a",
        "git_refs":[{"commit_hash":"must-not-land","commit_subject":"Must not land"}]
    });
    let (rejected_import, rejected_import_error) = server.call(
        52,
        "open-why_import",
        json!({"scope":"scope-a","rows":[new_row,conflict]}),
    );
    assert!(rejected_import_error);
    assert_eq!(rejected_import["contract"], "open-why.rationale-import/v1");
    assert_eq!(rejected_import["code"], "identity_conflict");
    assert_eq!(
        rejected_import["message"],
        "record identity conflicts with sealed evidence"
    );
    assert!(rejected_import["message"].as_str().unwrap().len() <= 128);
    assert!(!serde_json::to_string(&rejected_import)
        .unwrap()
        .contains("changed secret body"));
    let (preserved, preserved_error) = server.call(
        53,
        "open-why_get",
        json!({"id":"mcp-import","scope":"scope-a"}),
    );
    assert!(!preserved_error);
    assert_eq!(preserved["record"]["content"], "sealed MCP body");
    assert_eq!(preserved["git_refs"].as_array().unwrap().len(), 1);
    let (missing_batch_row, missing_batch_row_error) = server.call(
        54,
        "open-why_get",
        json!({"id":"mcp-must-not-insert","scope":"scope-a"}),
    );
    assert!(missing_batch_row_error);
    assert_eq!(missing_batch_row["code"], "not_found");
    server.finish();

    let mut fresh = Server::spawn(&db_path);
    let (fresh_digest, fresh_tools_json) = initialize(&mut fresh, 100);
    assert_eq!(digest, fresh_digest);
    assert_eq!(tools_json, fresh_tools_json);
    fresh.finish();

    std::fs::remove_dir_all(db_path.parent().unwrap()).unwrap();
}

#[test]
fn scoped_commit_link_real_process_replays_and_redacts_authority() {
    let db_path = unique_temp_db("scoped-link");
    let mut foreign = record("foreign-link", "foreign secret".to_owned(), None);
    foreign.scope = "scope-b".to_owned();
    Store::open_with_store_instance_id(&db_path, &provider_id_for(&db_path))
        .unwrap()
        .import_external(&[
            record("sealed-link", "sealed body".to_owned(), None),
            foreign,
        ])
        .unwrap();
    let observer = Connection::open(&db_path).unwrap();
    let mut server = Server::spawn(&db_path);
    initialize(&mut server, 1);

    let arguments = json!({
        "commit":"abc123",
        "decision":"sealed-link",
        "subject":"Create link",
        "scope":"scope-a"
    });
    let (created, created_error) = server.call(10, "open-why_link", arguments.clone());
    assert!(!created_error);
    assert_eq!(
        created,
        json!({"status":"ok","scope":"scope-a","decision":"sealed-link","commit":"abc123"})
    );
    let version_after_create: i64 = observer
        .pragma_query_value(None, "data_version", |row| row.get(0))
        .unwrap();

    let (replay, replay_error) = server.call(11, "open-why_link", arguments);
    assert!(!replay_error);
    assert_eq!(replay, created);
    assert_eq!(
        observer
            .pragma_query_value(None, "data_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        version_after_create
    );

    let (conflict, conflict_error) = server.call(
        12,
        "open-why_link",
        json!({"commit":"abc123","decision":"sealed-link","subject":"Changed secret","scope":"scope-a"}),
    );
    assert!(conflict_error);
    assert_eq!(conflict["code"], "link_conflict");
    assert_eq!(
        conflict["message"],
        "commit link already exists with a different subject"
    );
    assert!(!serde_json::to_string(&conflict)
        .unwrap()
        .contains("Changed secret"));
    assert_eq!(
        observer
            .pragma_query_value(None, "data_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        version_after_create
    );

    observer
        .execute_batch(
            "CREATE TRIGGER reject_mcp_link
             BEFORE INSERT ON decision_git_refs
             WHEN NEW.commit_hash='reject-insert'
             BEGIN SELECT RAISE(ABORT, 'private sqlite detail'); END",
        )
        .unwrap();
    let (store_failure, store_failure_error) = server.call(
        15,
        "open-why_link",
        json!({"commit":"reject-insert","decision":"sealed-link","scope":"scope-a"}),
    );
    assert!(store_failure_error);
    assert_eq!(store_failure["code"], "store_unavailable");
    assert_eq!(store_failure["message"], "commit link store is unavailable");
    assert_eq!(store_failure["retryable"], false);
    assert!(!serde_json::to_string(&store_failure)
        .unwrap()
        .contains("private sqlite detail"));
    observer
        .execute_batch("DROP TRIGGER reject_mcp_link")
        .unwrap();

    let (foreign, foreign_error) = server.call(
        13,
        "open-why_link",
        json!({"commit":"private-commit","decision":"foreign-link","scope":"scope-a"}),
    );
    let (missing, missing_error) = server.call(
        14,
        "open-why_link",
        json!({"commit":"private-commit","decision":"missing-link","scope":"scope-a"}),
    );
    assert!(foreign_error && missing_error);
    assert_eq!(foreign, missing);
    assert_eq!(foreign["code"], "not_found");
    assert_eq!(
        foreign["message"],
        "record is unavailable in the requested scope"
    );
    let wire = serde_json::to_string(&foreign).unwrap();
    assert!(!wire.contains("foreign-link"));
    assert!(!wire.contains("foreign secret"));
    assert_eq!(
        observer
            .pragma_query_value(None, "data_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        version_after_create
    );
    let subject: String = observer
        .query_row(
            "SELECT commit_subject FROM decision_git_refs
             WHERE decision_id='sealed-link' AND commit_hash='abc123'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(subject, "Create link");
    let private_links: i64 = observer
        .query_row(
            "SELECT count(*) FROM decision_git_refs WHERE commit_hash='private-commit'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(private_links, 0);

    server.finish();
    drop(observer);
    std::fs::remove_dir_all(db_path.parent().unwrap()).unwrap();
}

#[test]
fn scoped_commit_link_real_process_redacts_pre_read_and_write_lock_failures() {
    fn locked_call(wal: bool) -> Value {
        let db_path = unique_temp_db(if wal {
            "link-write-lock"
        } else {
            "link-read-lock"
        });
        Store::open_with_store_instance_id(&db_path, &provider_id_for(&db_path))
            .unwrap()
            .import_external(&[record("sealed-link", "sealed body".to_owned(), None)])
            .unwrap();
        if wal {
            Connection::open(&db_path)
                .unwrap()
                .pragma_update(None, "journal_mode", "WAL")
                .unwrap();
        }
        let mut server = Server::spawn(&db_path);
        initialize(&mut server, 1);
        let writer = Connection::open(&db_path).unwrap();
        writer.busy_timeout(std::time::Duration::ZERO).unwrap();
        writer
            .execute_batch(if wal {
                "BEGIN IMMEDIATE"
            } else {
                "BEGIN EXCLUSIVE"
            })
            .unwrap();
        let (payload, is_error) = server.call(
            10,
            "open-why_link",
            json!({"commit":"abc123","decision":"sealed-link","scope":"scope-a"}),
        );
        assert!(is_error);
        writer.execute_batch("ROLLBACK").unwrap();
        drop(writer);
        server.finish();
        let links: i64 = Connection::open(&db_path)
            .unwrap()
            .query_row(
                "SELECT count(*) FROM decision_git_refs WHERE commit_hash='abc123'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(links, 0);
        std::fs::remove_dir_all(db_path.parent().unwrap()).unwrap();
        payload
    }

    let pre_read = locked_call(false);
    let begin_immediate = locked_call(true);
    assert_eq!(pre_read, begin_immediate);
    assert_eq!(pre_read["code"], "store_unavailable");
    assert_eq!(pre_read["message"], "commit link store is unavailable");
    assert_eq!(pre_read["retryable"], true);
    let wire = serde_json::to_string(&pre_read).unwrap();
    assert!(!wire.contains("database"));
    assert!(!wire.contains("locked"));
    assert!(!wire.contains("SQLite"));
}
