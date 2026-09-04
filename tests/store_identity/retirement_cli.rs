use super::*;

#[test]
fn real_cli_capture_validates_time_and_missing_target_before_all_effects() {
    let dir = temp_dir("cli-capture-preflight");
    let db_path = dir.join("store.db");
    let provider = "provider:cli-capture-preflight";
    let capture = |id: &str, timestamp: &str, supersedes: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_why"));
        command
            .arg("capture")
            .arg("--id")
            .arg(id)
            .arg("--title")
            .arg(format!("CLI capture {id}"))
            .arg("--content")
            .arg("CLI capture preflight body")
            .arg("--scope")
            .arg("repo-a")
            .arg("--valid-from")
            .arg(timestamp)
            .env("OPEN_WHY_DB", &db_path)
            .env("OPEN_WHY_STORE_INSTANCE_ID", provider);
        if let Some(predecessor) = supersedes {
            command.arg("--supersedes").arg(predecessor);
        }
        command.output().unwrap()
    };
    let boundary = format!("2026-01-01T00:00:00.{}Z", "1".repeat(107));
    assert_eq!(boundary.len(), MAX_TEMPORAL_VALUE_BYTES);
    assert!(capture("cli-boundary", &boundary, None).status.success());
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
        (decisions, git_refs)
    };
    let before = snapshot();
    let over_bound = format!("2026-01-01T00:00:00.{}Z", "1".repeat(108));
    for (id, timestamp) in [
        ("cli-over-bound", over_bound.as_str()),
        ("cli-noncanonical", "2026X01Y01Q00R00S00Z"),
    ] {
        let rejected = capture(id, timestamp, None);
        assert!(!rejected.status.success());
        assert!(String::from_utf8(rejected.stderr)
            .unwrap()
            .contains("invalid_temporal_data"));
        assert_eq!(snapshot(), before);
    }
    let missing = capture(
        "cli-missing-target",
        "2026-03-01T00:00:00Z",
        Some("absent-predecessor"),
    );
    assert!(!missing.status.success());
    let missing_error = String::from_utf8(missing.stderr).unwrap();
    assert!(missing_error.contains("supersession_target_not_found"));
    assert!(!missing_error.contains("absent-predecessor"));
    assert_eq!(snapshot(), before);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn real_cli_capture_paths_reject_cross_scope_explicit_supersession_before_all_effects() {
    let dir = temp_dir("cli-cross-scope-supersession");
    let db_path = dir.join("store.db");
    let provider = "provider:cross-scope-cli";
    let capture = |id: Option<&str>, title: &str, scope: &str, supersedes: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_why"));
        command
            .arg("capture")
            .arg("--title")
            .arg(title)
            .arg("--content")
            .arg(format!("body for {title}"))
            .arg("--scope")
            .arg(scope)
            .env("OPEN_WHY_DB", &db_path)
            .env("OPEN_WHY_STORE_INSTANCE_ID", provider);
        if let Some(id) = id {
            command.arg("--id").arg(id);
        }
        if let Some(predecessor) = supersedes {
            command.arg("--supersedes").arg(predecessor);
        }
        command.output().unwrap()
    };
    let external_old_id = "cli-foreign-external-predecessor";
    assert!(capture(
        Some(external_old_id),
        "CLI foreign external predecessor",
        "repo-a",
        None,
    )
    .status
    .success());
    assert!(
        capture(None, "CLI foreign ordinary predecessor", "repo-a", None,)
            .status
            .success()
    );
    let ordinary_old_id: String = Connection::open(&db_path)
        .unwrap()
        .query_row(
            "SELECT id FROM decisions
             WHERE scope='repo-a' AND title='CLI foreign ordinary predecessor'",
            [],
            |record| record.get(0),
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
            capture(
                Some("cli-cross-scope-external-successor"),
                "CLI cross-scope external successor",
                "repo-b",
                Some(external_old_id),
            ),
            external_old_id,
        ),
        (
            capture(
                None,
                "CLI cross-scope ordinary successor",
                "repo-b",
                Some(&ordinary_old_id),
            ),
            ordinary_old_id.as_str(),
        ),
    ];
    for (rejected, foreign_id) in attempts {
        assert!(!rejected.status.success());
        let error = String::from_utf8(rejected.stderr).unwrap();
        assert!(error.contains("supersession_target_not_found"));
        assert!(!error.contains(foreign_id));
        assert!(!error.contains("repo-a"));
        assert_eq!(snapshot(), before);
    }

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn real_cli_capture_paths_prevent_bounded_supersession_cycles_before_all_effects() {
    let dir = temp_dir("cli-cycle-prevention");
    let db_path = dir.join("store.db");
    let provider = "provider:cycle-cli";
    let capture = |id: Option<&str>,
                   title: &str,
                   scope: &str,
                   fact_key: Option<&str>,
                   supersedes: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_why"));
        command
            .arg("capture")
            .arg("--title")
            .arg(title)
            .arg("--content")
            .arg(format!("body for {title}"))
            .arg("--scope")
            .arg(scope)
            .env("OPEN_WHY_DB", &db_path)
            .env("OPEN_WHY_STORE_INSTANCE_ID", provider);
        if let Some(id) = id {
            command
                .arg("--id")
                .arg(id)
                .arg("--valid-from")
                .arg("2026-01-01T00:00:00Z");
        }
        if let Some(key) = fact_key {
            command.arg("--fact-key").arg(key);
        }
        if let Some(predecessor) = supersedes {
            command.arg("--supersedes").arg(predecessor);
        }
        command.output().unwrap()
    };
    let id_for_title = |title: &str| -> String {
        Connection::open(&db_path)
            .unwrap()
            .query_row(
                "SELECT id FROM decisions WHERE scope='repo-a' AND title=?1",
                [title],
                |record| record.get(0),
            )
            .unwrap()
    };

    assert!(capture(
        Some("cli-cycle-self-external"),
        "CLI cycle self external",
        "repo-a",
        None,
        None,
    )
    .status
    .success());
    let before = full_capture_snapshot(&db_path);
    let rejected = capture(
        Some("cli-cycle-self-external"),
        "CLI cycle self external",
        "repo-a",
        None,
        Some("cli-cycle-self-external"),
    );
    assert_cli_cycle_rejected(&db_path, &before, rejected, &["cli-cycle-self-external"]);

    assert!(
        capture(None, "CLI cycle self ordinary", "repo-a", None, None,)
            .status
            .success()
    );
    let ordinary_self_id = id_for_title("CLI cycle self ordinary");
    let before = full_capture_snapshot(&db_path);
    let rejected = capture(
        None,
        "CLI cycle self ordinary",
        "repo-a",
        None,
        Some(&ordinary_self_id),
    );
    assert_cli_cycle_rejected(&db_path, &before, rejected, &[&ordinary_self_id]);

    for (prefix, length) in [("cli-ext-two", 2_usize), ("cli-ext-three", 3_usize)] {
        for index in 0..length {
            let id = format!("{prefix}-{index}");
            let title = format!("{prefix} title {index}");
            let predecessor = (index > 0).then(|| format!("{prefix}-{}", index - 1));
            assert!(
                capture(Some(&id), &title, "repo-a", None, predecessor.as_deref(),)
                    .status
                    .success()
            );
        }
        let predecessor = format!("{prefix}-{}", length - 1);
        let before = full_capture_snapshot(&db_path);
        let rejected = capture(
            Some(&format!("{prefix}-0")),
            &format!("{prefix} title 0"),
            "repo-a",
            None,
            Some(&predecessor),
        );
        assert_cli_cycle_rejected(&db_path, &before, rejected, &[&predecessor]);
    }

    for (prefix, length) in [
        ("CLI ordinary two", 2_usize),
        ("CLI ordinary three", 3_usize),
    ] {
        let mut ids = Vec::new();
        for index in 0..length {
            let title = format!("{prefix} {index}");
            assert!(
                capture(None, &title, "repo-a", None, ids.last().map(String::as_str),)
                    .status
                    .success()
            );
            ids.push(id_for_title(&title));
        }
        let before = full_capture_snapshot(&db_path);
        let rejected = capture(
            None,
            &format!("{prefix} 0"),
            "repo-a",
            None,
            ids.last().map(String::as_str),
        );
        assert_cli_cycle_rejected(&db_path, &before, rejected, &[ids.last().unwrap()]);
    }

    assert!(capture(
        Some("cli-cycle-fact-a"),
        "CLI cycle fact A",
        "repo-a",
        Some("cli-cycle-fact"),
        None,
    )
    .status
    .success());
    assert!(capture(
        Some("cli-cycle-fact-b"),
        "CLI cycle fact B",
        "repo-a",
        Some("cli-cycle-fact"),
        None,
    )
    .status
    .success());
    let before = full_capture_snapshot(&db_path);
    let rejected = capture(
        Some("cli-cycle-fact-a"),
        "CLI cycle fact A",
        "repo-a",
        Some("cli-cycle-fact"),
        None,
    );
    assert_cli_cycle_rejected(&db_path, &before, rejected, &["cli-cycle-fact-b"]);

    assert!(capture(
        Some("cli-cycle-title-a"),
        "CLI cycle shared title",
        "repo-a",
        None,
        None,
    )
    .status
    .success());
    assert!(capture(
        Some("cli-cycle-title-b"),
        "CLI cycle shared title",
        "repo-a",
        None,
        None,
    )
    .status
    .success());
    let before = full_capture_snapshot(&db_path);
    let rejected = capture(
        Some("cli-cycle-title-a"),
        "CLI cycle shared title",
        "repo-a",
        None,
        None,
    );
    assert_cli_cycle_rejected(&db_path, &before, rejected, &["cli-cycle-title-b"]);

    for (candidate, successor, predecessor) in [
        (
            "cli-broken-candidate",
            "cli-secret-absent",
            "cli-broken-old",
        ),
        ("cli-cycle-candidate", "cli-cycle-node", "cli-cycle-old"),
        ("cli-malformed-candidate", "", "cli-malformed-old"),
    ] {
        assert!(capture(Some(candidate), candidate, "repo-a", None, None)
            .status
            .success());
        assert!(
            capture(Some(predecessor), predecessor, "repo-a", None, None)
                .status
                .success()
        );
        if !successor.is_empty() && candidate.contains("cycle-candidate") {
            assert!(capture(Some(successor), successor, "repo-a", None, None)
                .status
                .success());
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
        let rejected = capture(
            Some(candidate),
            candidate,
            "repo-a",
            None,
            Some(predecessor),
        );
        assert_cli_cycle_rejected(&db_path, &before, rejected, &[successor, predecessor]);
    }

    assert!(capture(
        Some("cli-foreign-candidate"),
        "CLI foreign cycle candidate",
        "repo-a",
        None,
        None,
    )
    .status
    .success());
    assert!(capture(
        Some("cli-secret-foreign-node"),
        "CLI secret foreign node",
        "repo-b",
        None,
        None,
    )
    .status
    .success());
    assert!(capture(
        Some("cli-foreign-old"),
        "CLI foreign cycle old",
        "repo-a",
        None,
        None,
    )
    .status
    .success());
    let mutator = Connection::open(&db_path).unwrap();
    mutator
        .execute(
            "UPDATE decisions SET superseded_by='cli-secret-foreign-node'
             WHERE id='cli-foreign-candidate'",
            [],
        )
        .unwrap();
    drop(mutator);
    let before = full_capture_snapshot(&db_path);
    let rejected = capture(
        Some("cli-foreign-candidate"),
        "CLI foreign cycle candidate",
        "repo-a",
        None,
        Some("cli-foreign-old"),
    );
    assert_cli_cycle_rejected(
        &db_path,
        &before,
        rejected,
        &["cli-secret-foreign-node", "repo-b"],
    );

    for index in 0..(MAX_SUPERSESSION_CHAIN - 1) {
        let id = format!("cli-boundary-ok-{index:03}");
        assert!(capture(Some(&id), &id, "repo-a", None, None)
            .status
            .success());
    }
    assert!(capture(
        Some("cli-boundary-ok-old"),
        "CLI boundary ok old",
        "repo-a",
        None,
        None,
    )
    .status
    .success());
    let mutator = Connection::open(&db_path).unwrap();
    for index in 0..(MAX_SUPERSESSION_CHAIN - 2) {
        mutator
            .execute(
                "UPDATE decisions SET superseded_by=?1 WHERE id=?2",
                [
                    format!("cli-boundary-ok-{:03}", index + 1),
                    format!("cli-boundary-ok-{index:03}"),
                ],
            )
            .unwrap();
    }
    drop(mutator);
    assert!(capture(
        Some("cli-boundary-ok-000"),
        "cli-boundary-ok-000",
        "repo-a",
        None,
        Some("cli-boundary-ok-old"),
    )
    .status
    .success());
    let current = Command::new(env!("CARGO_BIN_EXE_why"))
        .arg("get")
        .arg("cli-boundary-ok-old")
        .env("OPEN_WHY_DB", &db_path)
        .env("OPEN_WHY_STORE_INSTANCE_ID", provider)
        .output()
        .unwrap();
    assert!(
        current.status.success(),
        "{}",
        String::from_utf8_lossy(&current.stderr)
    );
    let current_output = String::from_utf8(current.stdout).unwrap();
    assert!(current_output.contains("[cli-boundary-ok-062]"));

    for index in 0..MAX_SUPERSESSION_CHAIN {
        let id = format!("cli-limit-{index:03}");
        assert!(capture(Some(&id), &id, "repo-a", None, None)
            .status
            .success());
    }
    assert!(
        capture(Some("cli-limit-old"), "CLI limit old", "repo-a", None, None,)
            .status
            .success()
    );
    let mutator = Connection::open(&db_path).unwrap();
    for index in 0..(MAX_SUPERSESSION_CHAIN - 1) {
        mutator
            .execute(
                "UPDATE decisions SET superseded_by=?1 WHERE id=?2",
                [
                    format!("cli-limit-{:03}", index + 1),
                    format!("cli-limit-{index:03}"),
                ],
            )
            .unwrap();
    }
    drop(mutator);
    let before = full_capture_snapshot(&db_path);
    let rejected = capture(
        Some("cli-limit-000"),
        "cli-limit-000",
        "repo-a",
        None,
        Some("cli-limit-old"),
    );
    assert_cli_cycle_rejected(&db_path, &before, rejected, &["cli-limit-063"]);

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn real_cli_completed_relation_replay_ignores_malformed_predecessor_time() {
    let dir = temp_dir("cli-completed-malformed-replay");
    let db_path = dir.join("store.db");
    let input_path = dir.join("completed-row.json");
    let provider = "provider:cli-completed-malformed-replay";
    let capture = |supersedes: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_why"));
        command
            .arg("capture")
            .arg("--id")
            .arg("cli-completed-new")
            .arg("--title")
            .arg("CLI completed successor")
            .arg("--content")
            .arg("completed successor body")
            .arg("--scope")
            .arg("repo-a")
            .arg("--valid-from")
            .arg("2026-02-01T00:00:00Z")
            .env("OPEN_WHY_DB", &db_path)
            .env("OPEN_WHY_STORE_INSTANCE_ID", provider);
        if let Some(old) = supersedes {
            command.arg("--supersedes").arg(old);
        }
        command.output().unwrap()
    };
    assert!(capture(None).status.success());

    let mut predecessor = row("cli-completed-old", "repo-a");
    predecessor.title = "CLI completed predecessor".to_owned();
    predecessor.valid_from = Some("legacy-not-a-time".to_owned());
    predecessor.valid_until = Some("2026-02-01T00:00:00Z".to_owned());
    predecessor.superseded_by = Some("cli-completed-new".to_owned());
    predecessor.fact_key = None;
    std::fs::write(&input_path, serde_json::to_vec(&[predecessor]).unwrap()).unwrap();
    let imported = Command::new(env!("CARGO_BIN_EXE_why"))
        .arg("import")
        .arg("--file")
        .arg(&input_path)
        .env("OPEN_WHY_DB", &db_path)
        .env("OPEN_WHY_STORE_INSTANCE_ID", provider)
        .output()
        .unwrap();
    assert!(imported.status.success());

    let observer = Connection::open(&db_path).unwrap();
    let snapshot = || {
        observer
            .query_row(
                "SELECT superseded_by,valid_from,valid_until,record_digest_v1,
                        (SELECT record_digest_v1 FROM decisions WHERE id='cli-completed-new'),
                        (SELECT count(*) FROM decisions),
                        (SELECT count(*) FROM decision_git_refs)
                 FROM decisions WHERE id='cli-completed-old'",
                [],
                |record| {
                    Ok((
                        record.get::<_, Option<String>>(0)?,
                        record.get::<_, Option<String>>(1)?,
                        record.get::<_, Option<String>>(2)?,
                        record.get::<_, String>(3)?,
                        record.get::<_, String>(4)?,
                        record.get::<_, i64>(5)?,
                        record.get::<_, i64>(6)?,
                    ))
                },
            )
            .unwrap()
    };
    let before = snapshot();
    let replayed = capture(Some("cli-completed-old"));
    assert!(
        replayed.status.success(),
        "{}",
        String::from_utf8_lossy(&replayed.stderr)
    );
    assert_eq!(snapshot(), before);

    let before_conflict = snapshot();
    let rejected = Command::new(env!("CARGO_BIN_EXE_why"))
        .arg("capture")
        .arg("--id")
        .arg("cli-conflicting-new")
        .arg("--title")
        .arg("CLI conflicting successor")
        .arg("--content")
        .arg("conflicting successor body")
        .arg("--scope")
        .arg("repo-a")
        .arg("--valid-from")
        .arg("2026-03-01T00:00:00Z")
        .arg("--supersedes")
        .arg("cli-completed-old")
        .env("OPEN_WHY_DB", &db_path)
        .env("OPEN_WHY_STORE_INSTANCE_ID", provider)
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(String::from_utf8(rejected.stderr)
        .unwrap()
        .contains("supersession_conflict"));
    assert_eq!(snapshot(), before_conflict);

    drop(observer);
    std::fs::remove_dir_all(dir).unwrap();
}
