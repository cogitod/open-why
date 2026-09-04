use super::super::*;
use super::support::*;

#[test]
fn scoped_commit_link_is_authoritative_idempotent_and_conflict_safe() {
    let store = temp_store();
    store
        .import_external(&[history_row("sealed-link", None, "scope-a", "body")])
        .unwrap();
    let identity = evidence_identity(&store, "sealed-link", "scope-a");
    let before_identity = identity.clone();

    let created = store.link_git_in_scope(&identity, "abc123", "Create link");
    assert_eq!(
        created,
        ScopedCommitLinkResolution::Ok {
            contract: SCOPED_COMMIT_LINK_WRITE_CONTRACT,
            outcome: ScopedCommitLinkOutcome::Created,
            evidence_identity: identity.clone(),
            git_ref: GitRef {
                commit_hash: "abc123".to_owned(),
                commit_subject: "Create link".to_owned(),
            },
        }
    );
    assert_eq!(
        evidence_identity(&store, "sealed-link", "scope-a"),
        before_identity
    );
    let CommitLinksResolution::Ok { items, .. } = store
        .get_commit_links("scope-a", "abc123", None, 20)
        .unwrap()
    else {
        panic!("created link must be readable through commit-links v1");
    };
    assert_eq!(items[0].record_id, "sealed-link");
    assert_eq!(items[0].commit_subject, "Create link");

    let observer = Connection::open(store.conn.path().unwrap()).unwrap();
    let data_version = || {
        observer
            .pragma_query_value(None, "data_version", |row| row.get::<_, i64>(0))
            .unwrap()
    };
    let version_before = data_version();
    let changes_before = store.conn.total_changes();
    let replay = store.link_git_in_scope(&identity, "abc123", "Create link");
    assert!(matches!(
        replay,
        ScopedCommitLinkResolution::Ok {
            outcome: ScopedCommitLinkOutcome::ExactReplay,
            ..
        }
    ));
    assert_eq!(store.conn.total_changes(), changes_before);
    assert_eq!(data_version(), version_before);

    let conflict = store.link_git_in_scope(&identity, "abc123", "Changed subject");
    assert!(matches!(
        conflict,
        ScopedCommitLinkResolution::Error {
            code: ScopedCommitLinkErrorCode::LinkConflict,
            retryable: false,
            ..
        }
    ));
    assert_eq!(store.conn.total_changes(), changes_before);
    assert_eq!(data_version(), version_before);
    assert_eq!(
        store.linked_commits("sealed-link").unwrap()[0].1,
        "Create link"
    );
}

#[test]
fn scoped_commit_link_collapses_identity_failures_before_effects() {
    let store = temp_store();
    store
        .import_external(&[
            history_row("sealed-link", None, "scope-a", "body"),
            history_row("foreign-link", None, "scope-b", "foreign secret"),
        ])
        .unwrap();
    let valid = evidence_identity(&store, "sealed-link", "scope-a");
    let foreign = evidence_identity(&store, "foreign-link", "scope-b");
    let mut cases = Vec::new();

    let mut missing = valid.clone();
    missing.record_id = "missing-link".to_owned();
    cases.push(missing);
    let mut wrong_scope = foreign;
    wrong_scope.scope = "scope-a".to_owned();
    cases.push(wrong_scope);
    let mut wrong_store = valid.clone();
    wrong_store.store_instance_id = "provider:another-store".to_owned();
    cases.push(wrong_store);
    let mut wrong_digest = valid.clone();
    wrong_digest.record_digest = "0".repeat(64);
    cases.push(wrong_digest);
    let mut wrong_contract = valid.clone();
    wrong_contract.contract = "open-why.evidence-identity/v0";
    cases.push(wrong_contract);
    let mut wrong_digest_contract = valid.clone();
    wrong_digest_contract.record_digest_contract = "open-why.record-digest/v0";
    cases.push(wrong_digest_contract);
    let mut malformed_digest = valid.clone();
    malformed_digest.record_digest = "A".repeat(64);
    cases.push(malformed_digest);
    let mut oversized_store = valid.clone();
    oversized_store.store_instance_id = "a".repeat(MAX_STORE_INSTANCE_ID_BYTES + 1);
    cases.push(oversized_store);
    let mut empty_scope = valid.clone();
    empty_scope.scope.clear();
    cases.push(empty_scope);
    let mut oversized_scope = valid.clone();
    oversized_scope.scope = "s".repeat(MAX_COMMIT_LINK_SCOPE_BYTES + 1);
    cases.push(oversized_scope);
    let mut empty_record = valid.clone();
    empty_record.record_id.clear();
    cases.push(empty_record);
    let mut oversized_record = valid.clone();
    oversized_record.record_id = "r".repeat(MAX_COMMIT_LINK_RECORD_ID_BYTES + 1);
    cases.push(oversized_record);

    let observer = Connection::open(store.conn.path().unwrap()).unwrap();
    let version_before: i64 = observer
        .pragma_query_value(None, "data_version", |row| row.get(0))
        .unwrap();
    let changes_before = store.conn.total_changes();
    let expected = serde_json::to_vec(&scoped_commit_link_error(
        ScopedCommitLinkErrorCode::EvidenceUnavailable,
        false,
    ))
    .unwrap();
    for identity in cases {
        let resolution = store.link_git_in_scope(&identity, "abc123", "No link");
        assert_eq!(serde_json::to_vec(&resolution).unwrap(), expected);
        assert_eq!(store.conn.total_changes(), changes_before);
        assert_eq!(
            observer
                .pragma_query_value(None, "data_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            version_before
        );
    }
    assert!(!store
        .linked_commits("sealed-link")
        .unwrap()
        .iter()
        .any(|git_ref| git_ref.0 == "abc123"));
    assert!(!store
        .linked_commits("foreign-link")
        .unwrap()
        .iter()
        .any(|git_ref| git_ref.0 == "abc123"));

    let invalid = store.link_git_in_scope(&valid, "", "No link");
    assert!(matches!(
        invalid,
        ScopedCommitLinkResolution::Error {
            code: ScopedCommitLinkErrorCode::InvalidRequest,
            ..
        }
    ));
    let oversized = "é".repeat(MAX_COMMIT_LINK_HASH_BYTES / 2 + 1);
    assert!(matches!(
        store.link_git_in_scope(&valid, &oversized, "No link"),
        ScopedCommitLinkResolution::Error {
            code: ScopedCommitLinkErrorCode::InvalidRequest,
            ..
        }
    ));
    assert!(matches!(
        store.link_git_in_scope(
            &valid,
            "subject-too-long",
            &"s".repeat(MAX_COMMIT_LINK_SUBJECT_BYTES + 1)
        ),
        ScopedCommitLinkResolution::Error {
            code: ScopedCommitLinkErrorCode::InvalidRequest,
            ..
        }
    ));

    let exact_commit = format!(" {} ", "c".repeat(MAX_COMMIT_LINK_HASH_BYTES - 2));
    let exact_subject = "s".repeat(MAX_COMMIT_LINK_SUBJECT_BYTES);
    let exact = store.link_git_in_scope(&valid, &exact_commit, &exact_subject);
    let ScopedCommitLinkResolution::Ok { git_ref, .. } = exact else {
        panic!("exact input bounds must be accepted");
    };
    assert_eq!(git_ref.commit_hash, exact_commit);
    assert_eq!(git_ref.commit_subject, exact_subject);
}

#[test]
fn scoped_commit_link_recomputes_the_sealed_envelope_before_effects() {
    let store = temp_store();
    store
        .import_external(&[history_row("sealed-link", None, "scope-a", "body")])
        .unwrap();
    let identity = evidence_identity(&store, "sealed-link", "scope-a");
    let trigger: String = store
        .conn
        .query_row(
            "SELECT sql FROM sqlite_schema
                 WHERE type='trigger' AND name='decisions_identity_update_guard'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    store
        .conn
        .execute_batch("DROP TRIGGER decisions_identity_update_guard")
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE decisions SET content='tampered secret' WHERE id='sealed-link'",
            [],
        )
        .unwrap();
    store.conn.execute_batch(&trigger).unwrap();
    let observer = Connection::open(store.conn.path().unwrap()).unwrap();
    let version_before: i64 = observer
        .pragma_query_value(None, "data_version", |row| row.get(0))
        .unwrap();
    let changes_before = store.conn.total_changes();

    assert_eq!(
        store.link_git_in_scope(&identity, "abc123", "No link"),
        scoped_commit_link_error(ScopedCommitLinkErrorCode::EvidenceUnavailable, false)
    );
    assert_eq!(store.conn.total_changes(), changes_before);
    assert_eq!(
        observer
            .pragma_query_value(None, "data_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        version_before
    );
    assert!(!store
        .linked_commits("sealed-link")
        .unwrap()
        .iter()
        .any(|git_ref| git_ref.0 == "abc123"));
}

#[test]
fn scoped_commit_link_reports_held_writer_without_leaking_details() {
    let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!(
            "open-why-scoped-link-lock-{}-{n}",
            std::process::id()
        ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("link.db");
    let store = Store::open_with_store_instance_id(&path, &format!("provider:link:{n}")).unwrap();
    store
        .import_external(&[history_row("sealed-link", None, "scope-a", "body")])
        .unwrap();
    let identity = evidence_identity(&store, "sealed-link", "scope-a");
    store
        .conn
        .busy_timeout(std::time::Duration::from_millis(0))
        .unwrap();
    let writer = Connection::open(&path).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    let changes_before = store.conn.total_changes();

    assert_eq!(
        store.link_git_in_scope(&identity, "abc123", "No link"),
        scoped_commit_link_error(ScopedCommitLinkErrorCode::StoreUnavailable, true)
    );
    assert_eq!(store.conn.total_changes(), changes_before);
    writer.execute_batch("ROLLBACK").unwrap();
    assert!(!store
        .linked_commits("sealed-link")
        .unwrap()
        .iter()
        .any(|git_ref| git_ref.0 == "abc123"));

    store
        .conn
        .execute_batch(
            "CREATE TRIGGER reject_scoped_link
                 BEFORE INSERT ON decision_git_refs
                 WHEN NEW.commit_hash='reject-insert'
                 BEGIN SELECT RAISE(ABORT, 'private sqlite detail'); END",
        )
        .unwrap();
    let changes_before = store.conn.total_changes();
    let rejected = store.link_git_in_scope(&identity, "reject-insert", "No link");
    assert_eq!(
        rejected,
        scoped_commit_link_error(ScopedCommitLinkErrorCode::StoreUnavailable, false)
    );
    assert!(!serde_json::to_string(&rejected)
        .unwrap()
        .contains("private sqlite detail"));
    assert_eq!(store.conn.total_changes(), changes_before);
    assert!(!store
        .linked_commits("sealed-link")
        .unwrap()
        .iter()
        .any(|git_ref| git_ref.0 == "reject-insert"));
    drop(writer);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn concurrent_scoped_commit_links_never_overwrite() {
    let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!(
            "open-why-scoped-link-race-{}-{n}",
            std::process::id()
        ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("link.db");
    let provider = format!("provider:link-race:{n}");
    let store = Store::open_with_store_instance_id(&path, &provider).unwrap();
    store.conn.execute_batch("PRAGMA journal_mode=WAL").unwrap();
    store
        .import_external(&[history_row("sealed-link", None, "scope-a", "body")])
        .unwrap();
    let identity = evidence_identity(&store, "sealed-link", "scope-a");

    let race = |commit: &'static str, subjects: [&'static str; 2]| {
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let mut handles = Vec::new();
        for subject in subjects {
            let path = path.clone();
            let provider = provider.clone();
            let identity = identity.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                let thread_store = Store::open_with_store_instance_id(&path, &provider).unwrap();
                barrier.wait();
                thread_store.link_git_in_scope(&identity, commit, subject)
            }));
        }
        barrier.wait();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    };

    let same = race("same-commit", ["same subject", "same subject"]);
    assert_eq!(
        same.iter()
            .filter(|result| matches!(
                result,
                ScopedCommitLinkResolution::Ok {
                    outcome: ScopedCommitLinkOutcome::Created,
                    ..
                }
            ))
            .count(),
        1
    );
    assert!(same.iter().any(|result| matches!(
        result,
        ScopedCommitLinkResolution::Ok {
            outcome: ScopedCommitLinkOutcome::ExactReplay,
            ..
        } | ScopedCommitLinkResolution::Error {
            code: ScopedCommitLinkErrorCode::StoreUnavailable,
            retryable: true,
            ..
        }
    )));

    let different = race("different-commit", ["first subject", "second subject"]);
    assert_eq!(
        different
            .iter()
            .filter(|result| matches!(
                result,
                ScopedCommitLinkResolution::Ok {
                    outcome: ScopedCommitLinkOutcome::Created,
                    ..
                }
            ))
            .count(),
        1
    );
    assert!(different.iter().any(|result| matches!(
        result,
        ScopedCommitLinkResolution::Error {
            code: ScopedCommitLinkErrorCode::LinkConflict,
            ..
        } | ScopedCommitLinkResolution::Error {
            code: ScopedCommitLinkErrorCode::StoreUnavailable,
            retryable: true,
            ..
        }
    )));
    let stored = store.linked_commits("sealed-link").unwrap();
    assert_eq!(stored.len(), 3);
    assert_eq!(
        stored
            .iter()
            .filter(|git_ref| git_ref.0 == "same-commit")
            .count(),
        1
    );
    assert_eq!(
        stored
            .iter()
            .filter(|git_ref| git_ref.0 == "different-commit")
            .count(),
        1
    );

    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn commit_links_page_exact_hashes_and_fail_closed_authority() {
    let store = temp_store();
    store
        .import_external(&[
            history_row("link-a", None, "scope-a", "a"),
            history_row("link-b", None, "scope-a", "b"),
            history_row("link-c", None, "scope-a", "c"),
            history_row("link-case", None, "scope-a", "case"),
            history_row("link-prefix", None, "scope-a", "prefix"),
            history_row("link-suffix", None, "scope-a", "suffix"),
            history_row("foreign-link", None, "scope-b", "foreign"),
            history_row("retired-link", Some("current-link"), "scope-a", "retired"),
            history_row("current-link", None, "scope-a", "current"),
        ])
        .unwrap();
    for (id, commit, subject) in [
        ("link-c", "ExactHash", "subject c"),
        ("link-a", "ExactHash", "subject a"),
        ("link-b", "ExactHash", "subject b"),
        ("link-case", "exacthash", "case variant"),
        ("link-prefix", "xExactHash", "prefix variant"),
        ("link-suffix", "ExactHashx", "suffix variant"),
        ("foreign-link", "ExactHash", "foreign mixed scope"),
        ("foreign-link", "foreign-only", "foreign only"),
        ("retired-link", "retired-commit", "historical evidence"),
    ] {
        store.link_git(id, commit, subject).unwrap();
    }
    store
        .conn
        .execute(
            "INSERT INTO decision_git_refs
                 (decision_id,commit_hash,commit_subject)
                 VALUES ('orphan-id','orphan-only','orphan')",
            [],
        )
        .unwrap();

    let first = store
        .get_commit_links("scope-a", "ExactHash", None, 2)
        .unwrap();
    let CommitLinksResolution::Ok {
        items, next_cursor, ..
    } = first
    else {
        panic!("expected first commit-link page");
    };
    assert_eq!(
        items
            .iter()
            .map(|item| item.record_id.as_str())
            .collect::<Vec<_>>(),
        ["link-a", "link-b"]
    );
    assert_eq!(items[0].commit_subject, "subject a");
    assert_eq!(next_cursor.as_deref(), Some("link-c"));

    let second = store
        .get_commit_links("scope-a", "ExactHash", Some("link-c"), 2)
        .unwrap();
    let CommitLinksResolution::Ok {
        items, next_cursor, ..
    } = second
    else {
        panic!("expected second commit-link page");
    };
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].record_id, "link-c");
    assert_eq!(next_cursor, None);

    for exact_variant in ["exacthash", "xExactHash", "ExactHashx"] {
        let CommitLinksResolution::Ok { items, .. } = store
            .get_commit_links("scope-a", exact_variant, None, 20)
            .unwrap()
        else {
            panic!("expected isolated exact-hash result");
        };
        assert_eq!(items.len(), 1);
    }

    let error_shape = |resolution| match resolution {
        CommitLinksResolution::Error { code, message, .. } => (code, message),
        CommitLinksResolution::Ok { .. } => panic!("expected commit-link error"),
    };
    let absent = error_shape(
        store
            .get_commit_links("scope-a", "absent", None, 20)
            .unwrap(),
    );
    for (scope, commit) in [
        ("scope-missing", "ExactHash"),
        ("scope-a", "foreign-only"),
        ("scope-a", "orphan-only"),
    ] {
        assert_eq!(
            error_shape(store.get_commit_links(scope, commit, None, 20).unwrap()),
            absent
        );
    }
    assert!(!absent.1.contains("foreign-only"));
    assert!(!absent.1.contains("orphan-id"));

    assert!(matches!(
        store
            .get_commit_links("scope-a", "ExactHash", Some("link-case"), 2)
            .unwrap(),
        CommitLinksResolution::Error {
            code: CommitLinksErrorCode::InvalidCursor,
            ..
        }
    ));
    store.link_git("link-a", "removed-cursor", "a").unwrap();
    store.link_git("link-b", "removed-cursor", "b").unwrap();
    let CommitLinksResolution::Ok { next_cursor, .. } = store
        .get_commit_links("scope-a", "removed-cursor", None, 1)
        .unwrap()
    else {
        panic!("expected cursor fixture page");
    };
    let removed = next_cursor.unwrap();
    store
        .conn
        .execute(
            "DELETE FROM decision_git_refs
                 WHERE decision_id=?1 AND commit_hash='removed-cursor'",
            params![removed],
        )
        .unwrap();
    assert!(matches!(
        store
            .get_commit_links("scope-a", "removed-cursor", Some(&removed), 1)
            .unwrap(),
        CommitLinksResolution::Error {
            code: CommitLinksErrorCode::InvalidCursor,
            ..
        }
    ));

    let CommitLinksResolution::Ok { items, .. } = store
        .get_commit_links("scope-a", "retired-commit", None, 20)
        .unwrap()
    else {
        panic!("expected historical direct link");
    };
    assert_eq!(items[0].record_id, "retired-link");
    let CurrentRecordResolution::Ok { current_id, .. } = store
        .get_current_evidence_at("retired-link", now_epoch(), 64)
        .unwrap()
    else {
        panic!("expected current resolution");
    };
    assert_eq!(current_id, "current-link");
}

#[test]
fn commit_links_reject_oversized_subject_and_aggregate() {
    let store = temp_store();
    let mut rows = Vec::new();
    for index in 0..20 {
        rows.push(history_row(
            &format!("budget-{index:02}"),
            None,
            "scope-a",
            "body",
        ));
    }
    rows.push(history_row("oversized-subject", None, "scope-a", "body"));
    store.import_external(&rows).unwrap();
    store
        .conn
        .execute(
            "INSERT INTO decision_git_refs
                 (decision_id,commit_hash,commit_subject) VALUES (?1,?2,?3)",
            params![
                "oversized-subject",
                "oversized-subject-commit",
                "s".repeat(MAX_COMMIT_LINK_SUBJECT_BYTES + 1)
            ],
        )
        .unwrap();
    assert!(matches!(
        store
            .get_commit_links("scope-a", "oversized-subject-commit", None, 20)
            .unwrap(),
        CommitLinksResolution::Error {
            code: CommitLinksErrorCode::ResponseTooLarge,
            ..
        }
    ));

    let bounded_subject = "e".repeat(MAX_COMMIT_LINK_SUBJECT_BYTES);
    for index in 0..20 {
        store
            .link_git(
                &format!("budget-{index:02}"),
                "aggregate-budget",
                &bounded_subject,
            )
            .unwrap();
    }
    assert!(matches!(
        store
            .get_commit_links("scope-a", "aggregate-budget", None, 20)
            .unwrap(),
        CommitLinksResolution::Error {
            code: CommitLinksErrorCode::ResponseTooLarge,
            ..
        }
    ));
}

#[test]
fn commit_links_use_one_snapshot_despite_concurrent_matching_insert() {
    let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!(
            "open-why-commit-links-snapshot-{}-{n}",
            std::process::id()
        ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("links.db");
    let store = Store::open_with_embedder_and_store_instance_id(
        &path,
        None,
        &format!("provider:links:{n}"),
    )
    .unwrap();
    store
        .conn
        .execute_batch("PRAGMA journal_mode=WAL;")
        .unwrap();
    store
        .import_external(&[
            history_row("snapshot-a", None, "scope-a", "a"),
            history_row("snapshot-b", None, "scope-a", "b"),
            history_row("snapshot-c", None, "scope-a", "c"),
        ])
        .unwrap();
    store.link_git("snapshot-a", "snapshot-hash", "a").unwrap();
    store.link_git("snapshot-c", "snapshot-hash", "c").unwrap();
    let writer = Connection::open(&path).unwrap();
    writer.execute_batch("PRAGMA journal_mode=WAL;").unwrap();

    let resolution = store
        .get_commit_links_with_hook("scope-a", "snapshot-hash", None, 20, || {
            writer.execute(
                "INSERT INTO decision_git_refs
                     (decision_id,commit_hash,commit_subject)
                     VALUES ('snapshot-b','snapshot-hash','b')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let CommitLinksResolution::Ok { items, .. } = resolution else {
        panic!("expected coherent snapshot");
    };
    assert_eq!(
        items
            .iter()
            .map(|item| item.record_id.as_str())
            .collect::<Vec<_>>(),
        ["snapshot-a", "snapshot-c"]
    );
    let CommitLinksResolution::Ok { items, .. } = store
        .get_commit_links("scope-a", "snapshot-hash", None, 20)
        .unwrap()
    else {
        panic!("expected live post-commit snapshot");
    };
    assert_eq!(
        items
            .iter()
            .map(|item| item.record_id.as_str())
            .collect::<Vec<_>>(),
        ["snapshot-a", "snapshot-b", "snapshot-c"]
    );
    drop(writer);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}
