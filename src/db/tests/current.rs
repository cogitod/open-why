use super::super::*;
use super::support::*;

#[test]
fn current_evidence_resolves_a_stale_link_and_returns_current_git_proof() {
    let store = temp_store();
    store
        .capture_external(
            &decision("database choice", "sqlite", 0.5, None),
            "global",
            "aaa",
            Some("2026-01-01T00:00:00Z"),
            None,
            None,
        )
        .unwrap();
    store
        .capture_external(
            &decision("database choice", "postgres now", 0.5, None),
            "global",
            "bbb",
            Some("2026-02-01T00:00:00Z"),
            None,
            Some("aaa"),
        )
        .unwrap();
    store.link_git("aaa", "old-commit", "Use SQLite").unwrap();
    store
        .link_git("bbb", "new-commit", "Move to Postgres")
        .unwrap();

    let as_of = iso_to_epoch("2026-03-01T00:00:00Z").unwrap();
    let evidence = store.get_current_evidence_at("aaa", as_of, 64).unwrap();
    let CurrentRecordResolution::Ok {
        requested_id,
        current_id,
        record,
        supersession_chain,
        git_refs,
        as_of: effective_as_of,
        ..
    } = evidence
    else {
        panic!("expected successful current resolution");
    };
    assert_eq!(requested_id, "aaa");
    assert_eq!(current_id, "bbb");
    assert_eq!(record.id, "bbb");
    assert_eq!(record.content, "postgres now");
    assert_eq!(supersession_chain, ["aaa", "bbb"]);
    assert_eq!(git_refs.len(), 1);
    assert_eq!(git_refs[0].commit_hash, "new-commit");
    assert_eq!(effective_as_of, "2026-03-01T00:00:00Z");
}

#[test]
fn current_evidence_fails_closed_for_a_retired_record_without_a_successor() {
    let store = temp_store();
    store
        .capture_external(
            &decision("retired", "no longer current", 0.5, None),
            "global",
            "retired-id",
            Some("2026-01-01T00:00:00Z"),
            None,
            None,
        )
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE decisions SET valid_until='2026-02-01T00:00:00Z' WHERE id='retired-id'",
            [],
        )
        .unwrap();

    let as_of = iso_to_epoch("2026-03-01T00:00:00Z").unwrap();
    assert!(matches!(
        store
            .get_current_evidence_at("retired-id", as_of, 64)
            .unwrap(),
        CurrentRecordResolution::Error {
            code: CurrentRecordErrorCode::ExpiredWithoutSuccessor,
            ..
        }
    ));
    assert!(matches!(
        store.get_current_evidence_at("missing", as_of, 64).unwrap(),
        CurrentRecordResolution::Error {
            code: CurrentRecordErrorCode::NotFound,
            ..
        }
    ));
}

#[test]
fn current_evidence_distinguishes_broken_cycle_and_traversal_limit() {
    let store = temp_store();
    let row = |id: &str, successor: Option<&str>| ExternalDecision {
        id: id.to_owned(),
        kind: "decision".to_owned(),
        title: format!("record {id}"),
        content: format!("complete body for {id}"),
        importance: 0.5,
        source: "synthetic".to_owned(),
        author: "tester".to_owned(),
        date: "2026-01-01".to_owned(),
        updated_at: None,
        accessed_count: None,
        times_injected: None,
        effectiveness: None,
        tags: None,
        scope: "scope-a".to_owned(),
        valid_from: Some("2026-01-01T00:00:00Z".to_owned()),
        valid_until: successor.map(|_| "2026-02-01T00:00:00Z".to_owned()),
        superseded_by: successor.map(str::to_owned),
        fact_key: None,
        git_refs: Vec::new(),
    };
    store
        .import_external(&[row("broken", Some("missing"))])
        .unwrap();
    store
        .import_external(&[
            row("cycle-a", Some("cycle-b")),
            row("cycle-b", Some("cycle-a")),
        ])
        .unwrap();
    store
        .import_external(&[row("long-a", Some("long-b")), row("long-b", None)])
        .unwrap();
    let as_of = iso_to_epoch("2026-03-01T00:00:00Z").unwrap();

    assert!(matches!(
        store.get_current_evidence_at("broken", as_of, 64).unwrap(),
        CurrentRecordResolution::Error {
            code: CurrentRecordErrorCode::BrokenChain,
            ..
        }
    ));
    assert!(matches!(
        store.get_current_evidence_at("cycle-a", as_of, 64).unwrap(),
        CurrentRecordResolution::Error {
            code: CurrentRecordErrorCode::Cycle,
            ..
        }
    ));
    assert!(matches!(
        store.get_current_evidence_at("long-a", as_of, 1).unwrap(),
        CurrentRecordResolution::Error {
            code: CurrentRecordErrorCode::TraversalLimit,
            ..
        }
    ));
}

#[test]
fn current_evidence_obeys_validity_instants_and_rejects_bad_stored_time() {
    let store = temp_store();
    let insert = |id: &str, from: &str, until: Option<&str>| {
        let mut row = history_row(id, None, "scope-a", "full body");
        row.valid_from = Some(from.to_owned());
        row.valid_until = until.map(str::to_owned);
        store.import_external(&[row]).unwrap();
    };
    insert("future", "2026-04-01T00:00:00Z", None);
    insert(
        "bounded",
        "2026-01-01T00:00:00Z",
        Some("2026-04-01T00:00:00Z"),
    );
    insert("invalid", "not-a-time", None);
    insert(
        "inverted",
        "2026-05-01T00:00:00Z",
        Some("2026-04-01T00:00:00Z"),
    );
    let before = iso_to_epoch("2026-03-01T00:00:00Z").unwrap();
    let boundary = iso_to_epoch("2026-04-01T00:00:00Z").unwrap();

    assert!(matches!(
        store.get_current_evidence_at("future", before, 64).unwrap(),
        CurrentRecordResolution::Error {
            code: CurrentRecordErrorCode::NotYetValid,
            ..
        }
    ));
    assert!(matches!(
        store
            .get_current_evidence_at("bounded", before, 64)
            .unwrap(),
        CurrentRecordResolution::Ok { .. }
    ));
    assert!(matches!(
        store
            .get_current_evidence_at("bounded", boundary, 64)
            .unwrap(),
        CurrentRecordResolution::Error {
            code: CurrentRecordErrorCode::ExpiredWithoutSuccessor,
            ..
        }
    ));
    assert!(matches!(
        store
            .get_current_evidence_at("invalid", before, 64)
            .unwrap(),
        CurrentRecordResolution::Error {
            code: CurrentRecordErrorCode::InvalidTemporalData,
            ..
        }
    ));
    assert!(matches!(
        store
            .get_current_evidence_at("inverted", before, 64)
            .unwrap(),
        CurrentRecordResolution::Error {
            code: CurrentRecordErrorCode::InvalidTemporalData,
            ..
        }
    ));
}

#[test]
fn scoped_current_hides_foreign_and_absent_nodes_without_mutating_state() {
    let store = temp_store();
    store
        .import_external(&[
            history_row("root", Some("middle"), "scope-a", "root body"),
            history_row("middle", Some("foreign"), "scope-a", "middle body"),
            history_row(
                "foreign",
                None,
                "scope-b",
                "foreign body sentinel 2099-01-01T00:00:00Z",
            ),
        ])
        .unwrap();
    store
        .link_git("foreign", "foreign-commit", "Foreign evidence sentinel")
        .unwrap();
    store
        .conn
        .execute_batch("DROP TRIGGER decisions_identity_update_guard;")
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE decisions
                 SET content=CAST(zeroblob(8388608) AS TEXT), importance=X'FF',
                     superseded_by=X'FF', valid_from=X'FF', valid_until=42
                 WHERE id='foreign'",
            [],
        )
        .unwrap();
    let as_of = iso_to_epoch("2026-03-01T00:00:00Z").unwrap();
    let changes_before_foreign = store.conn.total_changes();
    let foreign = store
        .get_current_evidence_in_scope_at("root", "scope-a", as_of, 64)
        .unwrap();
    assert_eq!(store.conn.total_changes(), changes_before_foreign);

    store
        .conn
        .execute(
            "UPDATE decisions SET superseded_by='absent' WHERE id='middle'",
            [],
        )
        .unwrap();
    let changes_before_absent = store.conn.total_changes();
    let absent = store
        .get_current_evidence_in_scope_at("root", "scope-a", as_of + 60, 64)
        .unwrap();
    assert_eq!(store.conn.total_changes(), changes_before_absent);

    let normalize = |resolution: CurrentRecordResolution| {
        let mut value = serde_json::to_value(resolution).unwrap();
        value["as_of"] = serde_json::Value::String("normalized".to_owned());
        value
    };
    let foreign = normalize(foreign);
    let absent = normalize(absent);
    assert_eq!(foreign, absent);
    assert_eq!(foreign["contract"], CURRENT_RATIONALE_CONTRACT);
    assert_eq!(foreign["code"], "broken_chain");
    assert_eq!(
        foreign["message"],
        "supersession chain is unavailable in the requested scope"
    );
    let wire = serde_json::to_string(&foreign).unwrap();
    for secret in [
        "foreign",
        "2099-01-01T00:00:00Z",
        "foreign body sentinel",
        "foreign-commit",
        "Foreign evidence sentinel",
    ] {
        assert!(!wire.contains(secret), "scoped error leaked {secret}");
    }
}

#[test]
fn scoped_current_makes_wrong_scope_root_indistinguishable_from_absence() {
    let wrong_scope_store = temp_store();
    wrong_scope_store
        .import_external(&[history_row("same-id", None, "scope-b", "foreign root body")])
        .unwrap();
    wrong_scope_store
        .conn
        .execute_batch("DROP TRIGGER decisions_identity_update_guard;")
        .unwrap();
    wrong_scope_store
        .conn
        .execute(
            "UPDATE decisions
                 SET content=CAST(zeroblob(8388608) AS TEXT), importance=X'FF',
                     superseded_by=X'FF', valid_from=X'FF', valid_until=42
                 WHERE id='same-id'",
            [],
        )
        .unwrap();
    let absent_store = temp_store();
    let as_of = iso_to_epoch("2026-03-01T00:00:00Z").unwrap();

    let changes_before_wrong_scope = wrong_scope_store.conn.total_changes();
    let wrong_scope = wrong_scope_store
        .get_current_evidence_in_scope_at("same-id", "scope-a", as_of, 64)
        .unwrap();
    assert_eq!(
        wrong_scope_store.conn.total_changes(),
        changes_before_wrong_scope
    );
    let changes_before_absent = absent_store.conn.total_changes();
    let absent = absent_store
        .get_current_evidence_in_scope_at("same-id", "scope-a", as_of, 64)
        .unwrap();
    assert_eq!(absent_store.conn.total_changes(), changes_before_absent);
    let wrong_scope = serde_json::to_value(wrong_scope).unwrap();
    assert_eq!(wrong_scope["contract"], CURRENT_RATIONALE_CONTRACT);
    assert_eq!(wrong_scope, serde_json::to_value(absent).unwrap());
}

#[test]
fn current_evidence_uses_one_snapshot_then_observes_the_next_complete_snapshot() {
    let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!(
            "open-why-current-snapshot-{}-{n}",
            std::process::id()
        ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("current.db");
    let store = Store::open_with_embedder_and_store_instance_id(
        &path,
        None,
        &format!("provider:current:{n}"),
    )
    .unwrap();
    store
        .conn
        .execute_batch("PRAGMA journal_mode=WAL;")
        .unwrap();
    store
        .import_external(&[
            history_row("snapshot-root", Some("snapshot-old"), "scope-a", "root"),
            history_row("snapshot-old", None, "scope-a", "old current body"),
            history_row("snapshot-new", None, "scope-a", "pending new body"),
        ])
        .unwrap();
    store
        .link_git("snapshot-old", "old-commit", "Old evidence")
        .unwrap();
    let writer = Connection::open(&path).unwrap();
    writer.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
    let as_of = iso_to_epoch("2026-03-01T00:00:00Z").unwrap();

    let first = store
        .get_current_evidence_at_with_scope_and_hook(
            "snapshot-root",
            Some("scope-a"),
            as_of,
            64,
            true,
            || {
                writer.execute_batch(
                    "BEGIN IMMEDIATE;
                         UPDATE decisions SET superseded_by='snapshot-new'
                         WHERE id='snapshot-root';
                         INSERT INTO decision_git_refs
                         (decision_id,commit_hash,commit_subject)
                         VALUES ('snapshot-new','new-commit','New evidence');
                         COMMIT;",
                )?;
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(
        first
            .identity
            .as_ref()
            .map(|identity| identity.record_id.as_str()),
        Some("snapshot-old")
    );
    let CurrentRecordResolution::Ok {
        current_id,
        record,
        git_refs,
        supersession_chain,
        ..
    } = first.resolution
    else {
        panic!("expected old coherent snapshot");
    };
    assert_eq!(current_id, "snapshot-old");
    assert_eq!(record.content, "old current body");
    assert_eq!(supersession_chain, ["snapshot-root", "snapshot-old"]);
    assert!(git_refs.iter().any(|item| item.commit_hash == "old-commit"));
    assert!(!git_refs.iter().any(|item| item.commit_hash == "new-commit"));

    let second = store
        .get_current_evidence_at_with_scope_and_hook(
            "snapshot-root",
            Some("scope-a"),
            as_of,
            64,
            true,
            || Ok(()),
        )
        .unwrap();
    assert_eq!(
        second
            .identity
            .as_ref()
            .map(|identity| identity.record_id.as_str()),
        Some("snapshot-new")
    );
    let CurrentRecordResolution::Ok {
        current_id,
        record,
        git_refs,
        supersession_chain,
        ..
    } = second.resolution
    else {
        panic!("expected new coherent snapshot");
    };
    assert_eq!(current_id, "snapshot-new");
    assert_eq!(record.content, "pending new body");
    assert_eq!(supersession_chain, ["snapshot-root", "snapshot-new"]);
    assert!(git_refs.iter().any(|item| item.commit_hash == "new-commit"));
    assert!(!git_refs.iter().any(|item| item.commit_hash == "old-commit"));

    drop(writer);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}
