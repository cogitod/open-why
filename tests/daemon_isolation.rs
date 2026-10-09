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
    while !path.with_extension("sock").exists()
        && !Path::new(&format!("{}.sock", path.display())).exists()
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
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n")
        .unwrap();
    child.wait_with_output().unwrap()
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
