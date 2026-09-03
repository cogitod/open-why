//! Smoke test for `why serve`: spawns the real binary, speaks the MCP stdio protocol
//! (raw JSON-RPC, no SDK on either side — see `src/mcp.rs`), and checks the tool
//! catalog it advertises. Not a correctness test of any tool's behavior, just proof
//! the server starts, understands `initialize`/`tools/list`, and exits cleanly on EOF.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn unique_temp_db() -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("open-why-mcp-smoke-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("t.db")
}

#[test]
fn serve_advertises_all_tools_over_stdio() {
    let db_path = unique_temp_db();

    let mut child = Command::new(env!("CARGO_BIN_EXE_why"))
        .arg("serve")
        .env("OPEN_WHY_DB", &db_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("failed to spawn `why serve`");

    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{}}}}"#
    )
    .unwrap();
    let mut init_line = String::new();
    stdout.read_line(&mut init_line).unwrap();
    let init: serde_json::Value = serde_json::from_str(&init_line).unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], "open-why");

    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{{}}}}"#
    )
    .unwrap();
    let mut list_line = String::new();
    stdout.read_line(&mut list_line).unwrap();
    let list: serde_json::Value = serde_json::from_str(&list_line).unwrap();
    let tools = list["result"]["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();

    for expected in [
        "open-why_ask",
        "open-why_index",
        "open-why_capture",
        "open-why_import",
        "open-why_search",
        "open-why_get",
        "open-why_link",
        "open-why_feedback",
    ] {
        assert!(
            names.contains(&expected),
            "missing tool {expected} in {names:?}"
        );
    }

    // Closing stdin sends EOF; `serve()`'s read loop exits and the process returns Ok(()).
    drop(stdin);
    let status = child.wait().expect("why serve did not exit cleanly");
    assert!(status.success());

    let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
}
