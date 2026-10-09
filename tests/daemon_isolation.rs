#[allow(dead_code)]
mod onboarding_support;
use onboarding_support::{Sandbox, Server};
use open_why::Store;
use serde_json::json;
use std::{
    io::Write,
    path::Path,
    process::{Child, Stdio},
    time::{Duration, Instant},
};

struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn start(s: &Sandbox, path: &Path, id: &str) -> Daemon {
    let child = s
        .command()
        .env("OPEN_WHY_DB", path)
        .env("OPEN_WHY_STORE_INSTANCE_ID", id)
        .arg("serve-daemon")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut daemon = Daemon(child);
    let end = Instant::now() + Duration::from_secs(10);
    use std::os::unix::fs::PermissionsExt;
    while !std::fs::metadata(format!("{}.sock", path.display()))
        .is_ok_and(|m| m.permissions().mode() & 0o077 == 0)
    {
        assert!(daemon.0.try_wait().unwrap().is_none(), "daemon exited");
        assert!(Instant::now() < end, "daemon startup timeout");
        std::thread::sleep(Duration::from_millis(10));
    }
    daemon
}
fn exchange(s: &Sandbox, path: &Path, id: &str) -> std::process::Output {
    let mut child = s
        .command()
        .env("OPEN_WHY_DB", path)
        .env("OPEN_WHY_STORE_INSTANCE_ID", id)
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let written = child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n");
    // Rejected startup may close stdin before the parent writes, especially on Linux.
    if let Err(error) = written {
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    }
    let output = child.wait_with_output().unwrap();
    if output.status.success() {
        assert!(String::from_utf8_lossy(&output.stdout).contains("serverInfo"));
    }
    output
}
#[test]
fn identity_is_enforced_directly_and_through_daemon() {
    let s = Sandbox::new();
    let db = s.0.join("store.db");
    drop(Store::open_with_store_instance_id(&db, "test:a").unwrap());
    assert!(!exchange(&s, &db, "test:wrong").status.success());
    let _daemon = start(&s, &db, "test:a");
    assert!(
        !exchange(&s, &db, "test:wrong").status.success(),
        "daemon bypassed identity"
    );
    assert!(exchange(&s, &db, "test:a").status.success());
}
#[test]
fn colliding_basename_stores_remain_isolated() {
    let s = Sandbox::new();
    let a = s.0.join("store.db");
    let b = s.0.join("store.sqlite");
    for (db, id) in [(&a, "test:a"), (&b, "test:b")] {
        let out = s
            .command()
            .env("OPEN_WHY_DB", db)
            .env("OPEN_WHY_STORE_INSTANCE_ID", id)
            .args([
                "capture",
                "--id",
                id,
                "--title",
                "isolationmarker",
                "--content",
                id,
            ])
            .output()
            .unwrap();
        onboarding_support::success(&out);
    }
    let _a = start(&s, &a, "test:a");
    let entry = |db: &Path, id: &str| json!({"command":env!("CARGO_BIN_EXE_why"),"args":["serve"],"env":{"OPEN_WHY_DB":db,"OPEN_WHY_STORE_INSTANCE_ID":id}});
    let mut client = Server::spawn(&s, &entry(&b, "test:b"));
    let result = client.request(
        "tools/call",
        json!({"name":"open-why_search","arguments":{"query":"isolationmarker","scope":"global"}}),
    );
    let text = result.to_string();
    assert!(text.contains("test:b"), "wrong store: {text}");
    assert!(!text.contains("test:a"), "foreign record leaked: {text}");
    let _b = start(&s, &b, "test:b");
    std::thread::scope(|threads| {
        let s = &s;
        let entry = &entry;
        for (db, id, foreign) in [(&a, "test:a", "test:b"), (&b, "test:b", "test:a")] {
            threads.spawn(move || {
                let mut client = Server::spawn(s, &entry(db, id));
                let result = client.tool("open-why_search", json!({"query":"isolationmarker", "scope":"global"}));
                assert!(result.to_string().contains(id));
                assert!(!result.to_string().contains(foreign));
                let result = client.request("tools/call", json!({"name":"open-why_feedback","arguments":{"id":foreign,"scope":"global","helpful":true}}));
                assert_eq!(result["result"]["isError"], true);
            });
        }
    });
    for _ in 0..3 {
        assert!(exchange(&s, &a, "test:a").status.success());
        assert!(exchange(&s, &b, "test:b").status.success());
    }
}

#[test]
fn concurrent_clients_and_endpoint_failures_are_safe() {
    use std::os::unix::net::UnixListener;
    let s = Sandbox::new();
    let db = s.0.join("store.db");
    drop(Store::open_with_store_instance_id(&db, "test:a").unwrap());
    let socket = s.0.join("store.db.sock");
    std::fs::write(&socket, b"unrelated file").unwrap();
    assert!(!exchange(&s, &db, "test:a").status.success());
    let out = s
        .command()
        .env("OPEN_WHY_DB", &db)
        .arg("serve-daemon")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(std::fs::read(&socket).unwrap(), b"unrelated file");
    std::fs::remove_file(&socket).unwrap();
    drop(UnixListener::bind(&socket).unwrap());
    assert!(!exchange(&s, &db, "test:a").status.success());
    std::fs::remove_file(&socket).unwrap();
    let _daemon = start(&s, &db, "test:a");
    std::thread::scope(|scope| {
        for _ in 0..6 {
            scope.spawn(|| {
                assert!(exchange(&s, &db, "test:a").status.success());
            });
        }
    });
    let out = s
        .command()
        .env("OPEN_WHY_DB", &db)
        .arg("serve-daemon")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(exchange(&s, &db, "test:a").status.success());
}

#[test]
fn replaced_database_and_ambiguous_paths_are_rejected() {
    let s = Sandbox::new();
    let db = s.0.join("store.db");
    drop(Store::open_with_store_instance_id(&db, "test:a").unwrap());
    let _daemon = start(&s, &db, "test:a");
    std::fs::rename(&db, s.0.join("original.db")).unwrap();
    drop(Store::open_with_store_instance_id(&db, "test:a").unwrap());
    assert!(!exchange(&s, &db, "test:a").status.success());
    assert!(!exchange(&s, &s.0.join("../relative.db"), "test:a")
        .status
        .success());
    std::os::unix::fs::symlink(&db, s.0.join("alias.db")).unwrap();
    assert!(!exchange(&s, &s.0.join("alias.db"), "test:a")
        .status
        .success());
    std::fs::hard_link(&db, s.0.join("hard.db")).unwrap();
    assert!(!exchange(&s, &s.0.join("hard.db"), "test:a")
        .status
        .success());
}

#[test]
fn proxy_exits_when_daemon_dies_even_with_client_stdin_open() {
    use std::io::{BufRead, BufReader};
    let s = Sandbox::new();
    let db = s.0.join("store.db");
    drop(Store::open_with_store_instance_id(&db, "test:a").unwrap());
    let daemon = start(&s, &db, "test:a");
    let mut proxy = s
        .command()
        .env("OPEN_WHY_DB", &db)
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    proxy
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n")
        .unwrap();
    let mut line = String::new();
    BufReader::new(proxy.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert!(line.contains("serverInfo"));
    drop(daemon);
    let deadline = Instant::now() + Duration::from_secs(5);
    while proxy.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = proxy.kill();
            let _ = proxy.wait();
            panic!("proxy hung after daemon exit");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn redirected_endpoint_cannot_authorize_another_physical_store() {
    let s = Sandbox::new();
    let a = s.0.join("a.db");
    let b = s.0.join("b.db");
    // Copied stores can deliberately share a logical identity, so identity alone is insufficient.
    for path in [&a, &b] {
        drop(Store::open_with_store_instance_id(path, "test:shared").unwrap());
    }
    let _daemon = start(&s, &a, "test:shared");
    std::fs::rename(s.0.join("a.db.sock"), s.0.join("b.db.sock")).unwrap();
    let result = exchange(&s, &b, "test:shared");
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
}

#[test]
fn oversized_requests_close_direct_and_daemon_sessions_without_stopping_daemon() {
    use std::io::Read;
    let s = Sandbox::new();
    let db = s.0.join("store.db");
    drop(Store::open_with_store_instance_id(&db, "test:bounded").unwrap());
    let mut daemon = None;
    for mediated in [false, true] {
        if mediated {
            daemon = Some(start(&s, &db, "test:bounded"));
        }
        let mut client = Daemon(
            s.command()
                .env("OPEN_WHY_DB", &db)
                .env("OPEN_WHY_STORE_INSTANCE_ID", "test:bounded")
                .arg("serve")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        // Leave stdin open and omit LF: rejection cannot rely on client EOF.
        let mut stdin = client.0.stdin.take().unwrap();
        let writer = std::thread::spawn(move || {
            stdin.write_all(&vec![b' '; 8 * 1024 * 1024 + 1]).unwrap();
            stdin
        });
        let end = Instant::now() + Duration::from_secs(5);
        while client.0.try_wait().unwrap().is_none() {
            assert!(Instant::now() < end, "oversized request kept session alive");
            std::thread::sleep(Duration::from_millis(10));
        }
        let _stdin = writer.join().unwrap();
        let mut output = String::new();
        client
            .0
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut output)
            .unwrap();
        let error: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(error["error"]["code"], -32600);
        assert_eq!(error["id"], serde_json::Value::Null);
        assert!(output.len() < 256);
        assert!(exchange(&s, &db, "test:bounded").status.success());
        if let Some(daemon) = &mut daemon {
            assert!(daemon.0.try_wait().unwrap().is_none());
        }
    }
}
