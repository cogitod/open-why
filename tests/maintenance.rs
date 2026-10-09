#[allow(dead_code)]
mod onboarding_support;
use onboarding_support::{failure, success, Sandbox};
use open_why::{Decision, Store};

struct Daemon(std::process::Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn cli_backup_verify_restore_round_trip_and_refusals() {
    let s = Sandbox::new();
    let db = s.0.join("source.db");
    let backup = s.0.join("backup.db");
    let restored = s.0.join("restored.db");
    let store = Store::open_with_store_instance_id(&db, "test:maintenance").unwrap();
    let id = store
        .capture(
            &Decision {
                subject: "Offline rationale".into(),
                body: "SQLite keeps data available".into(),
                kind: "decision".into(),
                ..Default::default()
            },
            "example",
            None,
        )
        .unwrap();
    let original =
        serde_json::to_value(store.evidence_identity_in_scope(&id, "example").unwrap()).unwrap();
    // A live source with uncheckpointed WAL commits must be copied consistently.
    let writer = rusqlite::Connection::open(&db).unwrap();
    writer.pragma_update(None, "journal_mode", "WAL").unwrap();
    writer.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
    store
        .capture(
            &Decision {
                subject: "WAL evidence".into(),
                body: "Committed while another connection is open".into(),
                kind: "decision".into(),
                ..Default::default()
            },
            "example",
            None,
        )
        .unwrap();
    let mut daemon = Daemon(
        s.command()
            .env("OPEN_WHY_DB", &db)
            .arg("serve-daemon")
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !s.0.join("source.db.sock").exists() {
        assert!(daemon.0.try_wait().unwrap().is_none());
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let output = s
        .command()
        .env("OPEN_WHY_DB", &db)
        .args(["backup", "--to"])
        .arg(&backup)
        .output()
        .unwrap();
    success(&output);
    success(
        &s.command()
            .arg("verify-backup")
            .arg(&backup)
            .output()
            .unwrap(),
    );
    success(
        &s.command()
            .arg("restore")
            .arg(&backup)
            .arg("--to")
            .arg(&restored)
            .output()
            .unwrap(),
    );
    let copy = Store::open(&restored).unwrap();
    assert_eq!(
        copy.store_identity().unwrap(),
        store.store_identity().unwrap()
    );
    assert_eq!(copy.count_for_scope("example").unwrap(), 2);
    assert_eq!(
        serde_json::to_value(copy.evidence_identity_in_scope(&id, "example").unwrap()).unwrap(),
        original
    );
    let before = std::fs::read(&restored).unwrap();
    failure(
        &s.command()
            .arg("restore")
            .arg(&backup)
            .arg("--to")
            .arg(&restored)
            .output()
            .unwrap(),
        "already exists",
    );
    assert_eq!(std::fs::read(&restored).unwrap(), before);
    failure(
        &s.command()
            .env("OPEN_WHY_STORE_INSTANCE_ID", "wrong")
            .arg("verify-backup")
            .arg(&backup)
            .output()
            .unwrap(),
        "identity_mismatch",
    );
    let missing = s.0.join("missing.db");
    failure(
        &s.command()
            .env("OPEN_WHY_DB", &missing)
            .args(["backup", "--to"])
            .arg(s.0.join("empty.db"))
            .output()
            .unwrap(),
        "does not exist",
    );
    assert!(!missing.exists());
    std::fs::write(s.0.join("corrupt.db"), b"not a database").unwrap();
    assert!(!s
        .command()
        .arg("restore")
        .arg(s.0.join("corrupt.db"))
        .arg("--to")
        .arg(s.0.join("partial.db"))
        .output()
        .unwrap()
        .status
        .success());
    assert!(!s.0.join("partial.db").exists());
    let tampered = rusqlite::Connection::open(&backup).unwrap();
    let triggers: Vec<(String, String)> = tampered
        .prepare(
            "SELECT name, sql FROM sqlite_master WHERE type='trigger' AND tbl_name='decisions'",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    for (name, _) in &triggers {
        tampered
            .execute_batch(&format!("DROP TRIGGER \"{}\"", name.replace('"', "\"\"")))
            .unwrap();
    }
    tampered
        .execute("UPDATE decisions SET content='tampered' WHERE id=?1", [&id])
        .unwrap();
    for (_, sql) in triggers {
        tampered.execute_batch(&sql).unwrap();
    }
    failure(
        &s.command()
            .arg("verify-backup")
            .arg(&backup)
            .output()
            .unwrap(),
        "digest validation failed",
    );
}
