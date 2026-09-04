//! Real-process compatibility checks for the optional `why` CLI.

use open_why::{ExternalDecision, Store};
use rusqlite::Connection;
use std::process::Command;

fn unique_temp_db() -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("open-why-cli-link-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("test.db")
}

#[test]
fn legacy_link_command_keeps_its_invocation_output_and_storage() {
    let db_path = unique_temp_db();
    let provider = "cli-test:scoped-link";
    let record = ExternalDecision {
        id: "cli-link".to_owned(),
        kind: "decision".to_owned(),
        title: "CLI link".to_owned(),
        content: "Compatibility fixture".to_owned(),
        importance: 0.5,
        source: "cli-smoke".to_owned(),
        author: "tester".to_owned(),
        date: "2026-01-01".to_owned(),
        updated_at: None,
        accessed_count: None,
        times_injected: None,
        effectiveness: None,
        tags: None,
        scope: "global".to_owned(),
        valid_from: Some("2026-01-01T00:00:00Z".to_owned()),
        valid_until: None,
        superseded_by: None,
        fact_key: None,
        git_refs: Vec::new(),
    };
    Store::open_with_store_instance_id(&db_path, provider)
        .unwrap()
        .import_external(&[record])
        .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_why"))
        .args(["link", "abc123", "cli-link", "--subject", "CLI subject"])
        .env("OPEN_WHY_DB", &db_path)
        .env("OPEN_WHY_STORE_INSTANCE_ID", provider)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "linked abc123 -> cli-link\n"
    );
    assert!(output.stderr.is_empty());

    let stored: (String, String, String) = Connection::open(&db_path)
        .unwrap()
        .query_row(
            "SELECT decision_id,commit_hash,commit_subject FROM decision_git_refs
             WHERE decision_id='cli-link' AND commit_hash='abc123'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        stored,
        (
            "cli-link".to_owned(),
            "abc123".to_owned(),
            "CLI subject".to_owned()
        )
    );

    let help = Command::new(env!("CARGO_BIN_EXE_why"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8(help.stdout)
        .unwrap()
        .contains("Link a git commit"));

    std::fs::remove_dir_all(db_path.parent().unwrap()).unwrap();
}
