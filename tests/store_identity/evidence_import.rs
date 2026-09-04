use super::*;

#[test]
fn evidence_identity_is_sealed_and_stable_across_relations_and_lifecycle() {
    let dir = temp_dir("evidence-stability");
    let path = dir.join("store.db");
    let store = Store::open_with_store_instance_id(&path, "provider:evidence").unwrap();
    let mut original = row("stable", "repo-a");
    store
        .import_external_sealed(std::slice::from_ref(&original))
        .unwrap();
    let before = identity(&store, "stable", "repo-a");
    assert_eq!(before.contract, EVIDENCE_IDENTITY_CONTRACT);
    assert_eq!(before.record_digest_contract, RECORD_DIGEST_CONTRACT);

    original.tags = Some("[\"sqlite\",\"identity\"]".to_owned());
    store
        .import_external_sealed(std::slice::from_ref(&original))
        .unwrap();
    store.link_git("stable", "abc123", "Add identity").unwrap();
    store.feedback("stable", true).unwrap();
    let mutable = Connection::open(&path).unwrap();
    mutable
        .execute(
            "UPDATE decisions
             SET accessed_count=42,times_injected=7,effectiveness=0.9,embedding='[0.1]'
             WHERE id='stable'",
            [],
        )
        .unwrap();
    drop(mutable);
    assert_eq!(identity(&store, "stable", "repo-a"), before);

    let replacement = Decision {
        subject: "Replacement rationale".to_owned(),
        body: "A newer rationale supersedes the old record.".to_owned(),
        source: "capture".to_owned(),
        kind: "decision".to_owned(),
        importance: 0.8,
        ..Decision::default()
    };
    store
        .capture_external(
            &replacement,
            "repo-a",
            "replacement",
            Some("2026-01-02T00:00:00Z"),
            None,
            Some("stable"),
        )
        .unwrap();
    assert_eq!(identity(&store, "stable", "repo-a"), before);

    let ScopedCurrentRecordResolution::Ok {
        contract,
        current_id,
        evidence_identity,
        ..
    } = store
        .get_current_evidence_in_scope("stable", "repo-a")
        .unwrap()
    else {
        panic!("expected scoped current evidence");
    };
    assert_eq!(contract, SCOPED_CURRENT_EVIDENCE_CONTRACT);
    assert_eq!(current_id, "replacement");
    assert_eq!(evidence_identity.record_id, "replacement");

    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn conflicting_reimport_and_direct_update_leave_every_effect_unchanged() {
    let dir = temp_dir("immutable-conflict");
    let path = dir.join("store.db");
    let store = Store::open_with_store_instance_id(&path, "provider:conflict").unwrap();
    let original = row("sealed", "repo-a");
    store
        .import_external_sealed(std::slice::from_ref(&original))
        .unwrap();
    store.link_git("sealed", "abc123", "Original link").unwrap();
    let observer = Connection::open(&path).unwrap();
    let snapshot = || {
        let record: (String, String, String) = observer
            .query_row(
                "SELECT title,content,record_digest_v1 FROM decisions WHERE id='sealed'",
                [],
                |record| Ok((record.get(0)?, record.get(1)?, record.get(2)?)),
            )
            .unwrap();
        let fts: i64 = observer
            .query_row("SELECT count(*) FROM decisions_fts", [], |record| {
                record.get(0)
            })
            .unwrap();
        let links: i64 = observer
            .query_row(
                "SELECT count(*) FROM decision_git_refs WHERE decision_id='sealed'",
                [],
                |record| record.get(0),
            )
            .unwrap();
        let data_version: i64 = observer
            .pragma_query_value(None, "data_version", |record| record.get(0))
            .unwrap();
        (record, fts, links, data_version)
    };
    let before = snapshot();
    let mut conflict = original;
    conflict.content = "changed immutable content".to_owned();
    conflict.git_refs.push(GitRef {
        commit_hash: "must-not-land".to_owned(),
        commit_subject: "Must not land".to_owned(),
    });
    let error = store.import_external_sealed(&[conflict]).unwrap_err();
    assert!(error.downcast_ref::<RecordIdentityConflict>().is_some());
    assert_eq!(snapshot(), before);

    for assignment in [
        "id='sealed-2'",
        "scope='repo-b'",
        "kind='fact'",
        "title='direct mutation'",
        "content='direct mutation'",
        "importance=0.2",
        "source='direct mutation'",
        "author='direct mutation'",
        "commit_sha='direct-mutation'",
        "date='2027-01-01T00:00:00Z'",
        "tags='[\"changed\"]'",
        "fact_key='changed'",
        "valid_from='2027-01-01T00:00:00Z'",
        "declared_valid_until='2028-01-01T00:00:00Z'",
        "record_digest_v1='0000000000000000000000000000000000000000000000000000000000000000'",
    ] {
        let direct = observer.execute(
            &format!("UPDATE decisions SET {assignment} WHERE id='sealed'"),
            [],
        );
        assert!(direct
            .unwrap_err()
            .to_string()
            .contains("identity_conflict"));
        assert_eq!(snapshot(), before, "mutation escaped guard: {assignment}");
    }
    for destructive in [
        "DELETE FROM decisions WHERE id='sealed'",
        "INSERT OR REPLACE INTO decisions SELECT * FROM decisions WHERE id='sealed'",
    ] {
        let result = observer.execute(destructive, []);
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("identity_conflict"));
        assert_eq!(snapshot(), before, "destructive write escaped guard");
    }
    drop(observer);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn public_store_import_replays_exactly_and_rejects_a_batch_before_any_effect() {
    let dir = temp_dir("public-import");
    let path = dir.join("store.db");
    let store = Store::open_with_store_instance_id(&path, "provider:public-import").unwrap();
    let mut original = row("public-sealed", "repo-a");
    original.git_refs.push(GitRef {
        commit_hash: "original-commit".to_owned(),
        commit_subject: "Original relation".to_owned(),
    });
    assert_eq!(
        store
            .import_external(std::slice::from_ref(&original))
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .import_external(std::slice::from_ref(&original))
            .unwrap(),
        0
    );

    let mut new_row = row("must-not-insert", "repo-a");
    new_row.git_refs.push(GitRef {
        commit_hash: "must-not-insert-ref".to_owned(),
        commit_subject: "Must not insert".to_owned(),
    });
    let mut conflict = original.clone();
    conflict.content = "changed immutable body".to_owned();
    conflict.git_refs.push(GitRef {
        commit_hash: "must-not-append".to_owned(),
        commit_subject: "Must not append".to_owned(),
    });
    let error = store.import_external(&[new_row, conflict]).unwrap_err();
    assert!(error.downcast_ref::<RecordIdentityConflict>().is_some());

    let observer = Connection::open(&path).unwrap();
    let records: i64 = observer
        .query_row("SELECT count(*) FROM decisions", [], |record| record.get(0))
        .unwrap();
    let original_body: String = observer
        .query_row(
            "SELECT content FROM decisions WHERE id='public-sealed'",
            [],
            |record| record.get(0),
        )
        .unwrap();
    let refs: Vec<String> = observer
        .prepare(
            "SELECT commit_hash FROM decision_git_refs
             WHERE decision_id='public-sealed' ORDER BY commit_hash",
        )
        .unwrap()
        .query_map([], |record| record.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(records, 1);
    assert_eq!(original_body, original.content);
    assert_eq!(refs, ["original-commit"]);
    drop(observer);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn real_cli_import_reports_created_replay_and_conflict_truthfully() {
    let dir = temp_dir("cli-import");
    let db_path = dir.join("store.db");
    let input_path = dir.join("rows.json");
    let provider = "provider:cli-import";
    let mut original = row("cli-sealed", "repo-a");
    original.git_refs.push(GitRef {
        commit_hash: "cli-original-commit".to_owned(),
        commit_subject: "CLI original relation".to_owned(),
    });
    std::fs::write(&input_path, serde_json::to_vec(&[&original]).unwrap()).unwrap();

    let run_import = || {
        Command::new(env!("CARGO_BIN_EXE_why"))
            .arg("import")
            .arg("--file")
            .arg(&input_path)
            .env("OPEN_WHY_DB", &db_path)
            .env("OPEN_WHY_STORE_INSTANCE_ID", provider)
            .output()
            .unwrap()
    };
    let created = run_import();
    assert!(created.status.success());
    assert_eq!(
        String::from_utf8(created.stdout).unwrap(),
        "imported 1 decisions\n"
    );
    let replayed = run_import();
    assert!(replayed.status.success());
    assert_eq!(
        String::from_utf8(replayed.stdout).unwrap(),
        "imported 0 decisions\n"
    );

    let mut new_row = row("cli-must-not-insert", "repo-a");
    new_row.git_refs.push(GitRef {
        commit_hash: "cli-must-not-insert-ref".to_owned(),
        commit_subject: "Must not insert".to_owned(),
    });
    let mut conflict = original.clone();
    conflict.content = "changed CLI immutable body".to_owned();
    conflict.git_refs.push(GitRef {
        commit_hash: "cli-must-not-append".to_owned(),
        commit_subject: "Must not append".to_owned(),
    });
    std::fs::write(
        &input_path,
        serde_json::to_vec(&[new_row, conflict]).unwrap(),
    )
    .unwrap();
    let rejected = run_import();
    assert!(!rejected.status.success());
    assert!(String::from_utf8(rejected.stderr)
        .unwrap()
        .contains("identity_conflict"));

    let observer = Connection::open(&db_path).unwrap();
    let records: i64 = observer
        .query_row("SELECT count(*) FROM decisions", [], |record| record.get(0))
        .unwrap();
    let original_body: String = observer
        .query_row(
            "SELECT content FROM decisions WHERE id='cli-sealed'",
            [],
            |record| record.get(0),
        )
        .unwrap();
    let refs: Vec<String> = observer
        .prepare(
            "SELECT commit_hash FROM decision_git_refs
             WHERE decision_id='cli-sealed' ORDER BY commit_hash",
        )
        .unwrap()
        .query_map([], |record| record.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(records, 1);
    assert_eq!(original_body, original.content);
    assert_eq!(refs, ["cli-original-commit"]);
    drop(observer);
    std::fs::remove_dir_all(dir).unwrap();
}
