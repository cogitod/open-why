use super::super::*;
use super::support::*;

#[test]
fn feedback_moves_effectiveness_and_is_clamped() {
    let store = temp_store();
    let id = store
        .capture(
            &decision("use sqlite", "single file local-first", 0.5, None),
            "global",
            None,
        )
        .unwrap();
    // Ungraded prior is 0.5; a helpful verdict raises it by 0.05.
    let eff = store.feedback(&id, true).unwrap().unwrap();
    assert!((eff - 0.55).abs() < 1e-9, "expected 0.55, got {eff}");
    // A not-helpful verdict lowers it by 0.03.
    let eff = store.feedback(&id, false).unwrap().unwrap();
    assert!((eff - 0.52).abs() < 1e-9, "expected 0.52, got {eff}");
    // Unknown id returns None and records nothing.
    assert!(store.feedback("no-such-id", true).unwrap().is_none());
}

#[test]
fn exact_capture_replay_completes_every_requested_supersession_mode() {
    let store = temp_store();

    store
        .import_external(&[history_row(
            "generated-old",
            None,
            "global",
            "generated predecessor",
        )])
        .unwrap();
    let generated = decision("generated successor", "stable body", 0.5, None);
    let generated_id = store.capture(&generated, "global", None).unwrap();
    let generated_digest = Store::record_digest_row_by_id_on(&store.conn, &generated_id)
        .unwrap()
        .unwrap()
        .sealed_digest;
    assert_eq!(
        store
            .capture(&generated, "global", Some("generated-old"))
            .unwrap(),
        generated_id
    );

    store
        .import_external(&[history_row(
            "explicit-old",
            None,
            "global",
            "explicit predecessor",
        )])
        .unwrap();
    let explicit = decision("explicit successor", "stable body", 0.5, None);
    store
        .capture_external(
            &explicit,
            "global",
            "explicit-new",
            Some("2026-03-01T00:00:00Z"),
            None,
            None,
        )
        .unwrap();
    let explicit_digest = Store::record_digest_row_by_id_on(&store.conn, "explicit-new")
        .unwrap()
        .unwrap()
        .sealed_digest;
    store
        .capture_external(
            &explicit,
            "global",
            "explicit-new",
            Some("2026-03-01T00:00:00Z"),
            None,
            Some("explicit-old"),
        )
        .unwrap();

    let keyed = decision("keyed successor", "stable body", 0.5, None);
    store
        .capture_external(
            &keyed,
            "global",
            "keyed-new",
            Some("2026-03-01T00:00:00Z"),
            Some("shared-key"),
            None,
        )
        .unwrap();
    let keyed_digest = Store::record_digest_row_by_id_on(&store.conn, "keyed-new")
        .unwrap()
        .unwrap()
        .sealed_digest;
    let mut keyed_old = history_row("keyed-old", None, "global", "keyed predecessor");
    keyed_old.title = "different title".to_owned();
    keyed_old.fact_key = Some("shared-key".to_owned());
    store.import_external(&[keyed_old]).unwrap();
    store
        .capture_external(
            &keyed,
            "global",
            "keyed-new",
            Some("2026-03-01T00:00:00Z"),
            Some("shared-key"),
            None,
        )
        .unwrap();

    let titled = decision("shared title", "stable body", 0.5, None);
    store
        .capture_external(
            &titled,
            "global",
            "titled-new",
            Some("2026-03-01T00:00:00Z"),
            None,
            None,
        )
        .unwrap();
    let titled_digest = Store::record_digest_row_by_id_on(&store.conn, "titled-new")
        .unwrap()
        .unwrap()
        .sealed_digest;
    let mut titled_old = history_row("titled-old", None, "global", "title predecessor");
    titled_old.title = "shared title".to_owned();
    store.import_external(&[titled_old]).unwrap();
    store
        .capture_external(
            &titled,
            "global",
            "titled-new",
            Some("2026-03-01T00:00:00Z"),
            None,
            None,
        )
        .unwrap();

    for (old, successor) in [
        ("generated-old", generated_id.as_str()),
        ("explicit-old", "explicit-new"),
        ("keyed-old", "keyed-new"),
        ("titled-old", "titled-new"),
    ] {
        let actual: Option<String> = store
            .conn
            .query_row(
                "SELECT superseded_by FROM decisions WHERE id=?1",
                [old],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(actual.as_deref(), Some(successor), "replay mode {old}");
    }
    for (id, before) in [
        (generated_id.as_str(), generated_digest),
        ("explicit-new", explicit_digest),
        ("keyed-new", keyed_digest),
        ("titled-new", titled_digest),
    ] {
        assert_eq!(
            Store::record_digest_row_by_id_on(&store.conn, id)
                .unwrap()
                .unwrap()
                .sealed_digest,
            before,
            "replay rotated the sealed digest for {id}"
        );
    }
}

#[test]
fn conflicting_external_capture_fails_before_any_supersession_effect() {
    let store = temp_store();
    let original = decision("shared conflict title", "sealed body", 0.5, None);
    store
        .capture_external(
            &original,
            "global",
            "sealed-new",
            Some("2026-03-01T00:00:00Z"),
            Some("shared-conflict-key"),
            None,
        )
        .unwrap();
    let sealed_before = Store::record_digest_row_by_id_on(&store.conn, "sealed-new")
        .unwrap()
        .unwrap()
        .sealed_digest;

    let explicit_old = history_row("conflict-explicit-old", None, "global", "explicit");
    let mut keyed_old = history_row("conflict-keyed-old", None, "global", "keyed");
    keyed_old.title = "different conflict title".to_owned();
    keyed_old.fact_key = Some("shared-conflict-key".to_owned());
    let mut titled_old = history_row("conflict-titled-old", None, "global", "titled");
    titled_old.title = "shared conflict title".to_owned();
    store
        .import_external(&[explicit_old, keyed_old, titled_old])
        .unwrap();

    let mut conflict = original;
    conflict.body = "changed immutable body".to_owned();
    let error = store
        .capture_external(
            &conflict,
            "global",
            "sealed-new",
            Some("2026-03-01T00:00:00Z"),
            Some("shared-conflict-key"),
            Some("conflict-explicit-old"),
        )
        .unwrap_err();
    assert!(error.downcast_ref::<RecordIdentityConflict>().is_some());

    for id in [
        "conflict-explicit-old",
        "conflict-keyed-old",
        "conflict-titled-old",
    ] {
        let successor: Option<String> = store
            .conn
            .query_row(
                "SELECT superseded_by FROM decisions WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(successor, None, "conflict changed relation for {id}");
    }
    assert_eq!(
        Store::record_digest_row_by_id_on(&store.conn, "sealed-new")
            .unwrap()
            .unwrap()
            .sealed_digest,
        sealed_before
    );
}

#[test]
fn same_tick_exact_replay_keeps_a_positive_resolvable_interval() {
    let store = temp_store();
    let (old_id, new_id, successor) = (0..32)
        .find_map(|attempt| {
            let old_id = format!("same-tick-old-{attempt}");
            let new_id = format!("same-tick-new-{attempt}");
            let old = decision(&format!("same tick old {attempt}"), "old body", 0.5, None);
            let successor = decision(&format!("same tick new {attempt}"), "new body", 0.5, None);
            store
                .capture_external(&old, "global", &old_id, None, None, None)
                .unwrap();
            store
                .capture_external(&successor, "global", &new_id, None, None, None)
                .unwrap();
            let ticks: (Option<String>, Option<String>) = store
                .conn
                .query_row(
                    "SELECT
                           (SELECT valid_from FROM decisions WHERE id=?1),
                           (SELECT valid_from FROM decisions WHERE id=?2)",
                    params![old_id, new_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            (ticks.0 == ticks.1).then_some((old_id, new_id, successor))
        })
        .expect("32 immediate capture pairs must include one production-clock tick pair");
    let digest_before = Store::record_digest_row_by_id_on(&store.conn, &new_id)
        .unwrap()
        .unwrap()
        .sealed_digest;

    store
        .capture_external(&successor, "global", &new_id, None, None, Some(&old_id))
        .unwrap();
    let first_relation: (Option<String>, Option<String>, Option<String>) = store
        .conn
        .query_row(
            "SELECT superseded_by,valid_from,valid_until FROM decisions WHERE id=?1",
            [&old_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    store
        .capture_external(&successor, "global", &new_id, None, None, Some(&old_id))
        .unwrap();
    let second_relation: (Option<String>, Option<String>, Option<String>) = store
        .conn
        .query_row(
            "SELECT superseded_by,valid_from,valid_until FROM decisions WHERE id=?1",
            [&old_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();

    assert_eq!(first_relation, second_relation);
    assert_eq!(first_relation.0.as_deref(), Some(new_id.as_str()));
    assert!(
        iso_to_epoch(first_relation.2.as_deref().unwrap()).unwrap()
            > iso_to_epoch(first_relation.1.as_deref().unwrap()).unwrap()
    );
    assert_eq!(
        Store::record_digest_row_by_id_on(&store.conn, &new_id)
            .unwrap()
            .unwrap()
            .sealed_digest,
        digest_before
    );
    assert!(matches!(
        store
            .get_current_evidence_in_scope(&old_id, "global")
            .unwrap(),
        ScopedCurrentRecordResolution::Ok {
            current_id,
            ref record,
            ..
        } if current_id == new_id && record.id == new_id
    ));
}

#[test]
fn iso_to_epoch_accepts_only_canonical_utc_timestamps() {
    for valid in [
        "1970-01-01T00:00:00Z",
        "2000-02-29T23:59:59Z",
        "2026-09-03T12:34:56.1Z",
        "2026-09-03T12:34:56.123456789Z",
        "9999-12-31T23:59:59Z",
    ] {
        assert!(iso_to_epoch(valid).is_some(), "rejected canonical {valid}");
    }
    let boundary = format!("2026-09-03T12:34:56.{}Z", "1".repeat(107));
    let over_bound = format!("2026-09-03T12:34:56.{}Z", "1".repeat(108));
    assert_eq!(boundary.len(), MAX_TEMPORAL_VALUE_BYTES);
    assert!(iso_to_epoch(&boundary).is_some());
    assert_eq!(over_bound.len(), MAX_TEMPORAL_VALUE_BYTES + 1);
    assert!(iso_to_epoch(&over_bound).is_none());
    for invalid in [
        "2026X01Y01Q00R00S00Z",
        "2026-01-01 00:00:00Z",
        "2026-01-01T00:00:00z",
        "2026-01-01T00:00:00Ztrailing",
        "2026-01-01T00:00:00.Z",
        "2026-01-01T00:00:00.1Ztrailing",
        "2026-01-01T24:00:00Z",
        "2026-01-01T00:60:00Z",
        "2026-01-01T00:00:60Z",
        "2026-02-29T00:00:00Z",
        "2024-02-30T00:00:00Z",
        "2026-04-31T00:00:00Z",
        "1969-12-31T23:59:59Z",
        "10000-01-01T00:00:00Z",
    ] {
        assert!(iso_to_epoch(invalid).is_none(), "accepted {invalid}");
    }
}

#[test]
fn completed_exact_relation_replay_skips_malformed_predecessor_time() {
    let store = temp_store();
    let successor = decision("completed successor", "successor body", 0.5, None);
    store
        .capture_external(
            &successor,
            "global",
            "completed-new",
            Some("2026-02-01T00:00:00Z"),
            None,
            None,
        )
        .unwrap();
    let mut predecessor = history_row(
        "completed-old",
        Some("completed-new"),
        "global",
        "predecessor body",
    );
    predecessor.title = "completed predecessor".to_owned();
    predecessor.valid_from = Some("legacy-not-a-time".to_owned());
    predecessor.git_refs.clear();
    store.import_external(&[predecessor]).unwrap();
    let snapshot = || {
        store
            .conn
            .query_row(
                "SELECT superseded_by,valid_from,valid_until,record_digest_v1,
                            (SELECT record_digest_v1 FROM decisions WHERE id='completed-new'),
                            (SELECT count(*) FROM decisions),
                            (SELECT count(*) FROM decision_git_refs)
                     FROM decisions WHERE id='completed-old'",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                    ))
                },
            )
            .unwrap()
    };
    let before = snapshot();

    store
        .capture_external(
            &successor,
            "global",
            "completed-new",
            Some("2026-02-01T00:00:00Z"),
            None,
            Some("completed-old"),
        )
        .unwrap();

    assert_eq!(snapshot(), before);
}

#[test]
fn completed_different_relation_returns_typed_conflict_before_time_parsing() {
    let store = temp_store();
    let wanted = decision("wanted successor", "wanted body", 0.5, None);
    let other = decision("other successor", "other body", 0.5, None);
    store
        .capture_external(
            &other,
            "global",
            "other-new",
            Some("2026-02-01T00:00:00Z"),
            None,
            None,
        )
        .unwrap();
    let mut predecessor = history_row(
        "conflicting-old",
        Some("other-new"),
        "global",
        "predecessor body",
    );
    predecessor.title = "conflicting predecessor".to_owned();
    predecessor.valid_from = Some("legacy-not-a-time".to_owned());
    predecessor.git_refs.clear();
    store.import_external(&[predecessor]).unwrap();
    let snapshot = || {
        let decisions: Vec<(String, Option<String>, Option<String>, String)> = store
            .conn
            .prepare(
                "SELECT id,superseded_by,valid_until,record_digest_v1
                     FROM decisions ORDER BY id",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let relation_count: i64 = store
            .conn
            .query_row("SELECT count(*) FROM decision_git_refs", [], |row| {
                row.get(0)
            })
            .unwrap();
        (decisions, relation_count)
    };
    let before = snapshot();

    let error = store
        .capture_external(
            &wanted,
            "global",
            "wanted-new",
            Some("2026-03-01T00:00:00Z"),
            None,
            Some("conflicting-old"),
        )
        .unwrap_err();

    assert_eq!(
        error.downcast_ref::<SupersessionConflict>(),
        Some(&SupersessionConflict)
    );
    assert_eq!(
        error.to_string(),
        "supersession_conflict: predecessor already names a different successor"
    );
    assert_eq!(snapshot(), before);
}

#[test]
fn missing_supersession_target_fails_typed_before_either_capture_inserts() {
    let store = temp_store();
    let snapshot = || {
        let decision_count: i64 = store
            .conn
            .query_row("SELECT count(*) FROM decisions", [], |row| row.get(0))
            .unwrap();
        let relation_count: i64 = store
            .conn
            .query_row("SELECT count(*) FROM decision_git_refs", [], |row| {
                row.get(0)
            })
            .unwrap();
        let fts_count: i64 = store
            .conn
            .query_row("SELECT count(*) FROM decisions_fts", [], |row| row.get(0))
            .unwrap();
        (decision_count, relation_count, fts_count)
    };
    let before = snapshot();
    let decision = decision("missing target", "must not persist", 0.5, None);

    for error in [
        store
            .capture_external(
                &decision,
                "global",
                "missing-target-external",
                Some("2026-03-01T00:00:00Z"),
                None,
                Some("absent-old"),
            )
            .unwrap_err(),
        store
            .capture(&decision, "global", Some("absent-old"))
            .unwrap_err(),
    ] {
        assert_eq!(
            error.downcast_ref::<SupersessionTargetNotFound>(),
            Some(&SupersessionTargetNotFound)
        );
        assert_eq!(
            error.to_string(),
            "supersession_target_not_found: predecessor was not found"
        );
        assert_eq!(snapshot(), before);
    }
}

#[test]
fn changed_relation_during_apply_rolls_back_candidate_and_all_retirements() {
    let store = temp_store();
    let mut explicit = history_row("race-a-explicit", None, "global", "explicit body");
    explicit.title = "different title".to_owned();
    explicit.git_refs.clear();
    let mut automatic = history_row("race-z-automatic", None, "global", "automatic body");
    automatic.title = "race successor".to_owned();
    automatic.git_refs.clear();
    store.import_external(&[explicit, automatic]).unwrap();
    let snapshot = || {
        let decisions: Vec<(String, Option<String>, Option<String>, String)> = store
            .conn
            .prepare(
                "SELECT id,superseded_by,valid_until,record_digest_v1
                     FROM decisions ORDER BY id",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let fts: Vec<String> = store
            .conn
            .prepare("SELECT title FROM decisions_fts ORDER BY title")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let relation_count: i64 = store
            .conn
            .query_row("SELECT count(*) FROM decision_git_refs", [], |row| {
                row.get(0)
            })
            .unwrap();
        (decisions, fts, relation_count)
    };
    let before = snapshot();
    let successor = decision("race successor", "candidate body", 0.5, None);

    let error = store
        .capture_external_with_pre_retirement_hook(
            ExternalCaptureRequest {
                decision: &successor,
                scope: "global",
                id: "race-candidate",
                valid_from: Some("2026-03-01T00:00:00Z"),
                fact_key: None,
                supersedes: Some("race-a-explicit"),
            },
            |tx| {
                tx.execute(
                    "UPDATE decisions
                         SET valid_until='2026-02-01T00:00:00Z'
                         WHERE id='race-z-automatic'",
                    [],
                )?;
                Ok(())
            },
        )
        .unwrap_err();

    assert_eq!(
        error.downcast_ref::<SupersessionConflict>(),
        Some(&SupersessionConflict)
    );
    assert_eq!(snapshot(), before);
}

#[test]
fn automatic_retirement_preflights_all_predecessors_before_effects() {
    let store = temp_store();
    let successor = decision("automatic shared title", "successor body", 0.5, None);
    store
        .capture_external(
            &successor,
            "global",
            "automatic-new",
            Some("2026-02-01T00:00:00Z"),
            None,
            None,
        )
        .unwrap();
    let mut valid = history_row("automatic-a-valid", None, "global", "valid predecessor");
    valid.title = "automatic shared title".to_owned();
    valid.git_refs.clear();
    let mut invalid = history_row("automatic-z-invalid", None, "global", "invalid predecessor");
    invalid.title = "automatic shared title".to_owned();
    invalid.valid_from = Some("2026X01Y01Q00R00S00Z".to_owned());
    invalid.git_refs.clear();
    store.import_external(&[valid, invalid]).unwrap();
    let snapshot = || {
        let mut statement = store
            .conn
            .prepare(
                "SELECT id,superseded_by,valid_from,valid_until,record_digest_v1
                     FROM decisions ORDER BY id",
            )
            .unwrap();
        let decisions = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        let relations: Vec<(String, String, String)> = store
            .conn
            .prepare(
                "SELECT decision_id,commit_hash,commit_subject
                     FROM decision_git_refs ORDER BY decision_id,commit_hash",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        (decisions, relations)
    };
    let before = snapshot();

    let error = store
        .capture_external(
            &successor,
            "global",
            "automatic-new",
            Some("2026-02-01T00:00:00Z"),
            None,
            None,
        )
        .unwrap_err();

    assert_eq!(
        error.downcast_ref::<CurrentRecordErrorCode>(),
        Some(&CurrentRecordErrorCode::InvalidTemporalData)
    );
    assert_eq!(snapshot(), before);
}

#[test]
fn retirement_rejects_out_of_domain_and_malformed_predecessor_time_before_effects() {
    let store = temp_store();
    let over_bound = format!("2026-01-01T00:00:00.{}Z", "1".repeat(108));
    for (label, valid_from, parses) in [
        ("maximum", "9999-12-31T23:59:59Z".to_owned(), true),
        ("malformed", "legacy-not-a-time".to_owned(), false),
        ("noncanonical", "2026X01Y01Q00R00S00Z".to_owned(), false),
        ("over-bound", over_bound, false),
    ] {
        assert_eq!(iso_to_epoch(&valid_from).is_some(), parses);
        let old_id = format!("domain-old-{label}");
        let new_id = format!("domain-new-{label}");
        let mut predecessor = history_row(&old_id, None, "global", "predecessor body");
        predecessor.title = format!("domain old {label}");
        predecessor.valid_from = Some(valid_from);
        predecessor.git_refs.clear();
        store.import_external(&[predecessor]).unwrap();
        let successor = decision(&format!("domain new {label}"), "successor body", 0.5, None);
        let snapshot = || {
            store
                .conn
                .query_row(
                    "SELECT
                           (SELECT count(*) FROM decisions),
                           superseded_by,valid_from,valid_until,record_digest_v1,
                           (SELECT record_digest_v1 FROM decisions WHERE id=?2),
                           (SELECT count(*) FROM decision_git_refs)
                         FROM decisions WHERE id=?1",
                    params![old_id, new_id],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, Option<String>>(5)?,
                            row.get::<_, i64>(6)?,
                        ))
                    },
                )
                .unwrap()
        };
        let before = snapshot();
        let error = store
            .capture_external(
                &successor,
                "global",
                &new_id,
                Some("2026-01-01T00:00:00Z"),
                None,
                Some(&old_id),
            )
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<CurrentRecordErrorCode>(),
            Some(&CurrentRecordErrorCode::InvalidTemporalData)
        );
        assert_eq!(snapshot(), before, "{label} changed stored evidence");
    }
}
