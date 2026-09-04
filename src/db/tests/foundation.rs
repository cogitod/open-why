use super::super::*;
use super::support::*;

#[test]
fn record_digest_v1_has_stable_null_unicode_and_float_vectors() {
    let base = RecordDigestRow {
        id: "record\0id".to_owned(),
        scope: "/repo/alpha".to_owned(),
        kind: "decision".to_owned(),
        title: "Use SQLite".to_owned(),
        content: "local-first\nreason".to_owned(),
        importance: -0.0,
        source: "capture".to_owned(),
        author: "agent".to_owned(),
        commit_sha: "abc123".to_owned(),
        date: "2026-09-03T12:34:56Z".to_owned(),
        tags: None,
        fact_key: None,
        valid_from: None,
        declared_valid_until: None,
        sealed_digest: None,
    };
    let mut unicode = base.clone();
    unicode.id = "记录-β".to_owned();
    unicode.title = "为什么 SQLite?".to_owned();
    unicode.importance = 0.125;
    unicode.tags = Some("[\"β\",\"alpha\",\"\"]".to_owned());
    unicode.fact_key = Some(String::new());
    unicode.valid_from = Some("2026-09-03T12:34:56.123Z".to_owned());
    unicode.declared_valid_until = Some(String::new());

    assert_eq!(
        (
            record_digest_v1(&base).unwrap(),
            record_digest_v1(&unicode).unwrap()
        ),
        (
            "a68e21b2b9e9ed1d5f2ebb0c47390b1c06a927589b8d7ac8e6a3dbafa9412bd7".to_owned(),
            "e7393a1403bffa15cfb05e72f40e8ba177984adbd39751f7ff9dbf8f50e8efaa".to_owned(),
        )
    );

    let mut left = base.clone();
    left.title = "a\0b".to_owned();
    left.content = "c".to_owned();
    let mut right = base;
    right.title = "a".to_owned();
    right.content = "b\0c".to_owned();
    assert_ne!(
        record_digest_v1(&left).unwrap(),
        record_digest_v1(&right).unwrap()
    );
}

#[test]
fn migration_failure_rolls_back_every_foundation_effect_and_retries() {
    let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!(
            "open-why-migration-rollback-{}-{n}",
            std::process::id()
        ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("legacy.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(LEGACY_SCHEMA_V0_SQL).unwrap();
    conn.execute_batch(
        "INSERT INTO decisions
               (id,kind,title,content,importance,source,author,commit_sha,date,scope,
                valid_until,content_digest,source_identity,created_epoch)
             VALUES ('legacy','decision','Legacy','body',0.5,'import','author','',
                     '2025-01-01','repo-a','2027-01-01','old','legacy',1);",
    )
    .unwrap();
    let schema_snapshot = |conn: &Connection| {
        let mut statement = conn
            .prepare(
                "SELECT type,name,tbl_name,COALESCE(sql,'') FROM sqlite_schema
                     ORDER BY type,name,tbl_name",
            )
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    let decisions_snapshot = |conn: &Connection| {
        conn.query_row(
            "SELECT hex(CAST(id AS BLOB)),hex(CAST(title AS BLOB)),
                        hex(CAST(content AS BLOB)),hex(CAST(valid_until AS BLOB))
                 FROM decisions WHERE id='legacy'",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .unwrap()
    };
    let fts_snapshot = |conn: &Connection| {
        conn.query_row(
            "SELECT count(*),COALESCE(group_concat(rowid || ':' || title,','),'')
                 FROM decisions_fts",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .unwrap()
    };
    let schema_before = schema_snapshot(&conn);
    let decisions_before = decisions_snapshot(&conn);
    let fts_before = fts_snapshot(&conn);
    let bytes_before = std::fs::read(&path).unwrap();
    let sidecars = [
        PathBuf::from(format!("{}-journal", path.display())),
        PathBuf::from(format!("{}-wal", path.display())),
        PathBuf::from(format!("{}-shm", path.display())),
    ];
    assert!(sidecars.iter().all(|sidecar| !sidecar.exists()));
    let store = Store {
        conn,
        embedder: None,
        _store_parent: None,
    };
    let error = store
        .migrate_with_hook(Some("provider:rollback"), |_| {
            anyhow::bail!("injected migration crash")
        })
        .unwrap_err();
    assert!(error.to_string().contains("injected migration crash"));
    assert_eq!(schema_snapshot(&store.conn), schema_before);
    assert_eq!(decisions_snapshot(&store.conn), decisions_before);
    assert_eq!(fts_snapshot(&store.conn), fts_before);
    assert_eq!(std::fs::read(&path).unwrap(), bytes_before);
    assert!(sidecars.iter().all(|sidecar| !sidecar.exists()));
    let version: u32 = store
        .conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 0);
    assert!(!object_exists(&store.conn, "table", "open_why_metadata").unwrap());
    assert!(!object_exists(&store.conn, "table", "open_why_migrations").unwrap());
    assert!(matches!(
        inspect_connection(&store.conn),
        StoreCompatibility::MigrationRequired { .. }
    ));
    store
        .migrate_with_provider_identity(Some("provider:rollback"))
        .unwrap();
    let first = store.store_identity().unwrap();
    store
        .migrate_with_provider_identity(Some("provider:rollback"))
        .unwrap();
    assert_eq!(store.store_identity().unwrap(), first);
    assert!(matches!(
        inspect_store(&path).unwrap(),
        StoreCompatibility::Compatible { identity } if identity == first
    ));
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}
