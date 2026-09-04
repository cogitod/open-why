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

#[path = "mcp_smoke/commit_links.rs"]
mod commit_links;
#[path = "mcp_smoke/exact_contracts.rs"]
mod exact_contracts;
#[path = "mcp_smoke/startup_feedback.rs"]
mod startup_feedback;
