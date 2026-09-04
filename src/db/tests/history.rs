use super::super::*;
use super::support::*;

#[test]
fn historical_mode_surfaces_supersession_chain() {
    let store = temp_store();
    store
        .capture_external(
            &decision("database choice", "sqlite", 0.5, None),
            "global",
            "aaa",
            None,
            None,
            None,
        )
        .unwrap();
    store
        .capture_external(
            &decision("database choice v2", "postgres now", 0.5, None),
            "global",
            "bbb",
            None,
            None,
            Some("aaa"),
        )
        .unwrap();
    // Active search returns only the current (non-superseded) record.
    let hits = store
        .search("sqlite postgres", &["global"], &[], 10)
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].subject, "database choice v2");
    // Historical search returns both.
    let hits = store
        .search_records_with("sqlite postgres", &["global"], &[], 10, true)
        .unwrap();
    assert_eq!(hits.len(), 2);
    // The chain walks aaa -> bbb.
    let chain = store.supersession_chain("aaa", 20).unwrap();
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0].id, "aaa");
    assert_eq!(chain[1].id, "bbb");
}

#[test]
fn rationale_history_pages_one_two_and_long_chains_without_gaps() {
    let store = temp_store();
    store
        .import_external(&[history_row("one", None, "scope-a", "one body")])
        .unwrap();
    store
        .import_external(&[
            history_row("two-a", Some("two-b"), "scope-a", "old body"),
            history_row("two-b", None, "scope-a", "new body"),
        ])
        .unwrap();
    store
        .import_external(&[
            history_row("long-a", Some("long-b"), "scope-a", "body α"),
            history_row("long-b", Some("long-c"), "scope-a", "body β"),
            history_row("long-c", Some("long-d"), "scope-a", "body γ"),
            history_row("long-d", Some("long-e"), "scope-a", "body δ"),
            history_row("long-e", None, "scope-a", "body 🚀"),
        ])
        .unwrap();
    let as_of = iso_to_epoch("2026-03-01T00:00:00Z").unwrap();

    let RationaleHistoryResolution::Ok {
        records,
        next_cursor,
        complete,
        page_start_id,
        ..
    } = store
        .get_rationale_history_at("one", "scope-a", None, 3, as_of, 64)
        .unwrap()
    else {
        panic!("expected one-record history");
    };
    assert_eq!(page_start_id, "one");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].record.id, "one");
    assert_eq!(records[0].git_refs[0].commit_hash, "commit-one");
    assert!(complete);
    assert_eq!(next_cursor, None);

    let RationaleHistoryResolution::Ok { records, .. } = store
        .get_rationale_history_at("two-a", "scope-a", None, 3, as_of, 64)
        .unwrap()
    else {
        panic!("expected two-record history");
    };
    assert_eq!(
        records
            .iter()
            .map(|item| item.record.id.as_str())
            .collect::<Vec<_>>(),
        ["two-a", "two-b"]
    );

    let first = store
        .get_rationale_history_at("long-a", "scope-a", None, 3, as_of, 64)
        .unwrap();
    let RationaleHistoryResolution::Ok {
        records,
        next_cursor,
        complete,
        ..
    } = &first
    else {
        panic!("expected first history page");
    };
    assert_eq!(
        records
            .iter()
            .map(|item| item.record.id.as_str())
            .collect::<Vec<_>>(),
        ["long-a", "long-b", "long-c"]
    );
    assert_eq!(records[2].record.content, "body γ");
    assert_eq!(next_cursor.as_deref(), Some("long-d"));
    assert!(!complete);

    let second = store
        .get_rationale_history_at("long-a", "scope-a", next_cursor.as_deref(), 3, as_of, 64)
        .unwrap();
    let RationaleHistoryResolution::Ok {
        records,
        next_cursor,
        complete,
        page_start_id,
        ..
    } = &second
    else {
        panic!("expected final history page");
    };
    assert_eq!(page_start_id, "long-d");
    assert_eq!(
        records
            .iter()
            .map(|item| item.record.id.as_str())
            .collect::<Vec<_>>(),
        ["long-d", "long-e"]
    );
    assert_eq!(records[1].record.content, "body 🚀");
    assert_eq!(next_cursor, &None);
    assert!(*complete);

    let repeat = store
        .get_rationale_history_at("long-a", "scope-a", None, 3, as_of, 64)
        .unwrap();
    assert_eq!(
        serde_json::to_value(first).unwrap(),
        serde_json::to_value(repeat).unwrap()
    );
}

#[test]
fn rationale_history_rejects_off_chain_scope_and_structural_failures() {
    let store = temp_store();
    store
        .import_external(&[
            history_row("root", Some("next"), "scope-a", "root"),
            history_row("next", None, "scope-a", "next"),
            history_row("unrelated", None, "scope-a", "unrelated"),
            history_row("foreign", None, "scope-b", "foreign"),
            history_row("broken", Some("missing-successor"), "scope-a", "broken"),
            history_row("cycle-a", Some("cycle-b"), "scope-a", "cycle a"),
            history_row("cycle-b", Some("cycle-a"), "scope-a", "cycle b"),
            history_row("cap-a", Some("cap-b"), "scope-a", "cap a"),
            history_row("cap-b", None, "scope-a", "cap b"),
            history_row(
                "cross-root",
                Some("cross-successor"),
                "scope-a",
                "cross root",
            ),
            history_row("cross-successor", None, "scope-b", "foreign body"),
        ])
        .unwrap();
    let mut bad_time = history_row("bad-time", None, "scope-a", "body");
    bad_time.valid_from = Some("not-a-time".to_owned());
    store.import_external(&[bad_time]).unwrap();
    let as_of = iso_to_epoch("2026-03-01T00:00:00Z").unwrap();
    let code = |resolution| match resolution {
        RationaleHistoryResolution::Error { code, .. } => code,
        RationaleHistoryResolution::Ok { .. } => panic!("expected typed history error"),
    };

    let RationaleHistoryResolution::Ok { records, .. } = store
        .get_rationale_history_at("root", "scope-a", Some("next"), 3, as_of, 64)
        .unwrap()
    else {
        unreachable!();
    };
    assert_eq!(records[0].record.id, "next");
    for cursor in ["unrelated", "foreign", "missing-cursor"] {
        assert_eq!(
            code(
                store
                    .get_rationale_history_at("root", "scope-a", Some(cursor), 3, as_of, 64,)
                    .unwrap()
            ),
            RationaleHistoryErrorCode::InvalidCursor
        );
    }
    let wrong_scope = store
        .get_rationale_history_at("foreign", "scope-a", None, 3, as_of, 64)
        .unwrap();
    let empty_store = temp_store();
    let same_id_missing = empty_store
        .get_rationale_history_at("foreign", "scope-a", None, 3, as_of, 64)
        .unwrap();
    assert_eq!(
        serde_json::to_value(wrong_scope).unwrap(),
        serde_json::to_value(same_id_missing).unwrap(),
        "wrong-scope and missing records must be indistinguishable"
    );
    assert_eq!(
        code(
            store
                .get_rationale_history_at("absent", "scope-a", None, 3, as_of, 64)
                .unwrap()
        ),
        RationaleHistoryErrorCode::NotFound
    );
    assert_eq!(
        code(
            store
                .get_rationale_history_at("broken", "scope-a", None, 3, as_of, 64)
                .unwrap()
        ),
        RationaleHistoryErrorCode::BrokenChain
    );
    let foreign_successor = store
        .get_rationale_history_at("cross-root", "scope-a", None, 3, as_of, 64)
        .unwrap();
    let unavailable_successor = store
        .get_rationale_history_at("broken", "scope-a", None, 3, as_of, 64)
        .unwrap();
    let error_shape = |resolution| match resolution {
        RationaleHistoryResolution::Error { code, message, .. } => (code, message),
        RationaleHistoryResolution::Ok { .. } => panic!("expected broken chain"),
    };
    let (foreign_code, foreign_message) = error_shape(foreign_successor);
    let (missing_code, missing_message) = error_shape(unavailable_successor);
    assert_eq!(foreign_code, RationaleHistoryErrorCode::BrokenChain);
    assert_eq!(foreign_code, missing_code);
    assert_eq!(foreign_message, missing_message);
    assert!(!foreign_message.contains("cross-successor"));
    assert_eq!(
        code(
            store
                .get_rationale_history_at("cycle-a", "scope-a", None, 3, as_of, 64)
                .unwrap()
        ),
        RationaleHistoryErrorCode::Cycle
    );
    assert_eq!(
        code(
            store
                .get_rationale_history_at("cap-a", "scope-a", None, 3, as_of, 1)
                .unwrap()
        ),
        RationaleHistoryErrorCode::TraversalLimit
    );
    assert_eq!(
        code(
            store
                .get_rationale_history_at("bad-time", "scope-a", None, 3, as_of, 64)
                .unwrap()
        ),
        RationaleHistoryErrorCode::InvalidTemporalData
    );
}

#[test]
fn rationale_history_uses_one_snapshot_and_bounds_selected_hydration() {
    let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!(
            "open-why-history-snapshot-{}-{n}",
            std::process::id()
        ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("history.db");
    let store = Store::open_with_embedder_and_store_instance_id(
        &path,
        None,
        &format!("provider:history:{n}"),
    )
    .unwrap();
    store
        .conn
        .execute_batch("PRAGMA journal_mode=WAL;")
        .unwrap();
    store
        .import_external(&[
            history_row("snapshot-a", Some("snapshot-b"), "scope-a", "old root"),
            history_row("snapshot-b", None, "scope-a", "old successor"),
            history_row("snapshot-alt", None, "scope-a", "alternate successor"),
        ])
        .unwrap();
    let writer = Connection::open(&path).unwrap();
    writer.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
    let as_of = iso_to_epoch("2026-03-01T00:00:00Z").unwrap();
    let resolution = store
        .get_rationale_history_at_with_hook(
            HistoryPageRequest {
                id: "snapshot-a",
                scope: "scope-a",
                page_cursor: None,
                limit: 3,
                as_of,
                chain_cap: 64,
            },
            || {
                writer.execute(
                    "UPDATE decisions SET superseded_by='snapshot-alt'
                         WHERE id='snapshot-a'",
                    [],
                )?;
                writer.execute(
                    "INSERT INTO decision_git_refs
                         (decision_id,commit_hash,commit_subject)
                         VALUES ('snapshot-b','concurrent-commit','Concurrent evidence')",
                    [],
                )?;
                Ok(())
            },
        )
        .unwrap();
    let RationaleHistoryResolution::Ok { records, .. } = resolution else {
        panic!("expected snapshot history");
    };
    assert_eq!(records.len(), 2);
    assert_eq!(
        records[0].record.superseded_by.as_deref(),
        Some("snapshot-b")
    );
    assert_eq!(records[1].record.content, "old successor");
    assert_eq!(records[1].git_refs.len(), 1);
    assert!(!records[1]
        .git_refs
        .iter()
        .any(|git_ref| git_ref.commit_hash == "concurrent-commit"));
    assert_eq!(
        store
            .get_record_any("snapshot-a", true)
            .unwrap()
            .unwrap()
            .superseded_by
            .as_deref(),
        Some("snapshot-alt")
    );
    assert_eq!(
        store
            .get_record_any("snapshot-b", true)
            .unwrap()
            .unwrap()
            .content,
        "old successor"
    );
    assert_eq!(store.linked_commits("snapshot-b").unwrap().len(), 2);

    let huge_tail = "\0".repeat(MAX_HISTORY_PAGE_SOURCE_BYTES + 1);
    store
        .import_external(&[
            history_row("bounded-a", Some("bounded-b"), "scope-a", "a"),
            history_row("bounded-b", Some("bounded-c"), "scope-a", "b"),
            history_row("bounded-c", Some("bounded-d"), "scope-a", "c"),
            history_row("bounded-d", None, "scope-a", &huge_tail),
        ])
        .unwrap();
    assert!(matches!(
        store
            .get_rationale_history_at("bounded-a", "scope-a", None, 3, as_of, 64)
            .unwrap(),
        RationaleHistoryResolution::Ok {
            complete: false,
            ..
        }
    ));
    assert!(matches!(
        store
            .get_rationale_history_at("bounded-a", "scope-a", Some("bounded-d"), 3, as_of, 64,)
            .unwrap(),
        RationaleHistoryResolution::Error {
            code: RationaleHistoryErrorCode::ResponseTooLarge,
            ..
        }
    ));

    store
        .import_external(&[history_row(
            "reference-budget",
            None,
            "scope-a",
            "bounded body",
        )])
        .unwrap();
    store
        .conn
        .execute(
            "WITH RECURSIVE numbers(n) AS (
                     SELECT 1
                     UNION ALL
                     SELECT n + 1 FROM numbers WHERE n < ?2
                 )
                 INSERT INTO decision_git_refs
                     (decision_id,commit_hash,commit_subject)
                 SELECT ?1, printf('budget-commit-%03d',n), 'bounded evidence'
                 FROM numbers",
            params!["reference-budget", MAX_HISTORY_PAGE_GIT_REFS + 1],
        )
        .unwrap();
    assert!(matches!(
        store
            .get_rationale_history_at("reference-budget", "scope-a", None, 3, as_of, 64,)
            .unwrap(),
        RationaleHistoryResolution::Error {
            code: RationaleHistoryErrorCode::ResponseTooLarge,
            ..
        }
    ));
    drop(writer);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rationale_history_v1_validates_records_but_not_cross_record_continuity() {
    let store = temp_store();
    let mut overlap_a = history_row("overlap-a", Some("overlap-b"), "scope-a", "a");
    overlap_a.valid_until = Some("2026-03-01T00:00:00Z".to_owned());
    let mut overlap_b = history_row("overlap-b", None, "scope-a", "b");
    overlap_b.valid_from = Some("2026-02-01T00:00:00Z".to_owned());
    let mut gap_a = history_row("gap-a", Some("gap-b"), "scope-a", "a");
    gap_a.valid_until = Some("2026-02-01T00:00:00Z".to_owned());
    let mut gap_b = history_row("gap-b", None, "scope-a", "b");
    gap_b.valid_from = Some("2026-03-01T00:00:00Z".to_owned());
    store
        .import_external(&[overlap_a, overlap_b, gap_a, gap_b])
        .unwrap();
    let as_of = iso_to_epoch("2026-04-01T00:00:00Z").unwrap();

    for root in ["overlap-a", "gap-a"] {
        assert!(matches!(
            store
                .get_rationale_history_at(root, "scope-a", None, 3, as_of, 64)
                .unwrap(),
            RationaleHistoryResolution::Ok { .. }
        ));
    }
}
