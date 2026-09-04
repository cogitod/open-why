use super::*;

#[test]
fn real_cli_same_tick_replay_keeps_current_resolution_valid() {
    let dir = temp_dir("cli-same-tick");
    let db_path = dir.join("store.db");
    let provider = "provider:cli-same-tick";
    let capture = |id: &str, title: &str, supersedes: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_why"));
        command
            .arg("capture")
            .arg("--id")
            .arg(id)
            .arg("--title")
            .arg(title)
            .arg("--content")
            .arg("same-tick body")
            .arg("--scope")
            .arg("repo-a")
            .env("OPEN_WHY_DB", &db_path)
            .env("OPEN_WHY_STORE_INSTANCE_ID", provider);
        if let Some(old) = supersedes {
            command.arg("--supersedes").arg(old);
        }
        command.output().unwrap()
    };

    let (old_id, new_id, new_title) = (0..32)
        .find_map(|attempt| {
            let old_id = format!("cli-tick-old-{attempt}");
            let new_id = format!("cli-tick-new-{attempt}");
            let old_title = format!("CLI tick old {attempt}");
            let new_title = format!("CLI tick new {attempt}");
            assert!(capture(&old_id, &old_title, None).status.success());
            assert!(capture(&new_id, &new_title, None).status.success());
            let observer = Connection::open(&db_path).unwrap();
            let ticks: (Option<String>, Option<String>) = observer
                .query_row(
                    "SELECT
                       (SELECT valid_from FROM decisions WHERE id=?1),
                       (SELECT valid_from FROM decisions WHERE id=?2)",
                    [&old_id, &new_id],
                    |record| Ok((record.get(0)?, record.get(1)?)),
                )
                .unwrap();
            (ticks.0 == ticks.1).then_some((old_id, new_id, new_title))
        })
        .expect("32 immediate CLI capture pairs must include one production-clock tick pair");
    let observer = Connection::open(&db_path).unwrap();
    let digest_before: String = observer
        .query_row(
            "SELECT record_digest_v1 FROM decisions WHERE id=?1",
            [&new_id],
            |record| record.get(0),
        )
        .unwrap();

    assert!(capture(&new_id, &new_title, Some(&old_id)).status.success());
    let first_relation: (Option<String>, Option<String>, Option<String>) = observer
        .query_row(
            "SELECT superseded_by,valid_from,valid_until FROM decisions WHERE id=?1",
            [&old_id],
            |record| Ok((record.get(0)?, record.get(1)?, record.get(2)?)),
        )
        .unwrap();
    assert!(capture(&new_id, &new_title, Some(&old_id)).status.success());
    let second_relation: (Option<String>, Option<String>, Option<String>) = observer
        .query_row(
            "SELECT superseded_by,valid_from,valid_until FROM decisions WHERE id=?1",
            [&old_id],
            |record| Ok((record.get(0)?, record.get(1)?, record.get(2)?)),
        )
        .unwrap();
    let digest_after: String = observer
        .query_row(
            "SELECT record_digest_v1 FROM decisions WHERE id=?1",
            [&new_id],
            |record| record.get(0),
        )
        .unwrap();

    assert_eq!(first_relation, second_relation);
    assert_eq!(first_relation.0.as_deref(), Some(new_id.as_str()));
    assert!(first_relation.1.as_deref().unwrap() < first_relation.2.as_deref().unwrap());
    assert_eq!(digest_after, digest_before);

    let get = Command::new(env!("CARGO_BIN_EXE_why"))
        .arg("get")
        .arg(&old_id)
        .env("OPEN_WHY_DB", &db_path)
        .env("OPEN_WHY_STORE_INSTANCE_ID", provider)
        .output()
        .unwrap();
    assert!(get.status.success());
    let stdout = String::from_utf8(get.stdout).unwrap();
    assert!(stdout.contains(&format!("[{new_id}]")));
    assert!(stdout.contains(&format!("{old_id} -> {new_id}")));
    assert!(!stdout.contains("InvalidTemporalData"));

    drop(observer);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn real_cli_retirement_rejects_invalid_timestamp_domains_without_effects() {
    let dir = temp_dir("cli-retirement-domain");
    let db_path = dir.join("store.db");
    let input_path = dir.join("legacy-rows.json");
    let provider = "provider:cli-retirement-domain";
    let mut maximum = row("cli-domain-old-maximum", "repo-a");
    maximum.title = "CLI domain old maximum".to_owned();
    maximum.valid_from = Some("9999-12-31T23:59:59Z".to_owned());
    maximum.valid_until = None;
    maximum.fact_key = None;
    let mut malformed = row("cli-domain-old-malformed", "repo-a");
    malformed.title = "CLI domain old malformed".to_owned();
    malformed.valid_from = Some("legacy-not-a-time".to_owned());
    malformed.valid_until = None;
    malformed.fact_key = None;
    let mut noncanonical = row("cli-domain-old-noncanonical", "repo-a");
    noncanonical.title = "CLI domain old noncanonical".to_owned();
    noncanonical.valid_from = Some("2026X01Y01Q00R00S00Z".to_owned());
    noncanonical.valid_until = None;
    noncanonical.fact_key = None;
    let over_bound_time = format!("2026-01-01T00:00:00.{}Z", "1".repeat(108));
    assert_eq!(over_bound_time.len(), MAX_TEMPORAL_VALUE_BYTES + 1);
    let mut over_bound = row("cli-domain-old-over-bound", "repo-a");
    over_bound.title = "CLI domain old over-bound".to_owned();
    over_bound.valid_from = Some(over_bound_time);
    over_bound.valid_until = None;
    over_bound.fact_key = None;
    std::fs::write(
        &input_path,
        serde_json::to_vec(&[maximum, malformed, noncanonical, over_bound]).unwrap(),
    )
    .unwrap();
    let imported = Command::new(env!("CARGO_BIN_EXE_why"))
        .arg("import")
        .arg("--file")
        .arg(&input_path)
        .env("OPEN_WHY_DB", &db_path)
        .env("OPEN_WHY_STORE_INSTANCE_ID", provider)
        .output()
        .unwrap();
    assert!(imported.status.success());
    assert_eq!(
        String::from_utf8(imported.stdout).unwrap(),
        "imported 4 decisions\n"
    );

    let capture = |id: &str, title: &str, supersedes: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_why"));
        command
            .arg("capture")
            .arg("--id")
            .arg(id)
            .arg("--title")
            .arg(title)
            .arg("--content")
            .arg("domain successor body")
            .arg("--scope")
            .arg("repo-a")
            .arg("--valid-from")
            .arg("2026-01-01T00:00:00Z")
            .env("OPEN_WHY_DB", &db_path)
            .env("OPEN_WHY_STORE_INSTANCE_ID", provider);
        if let Some(old) = supersedes {
            command.arg("--supersedes").arg(old);
        }
        command.output().unwrap()
    };
    let observer = Connection::open(&db_path).unwrap();
    let snapshot = |old_id: &str, new_id: &str| {
        observer
            .query_row(
                "SELECT
                   (SELECT count(*) FROM decisions),
                   superseded_by,valid_until,record_digest_v1,
                   (SELECT record_digest_v1 FROM decisions WHERE id=?2),
                   (SELECT count(*) FROM decision_git_refs)
                 FROM decisions WHERE id=?1",
                [old_id, new_id],
                |record| {
                    Ok((
                        record.get::<_, i64>(0)?,
                        record.get::<_, Option<String>>(1)?,
                        record.get::<_, Option<String>>(2)?,
                        record.get::<_, String>(3)?,
                        record.get::<_, Option<String>>(4)?,
                        record.get::<_, i64>(5)?,
                    ))
                },
            )
            .unwrap()
    };
    for label in ["maximum", "malformed", "noncanonical", "over-bound"] {
        let old_id = format!("cli-domain-old-{label}");
        let new_id = format!("cli-domain-new-{label}");
        let title = format!("CLI domain new {label}");
        let before = snapshot(&old_id, &new_id);
        let rejected = capture(&new_id, &title, Some(&old_id));
        assert!(!rejected.status.success());
        assert!(String::from_utf8(rejected.stderr)
            .unwrap()
            .contains("invalid_temporal_data"));
        assert_eq!(snapshot(&old_id, &new_id), before);
    }

    drop(observer);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn public_capture_returns_typed_supersession_conflict_without_effects() {
    let dir = temp_dir("public-supersession-conflict");
    let db_path = dir.join("store.db");
    let store = Store::open_with_store_instance_id(&db_path, "provider:relation-conflict").unwrap();
    let decision = |title: &str, content: &str| Decision {
        subject: title.to_owned(),
        body: content.to_owned(),
        kind: "decision".to_owned(),
        source: "capture".to_owned(),
        importance: 0.5,
        ..Decision::default()
    };
    let wanted = decision("Wanted successor", "wanted body");
    let other = decision("Other successor", "other body");
    store
        .capture_external(
            &other,
            "repo-a",
            "public-other-new",
            Some("2026-02-01T00:00:00Z"),
            None,
            None,
        )
        .unwrap();
    let mut predecessor = row("public-conflicting-old", "repo-a");
    predecessor.title = "Public conflicting predecessor".to_owned();
    predecessor.valid_from = Some("legacy-not-a-time".to_owned());
    predecessor.valid_until = Some("2026-02-01T00:00:00Z".to_owned());
    predecessor.superseded_by = Some("public-other-new".to_owned());
    predecessor.fact_key = None;
    store.import_external(&[predecessor]).unwrap();
    let snapshot = || {
        let observer = Connection::open(&db_path).unwrap();
        let rows: Vec<(String, Option<String>, Option<String>, String)> = observer
            .prepare(
                "SELECT id,superseded_by,valid_until,record_digest_v1
                 FROM decisions ORDER BY id",
            )
            .unwrap()
            .query_map([], |record| {
                Ok((
                    record.get(0)?,
                    record.get(1)?,
                    record.get(2)?,
                    record.get(3)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        rows
    };
    let before = snapshot();

    let error = store
        .capture_external(
            &wanted,
            "repo-a",
            "public-wanted-new",
            Some("2026-03-01T00:00:00Z"),
            None,
            Some("public-conflicting-old"),
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
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn public_capture_validates_time_and_missing_target_before_all_effects() {
    let dir = temp_dir("public-capture-preflight");
    let db_path = dir.join("store.db");
    let store = Store::open_with_store_instance_id(&db_path, "provider:capture-preflight").unwrap();
    let decision = |title: &str| Decision {
        subject: title.to_owned(),
        body: "capture preflight body".to_owned(),
        kind: "decision".to_owned(),
        source: "capture".to_owned(),
        importance: 0.5,
        ..Decision::default()
    };
    let boundary = format!("2026-01-01T00:00:00.{}Z", "1".repeat(107));
    assert_eq!(boundary.len(), MAX_TEMPORAL_VALUE_BYTES);
    store
        .capture_external(
            &decision("Boundary capture"),
            "repo-a",
            "capture-boundary",
            Some(&boundary),
            None,
            None,
        )
        .unwrap();
    let snapshot = || {
        let observer = Connection::open(&db_path).unwrap();
        let decisions: Vec<(String, Option<String>, Option<String>, String)> = observer
            .prepare(
                "SELECT id,superseded_by,valid_until,record_digest_v1
                 FROM decisions ORDER BY id",
            )
            .unwrap()
            .query_map([], |record| {
                Ok((
                    record.get(0)?,
                    record.get(1)?,
                    record.get(2)?,
                    record.get(3)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let git_refs: i64 = observer
            .query_row("SELECT count(*) FROM decision_git_refs", [], |record| {
                record.get(0)
            })
            .unwrap();
        let fts: i64 = observer
            .query_row("SELECT count(*) FROM decisions_fts", [], |record| {
                record.get(0)
            })
            .unwrap();
        (decisions, git_refs, fts)
    };
    let before = snapshot();
    let over_bound = format!("2026-01-01T00:00:00.{}Z", "1".repeat(108));
    for (id, timestamp) in [
        ("capture-over-bound", over_bound.as_str()),
        ("capture-noncanonical", "2026X01Y01Q00R00S00Z"),
    ] {
        let error = store
            .capture_external(&decision(id), "repo-a", id, Some(timestamp), None, None)
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<CurrentRecordErrorCode>(),
            Some(&CurrentRecordErrorCode::InvalidTemporalData)
        );
        assert_eq!(snapshot(), before);
    }
    let missing = store
        .capture_external(
            &decision("Missing predecessor"),
            "repo-a",
            "capture-missing-target",
            Some("2026-03-01T00:00:00Z"),
            None,
            Some("absent-predecessor"),
        )
        .unwrap_err();
    assert_eq!(
        missing.downcast_ref::<SupersessionTargetNotFound>(),
        Some(&SupersessionTargetNotFound)
    );
    assert!(!missing.to_string().contains("absent-predecessor"));
    assert_eq!(snapshot(), before);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn public_capture_paths_reject_cross_scope_explicit_supersession_before_all_effects() {
    let dir = temp_dir("public-cross-scope-supersession");
    let db_path = dir.join("store.db");
    let store =
        Store::open_with_store_instance_id(&db_path, "provider:cross-scope-public").unwrap();
    let decision = |title: &str, body: &str| Decision {
        subject: title.to_owned(),
        body: body.to_owned(),
        kind: "decision".to_owned(),
        source: "cross-scope-test".to_owned(),
        importance: 0.5,
        ..Decision::default()
    };
    let external_old_id = "foreign-external-predecessor";
    store
        .capture_external(
            &decision("Foreign external predecessor", "external predecessor body"),
            "repo-a",
            external_old_id,
            Some("2026-01-01T00:00:00Z"),
            None,
            None,
        )
        .unwrap();
    let ordinary_old_id = store
        .capture(
            &decision("Foreign ordinary predecessor", "ordinary predecessor body"),
            "repo-a",
            None,
        )
        .unwrap();
    let snapshot = || {
        let observer = Connection::open(&db_path).unwrap();
        let decisions: Vec<SupersessionSnapshotRow> = observer
            .prepare(
                "SELECT id,scope,superseded_by,valid_until,record_digest_v1
                 FROM decisions ORDER BY id",
            )
            .unwrap()
            .query_map([], |record| {
                Ok((
                    record.get(0)?,
                    record.get(1)?,
                    record.get(2)?,
                    record.get(3)?,
                    record.get(4)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let git_refs: i64 = observer
            .query_row("SELECT count(*) FROM decision_git_refs", [], |record| {
                record.get(0)
            })
            .unwrap();
        let fts: i64 = observer
            .query_row("SELECT count(*) FROM decisions_fts", [], |record| {
                record.get(0)
            })
            .unwrap();
        (decisions, git_refs, fts)
    };
    let before = snapshot();

    let attempts = [
        (
            store
                .capture_external(
                    &decision("Cross-scope external successor", "external successor body"),
                    "repo-b",
                    "cross-scope-external-successor",
                    Some("2026-03-01T00:00:00Z"),
                    None,
                    Some(external_old_id),
                )
                .unwrap_err(),
            external_old_id,
        ),
        (
            store
                .capture(
                    &decision("Cross-scope ordinary successor", "ordinary successor body"),
                    "repo-b",
                    Some(&ordinary_old_id),
                )
                .unwrap_err(),
            ordinary_old_id.as_str(),
        ),
    ];
    for (error, foreign_id) in attempts {
        assert_eq!(
            error.downcast_ref::<SupersessionTargetNotFound>(),
            Some(&SupersessionTargetNotFound)
        );
        assert_eq!(
            error.to_string(),
            "supersession_target_not_found: predecessor was not found"
        );
        assert!(!error.to_string().contains(foreign_id));
        assert!(!error.to_string().contains("repo-a"));
        assert_eq!(snapshot(), before);
    }

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn public_capture_paths_prevent_bounded_supersession_cycles_before_all_effects() {
    let dir = temp_dir("public-cycle-prevention");
    let db_path = dir.join("store.db");
    let store = Store::open_with_store_instance_id(&db_path, "provider:cycle-public").unwrap();
    let scope = "repo-a";
    let external = |id: &str, title: &str, fact_key: Option<&str>, supersedes: Option<&str>| {
        store.capture_external(
            &capture_decision(title),
            scope,
            id,
            Some("2026-01-01T00:00:00Z"),
            fact_key,
            supersedes,
        )
    };

    external("public-self-external", "Public self external", None, None).unwrap();
    let before = full_capture_snapshot(&db_path);
    let error = external(
        "public-self-external",
        "Public self external",
        None,
        Some("public-self-external"),
    )
    .unwrap_err();
    assert_public_cycle_rejected(&db_path, &before, error, &["public-self-external"]);

    let ordinary_self = capture_decision("Public self ordinary");
    let ordinary_self_id = store.capture(&ordinary_self, scope, None).unwrap();
    let before = full_capture_snapshot(&db_path);
    let error = store
        .capture(&ordinary_self, scope, Some(&ordinary_self_id))
        .unwrap_err();
    assert_public_cycle_rejected(&db_path, &before, error, &[&ordinary_self_id]);

    external("public-ext-two-a", "Public ext two A", None, None).unwrap();
    external(
        "public-ext-two-b",
        "Public ext two B",
        None,
        Some("public-ext-two-a"),
    )
    .unwrap();
    let before = full_capture_snapshot(&db_path);
    let error = external(
        "public-ext-two-a",
        "Public ext two A",
        None,
        Some("public-ext-two-b"),
    )
    .unwrap_err();
    assert_public_cycle_rejected(&db_path, &before, error, &["public-ext-two-b"]);

    external("public-ext-three-a", "Public ext three A", None, None).unwrap();
    external(
        "public-ext-three-b",
        "Public ext three B",
        None,
        Some("public-ext-three-a"),
    )
    .unwrap();
    external(
        "public-ext-three-c",
        "Public ext three C",
        None,
        Some("public-ext-three-b"),
    )
    .unwrap();
    let before = full_capture_snapshot(&db_path);
    let error = external(
        "public-ext-three-a",
        "Public ext three A",
        None,
        Some("public-ext-three-c"),
    )
    .unwrap_err();
    assert_public_cycle_rejected(&db_path, &before, error, &["public-ext-three-c"]);

    let ordinary_two_a = capture_decision("Public ordinary two A");
    let ordinary_two_b = capture_decision("Public ordinary two B");
    let ordinary_two_a_id = store.capture(&ordinary_two_a, scope, None).unwrap();
    let ordinary_two_b_id = store
        .capture(&ordinary_two_b, scope, Some(&ordinary_two_a_id))
        .unwrap();
    let before = full_capture_snapshot(&db_path);
    let error = store
        .capture(&ordinary_two_a, scope, Some(&ordinary_two_b_id))
        .unwrap_err();
    assert_public_cycle_rejected(&db_path, &before, error, &[&ordinary_two_b_id]);

    let ordinary_three_a = capture_decision("Public ordinary three A");
    let ordinary_three_b = capture_decision("Public ordinary three B");
    let ordinary_three_c = capture_decision("Public ordinary three C");
    let ordinary_three_a_id = store.capture(&ordinary_three_a, scope, None).unwrap();
    let ordinary_three_b_id = store
        .capture(&ordinary_three_b, scope, Some(&ordinary_three_a_id))
        .unwrap();
    let ordinary_three_c_id = store
        .capture(&ordinary_three_c, scope, Some(&ordinary_three_b_id))
        .unwrap();
    let before = full_capture_snapshot(&db_path);
    let error = store
        .capture(&ordinary_three_a, scope, Some(&ordinary_three_c_id))
        .unwrap_err();
    assert_public_cycle_rejected(&db_path, &before, error, &[&ordinary_three_c_id]);

    external(
        "public-fact-a",
        "Public automatic fact A",
        Some("public-cycle-fact"),
        None,
    )
    .unwrap();
    external(
        "public-fact-b",
        "Public automatic fact B",
        Some("public-cycle-fact"),
        None,
    )
    .unwrap();
    let before = full_capture_snapshot(&db_path);
    let error = external(
        "public-fact-a",
        "Public automatic fact A",
        Some("public-cycle-fact"),
        None,
    )
    .unwrap_err();
    assert_public_cycle_rejected(&db_path, &before, error, &["public-fact-b"]);

    external("public-title-a", "Public automatic title", None, None).unwrap();
    external("public-title-b", "Public automatic title", None, None).unwrap();
    let before = full_capture_snapshot(&db_path);
    let error = external("public-title-a", "Public automatic title", None, None).unwrap_err();
    assert_public_cycle_rejected(&db_path, &before, error, &["public-title-b"]);

    for (candidate, successor, predecessor) in [
        (
            "public-broken-candidate",
            "public-secret-absent",
            "public-broken-old",
        ),
        (
            "public-cycle-candidate",
            "public-cycle-node",
            "public-cycle-old",
        ),
        ("public-malformed-candidate", "", "public-malformed-old"),
    ] {
        external(candidate, candidate, None, None).unwrap();
        external(predecessor, predecessor, None, None).unwrap();
        if !successor.is_empty() && candidate.contains("cycle-candidate") {
            external(successor, successor, None, None).unwrap();
        }
        let mutator = Connection::open(&db_path).unwrap();
        if candidate.contains("malformed") {
            mutator
                .execute(
                    "UPDATE decisions SET superseded_by=X'FF' WHERE id=?1",
                    [candidate],
                )
                .unwrap();
        } else {
            mutator
                .execute(
                    "UPDATE decisions SET superseded_by=?1 WHERE id=?2",
                    [successor, candidate],
                )
                .unwrap();
            if candidate.contains("cycle-candidate") {
                mutator
                    .execute(
                        "UPDATE decisions SET superseded_by=?1 WHERE id=?2",
                        [candidate, successor],
                    )
                    .unwrap();
            }
        }
        drop(mutator);
        let before = full_capture_snapshot(&db_path);
        let error = external(candidate, candidate, None, Some(predecessor)).unwrap_err();
        assert_public_cycle_rejected(&db_path, &before, error, &[successor, predecessor]);
    }

    external(
        "public-foreign-candidate",
        "Public foreign candidate",
        None,
        None,
    )
    .unwrap();
    store
        .capture_external(
            &capture_decision("Public foreign node"),
            "repo-b",
            "public-secret-foreign-node",
            Some("2026-01-01T00:00:00Z"),
            None,
            None,
        )
        .unwrap();
    external("public-foreign-old", "Public foreign old", None, None).unwrap();
    let mutator = Connection::open(&db_path).unwrap();
    mutator
        .execute(
            "UPDATE decisions SET superseded_by='public-secret-foreign-node'
             WHERE id='public-foreign-candidate'",
            [],
        )
        .unwrap();
    drop(mutator);
    let before = full_capture_snapshot(&db_path);
    let error = external(
        "public-foreign-candidate",
        "Public foreign candidate",
        None,
        Some("public-foreign-old"),
    )
    .unwrap_err();
    assert_public_cycle_rejected(
        &db_path,
        &before,
        error,
        &["public-secret-foreign-node", "repo-b"],
    );

    for index in 0..(MAX_SUPERSESSION_CHAIN - 1) {
        let id = format!("public-boundary-ok-{index:03}");
        external(&id, &id, None, None).unwrap();
    }
    external(
        "public-boundary-ok-old",
        "Public boundary ok old",
        None,
        None,
    )
    .unwrap();
    let mutator = Connection::open(&db_path).unwrap();
    for index in 0..(MAX_SUPERSESSION_CHAIN - 2) {
        mutator
            .execute(
                "UPDATE decisions SET superseded_by=?1 WHERE id=?2",
                [
                    format!("public-boundary-ok-{:03}", index + 1),
                    format!("public-boundary-ok-{index:03}"),
                ],
            )
            .unwrap();
    }
    drop(mutator);
    external(
        "public-boundary-ok-000",
        "public-boundary-ok-000",
        None,
        Some("public-boundary-ok-old"),
    )
    .unwrap();
    let resolution = store
        .get_current_evidence_in_scope("public-boundary-ok-old", scope)
        .unwrap();
    let ScopedCurrentRecordResolution::Ok {
        current_id,
        supersession_chain,
        ..
    } = resolution
    else {
        panic!("63 successors plus predecessor must resolve as a 64-record chain");
    };
    assert_eq!(current_id, "public-boundary-ok-062");
    assert_eq!(supersession_chain.len(), MAX_SUPERSESSION_CHAIN);

    for index in 0..MAX_SUPERSESSION_CHAIN {
        let id = format!("public-limit-{index:03}");
        external(&id, &id, None, None).unwrap();
    }
    external("public-limit-old", "Public limit old", None, None).unwrap();
    let mutator = Connection::open(&db_path).unwrap();
    for index in 0..(MAX_SUPERSESSION_CHAIN - 1) {
        mutator
            .execute(
                "UPDATE decisions SET superseded_by=?1 WHERE id=?2",
                [
                    format!("public-limit-{:03}", index + 1),
                    format!("public-limit-{index:03}"),
                ],
            )
            .unwrap();
    }
    drop(mutator);
    let before = full_capture_snapshot(&db_path);
    let error = external(
        "public-limit-000",
        "public-limit-000",
        None,
        Some("public-limit-old"),
    )
    .unwrap_err();
    assert_public_cycle_rejected(&db_path, &before, error, &["public-limit-063"]);

    std::fs::remove_dir_all(dir).unwrap();
}
