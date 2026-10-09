//! Public first-use flows exercised against the installed-shape CLI and MCP processes.
mod onboarding_support;

use onboarding_support::*;
use open_why::{inspect_store, Store, StoreCompatibility};
use serde_json::json;
use std::net::TcpListener;
use std::process::Command;

#[test]
fn setup_binds_once_reuses_identity_and_prints_parseable_client_formats() {
    let sandbox = Sandbox::new();
    let db = sandbox
        .0
        .join("store with 'quotes' and \"double\" \\ unicode-é\u{7f}.db");
    let first = setup(&sandbox, &db, "generic");
    let entry = parse_config(&first.stdout, "generic");
    assert_eq!(entry["env"]["OPEN_WHY_DB"], db.to_str().unwrap());
    assert_eq!(entry["args"], json!(["serve"]));
    assert!(std::path::Path::new(entry["command"].as_str().unwrap()).is_absolute());
    let identity = entry["env"]["OPEN_WHY_STORE_INSTANCE_ID"].as_str().unwrap();
    assert!(identity.starts_with("open-why:"));
    assert_eq!(identity.len(), "open-why:".len() + 32);
    let bytes = std::fs::read(&db).unwrap();
    for client in ["codex", "claude-code", "generic"] {
        let output = setup(&sandbox, &db, client);
        let next = parse_config(&output.stdout, client);
        assert_eq!(next["command"], entry["command"]);
        assert_eq!(next["env"], entry["env"]);
        assert_eq!(std::fs::read(&db).unwrap(), bytes);
    }
    let other = setup(&sandbox, &sandbox.0.join("other.db"), "generic");
    assert_ne!(parse_config(&other.stdout, "generic")["env"], entry["env"]);
    assert!(!sandbox.0.join(".codex").exists());
    assert!(!sandbox.0.join(".mcp.json").exists());
    assert!(!sandbox.0.join(".cache").exists());
}

#[test]
fn explicit_identity_is_honored_and_mismatches_do_not_mutate() {
    let sandbox = Sandbox::new();
    let db = sandbox.0.join("explicit.db");
    let output = sandbox
        .command()
        .args(["setup", "--client", "generic", "--db"])
        .arg(&db)
        .env("OPEN_WHY_STORE_INSTANCE_ID", "test:explicit")
        .output()
        .unwrap();
    success(&output);
    assert_eq!(
        parse_config(&output.stdout, "generic")["env"]["OPEN_WHY_STORE_INSTANCE_ID"],
        "test:explicit"
    );
    let bytes = std::fs::read(&db).unwrap();
    for command in ["setup", "doctor"] {
        let mut process = sandbox.command();
        process
            .arg(command)
            .arg("--db")
            .arg(&db)
            .env("OPEN_WHY_STORE_INSTANCE_ID", "test:wrong");
        if command == "setup" {
            process.args(["--client", "generic"]);
        }
        let output = process.output().unwrap();
        failure(&output, "identity_mismatch");
        assert_eq!(std::fs::read(&db).unwrap(), bytes);
    }
    let invalid_path = sandbox.0.join("invalid/sub/store.db");
    let output = sandbox
        .command()
        .args(["setup", "--client", "generic", "--db"])
        .arg(&invalid_path)
        .env("OPEN_WHY_STORE_INSTANCE_ID", "invalid/id")
        .output()
        .unwrap();
    failure(&output, "invalid_identity");
    assert!(!sandbox.0.join("invalid").exists());
}

#[test]
fn setup_and_doctor_refuse_unsafe_existing_stores_without_changes() {
    let sandbox = Sandbox::new();
    for name in ["empty.db", "foreign.db", "newer.db", "wal.db"] {
        let db = sandbox.0.join(name);
        match name {
            "empty.db" => std::fs::write(&db, []).unwrap(),
            "foreign.db" => std::fs::write(&db, b"not a sqlite store").unwrap(),
            _ => {
                drop(Store::open_with_store_instance_id(&db, "test:existing").unwrap());
                if name == "newer.db" {
                    rusqlite::Connection::open(&db)
                        .unwrap()
                        .pragma_update(None, "user_version", 999)
                        .unwrap();
                } else {
                    std::fs::write(format!("{}-wal", db.display()), b"indeterminate").unwrap();
                }
            }
        }
        let before = snapshot(&sandbox.0);
        for command in ["setup", "doctor"] {
            let mut process = sandbox.command();
            process.arg(command).arg("--db").arg(&db);
            if command == "setup" {
                process.args(["--client", "generic"]);
            }
            let output = process.output().unwrap();
            assert!(!output.status.success());
            if name == "wal.db" {
                failure(&output, "live_wal_indeterminate");
            }
            if command == "setup" {
                assert!(output.stdout.is_empty());
            }
            assert_eq!(snapshot(&sandbox.0), before);
        }
    }
}

#[test]
fn doctor_does_not_create_missing_paths_or_contact_configured_endpoint() {
    let sandbox = Sandbox::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}/private-token", listener.local_addr().unwrap());
    let missing = sandbox.0.join("missing/nested/store.db");
    let before = snapshot(&sandbox.0);
    let output = sandbox
        .command()
        .args(["doctor", "--db"])
        .arg(&missing)
        .env("OPEN_WHY_EMBED_URL", &endpoint)
        .env("OPEN_WHY_EMBED_API_KEY", "never-print-this-value")
        .output()
        .unwrap();
    failure(&output, "store is missing");
    assert_eq!(snapshot(&sandbox.0), before);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("not verified"));
    assert!(!text.contains("private-token"));
    assert!(!text.contains("never-print-this-value"));
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );

    let db = sandbox.0.join("ready.db");
    let setup_output = sandbox
        .command()
        .args(["setup", "--client", "generic", "--db"])
        .arg(&db)
        .env("OPEN_WHY_EMBED_URL", &endpoint)
        .env("OPEN_WHY_AUTO_FETCH", "1")
        .output()
        .unwrap();
    success(&setup_output);
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    let before = snapshot(&sandbox.0);
    let output = sandbox
        .command()
        .args(["doctor", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("lexical retrieval"));
    assert_eq!(snapshot(&sandbox.0), before);
    let output = sandbox
        .command()
        .args(["doctor", "--db"])
        .arg(&db)
        .env("OPEN_WHY_EMBED_MODEL_PATH", sandbox.0.join("absent-model"))
        .output()
        .unwrap();
    failure(
        &output,
        if cfg!(feature = "local-embeddings") {
            "model is missing"
        } else {
            "local-embeddings Cargo feature"
        },
    );
    assert_eq!(snapshot(&sandbox.0), before);
}

#[test]
fn relative_and_symlink_store_paths_are_rejected() {
    let sandbox = Sandbox::new();
    let before = snapshot(&sandbox.0);
    let output = sandbox
        .command()
        .current_dir(&sandbox.0)
        .args(["setup", "--db", "relative.db", "--client", "generic"])
        .output()
        .unwrap();
    failure(&output, "absolute");
    assert_eq!(snapshot(&sandbox.0), before);
    std::os::unix::fs::symlink(&sandbox.0, sandbox.0.join("alias")).unwrap();
    let output = sandbox
        .command()
        .args(["setup", "--client", "generic", "--db"])
        .arg(sandbox.0.join("alias/store.db"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!sandbox.0.join("store.db").exists());
}

#[test]
fn every_client_config_launches_real_mcp_and_completes_demo() {
    for client in ["generic", "codex", "claude-code"] {
        let sandbox = Sandbox::new();
        let demo = sandbox.0.join("demo with 'single' and \"double\" quotes");
        let executable = sandbox.0.join("why with 'single' and \"double\" quotes");
        std::fs::copy(env!("CARGO_BIN_EXE_why"), &executable).unwrap();
        let mut script = Command::new("bash");
        sandbox.isolate(&mut script);
        script
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/examples/quickstart.sh"
            ))
            .arg(&demo)
            .arg(client)
            .env("OPEN_WHY_BIN", &executable)
            .env("OPEN_WHY_STORE_INSTANCE_ID", "test:must-not-reuse");
        let output = script.output().unwrap();
        success(&output);
        let config_file = if client == "codex" {
            "mcp.toml"
        } else {
            "mcp.json"
        };
        let entry = parse_config(&std::fs::read(demo.join(config_file)).unwrap(), client);
        assert_ne!(
            entry["env"]["OPEN_WHY_STORE_INSTANCE_ID"],
            "test:must-not-reuse"
        );
        let before = snapshot(&demo);
        let repeat = script.output().unwrap();
        failure(&repeat, "Destination already exists");
        assert_eq!(snapshot(&demo), before);

        let repo = demo.join("repository");
        let db = demo.join("open-why.db");
        let doctor = sandbox
            .command()
            .args(["doctor", "--db"])
            .arg(&db)
            .arg("--repo")
            .arg(&repo)
            .output()
            .unwrap();
        success(&doctor);
        let invalid_repo = sandbox
            .command()
            .args(["doctor", "--db"])
            .arg(&db)
            .arg("--repo")
            .arg(&demo)
            .output()
            .unwrap();
        failure(&invalid_repo, "Git installation and repository path");
        let mut server = Server::spawn(&sandbox, &entry);
        let init = server.request("initialize", json!({"protocolVersion":"2024-11-05", "capabilities":{}, "clientInfo":{"name":"onboarding-test","version":"1"}}));
        assert_eq!(init["result"]["serverInfo"]["name"], "open-why");
        server.notify("notifications/initialized");
        let tools = server.request("tools/list", json!({}));
        assert!(tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "open-why_ask"));
        let answer = server.tool(
            "open-why_ask",
            json!({"repo":repo,"question":"why SQLite offline notebook"}),
        );
        let id = answer["results"][0]["id"]
            .as_str()
            .expect("demo decision found");
        assert_eq!(answer["scope"], repo.to_str().unwrap());
        let evidence = server.tool("open-why_get", json!({"id":id,"scope":repo}));
        assert!(evidence["record"]["content"]
            .as_str()
            .unwrap()
            .contains("one writer"));
        let commit = open_why::miner::git(&repo, &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(evidence["record"]["commit_sha"], commit.trim());
        let unknown = server.tool(
            "open-why_ask",
            json!({"repo":repo,"question":"encryption algorithm"}),
        );
        assert!(unknown["results"].as_array().unwrap().is_empty());

        let original = server.tool("open-why_capture", json!({"scope":repo,"title":"Demo storage choice","content":"SQLite for offline use on one laptop with one writer."}));
        let old_id = original["id"].as_str().unwrap();
        let replacement = server.tool("open-why_capture", json!({"scope":repo,"title":"Demo storage choice","content":"PostgreSQL because multiple machines must write concurrently.","supersedes":old_id}));
        let current = server.tool("open-why_get", json!({"scope":repo,"id":old_id}));
        assert_eq!(current["record"]["id"], replacement["id"]);
        let history = server.tool("open-why_history", json!({"scope":repo,"id":old_id}));
        assert_eq!(history["records"].as_array().unwrap().len(), 2);
        drop(server);
        assert!(matches!(
            inspect_store(&db).unwrap(),
            StoreCompatibility::Compatible { .. }
        ));
        assert!(!sandbox.0.join(".cache").exists());
    }
}
