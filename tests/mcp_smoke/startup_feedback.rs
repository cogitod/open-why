use super::*;

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
