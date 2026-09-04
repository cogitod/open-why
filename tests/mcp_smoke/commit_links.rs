use super::*;

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
