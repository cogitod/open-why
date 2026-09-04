use super::*;

#[test]
fn store_identity_is_stable_distinct_and_preserved_by_copy() {
    let first_dir = temp_dir("stable-store");
    let first_path = first_dir.join("store.db");
    let first = Store::open_with_store_instance_id(&first_path, "provider:first")
        .unwrap()
        .store_identity()
        .unwrap();
    assert_eq!(first.schema_family, STORE_SCHEMA_FAMILY);
    assert_eq!(first.schema_version, STORE_SCHEMA_VERSION);
    assert_eq!(first.store_instance_id, "provider:first");

    let reopened = Store::open(&first_path).unwrap().store_identity().unwrap();
    assert_eq!(reopened, first);

    let second_dir = temp_dir("independent-store");
    let second_path = second_dir.join("store.db");
    let second = Store::open_with_store_instance_id(&second_path, "provider:second")
        .unwrap()
        .store_identity()
        .unwrap();
    assert_ne!(second.store_instance_id, first.store_instance_id);

    let (copy_dir, copy_path) = copy_store(&first_path, "copied-store");
    let copied = Store::open(&copy_path).unwrap().store_identity().unwrap();
    assert_eq!(copied, first);

    std::fs::remove_dir_all(first_dir).unwrap();
    std::fs::remove_dir_all(second_dir).unwrap();
    std::fs::remove_dir_all(copy_dir).unwrap();
}

#[test]
fn initial_binding_requires_a_bounded_provider_identity_and_rejects_mismatch() {
    let root = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!(
            "open-why-provider-required-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
    let path = root.join("nested/store.db");
    let error = Store::open(&path).err().unwrap();
    let binding = error.downcast_ref::<StoreIdentityBindingError>().unwrap();
    assert_eq!(
        binding.code,
        StoreIdentityBindingErrorCode::IdentityRequired
    );
    assert!(!root.exists());

    let invalid = Store::open_with_store_instance_id(&path, "invalid/provider")
        .err()
        .unwrap();
    assert_eq!(
        invalid
            .downcast_ref::<StoreIdentityBindingError>()
            .unwrap()
            .code,
        StoreIdentityBindingErrorCode::InvalidIdentity
    );
    assert!(!root.exists());

    drop(Store::open_with_store_instance_id(&path, "provider:bound").unwrap());
    let mismatch = Store::open_with_store_instance_id(&path, "provider:different")
        .err()
        .unwrap();
    assert_eq!(
        mismatch
            .downcast_ref::<StoreIdentityBindingError>()
            .unwrap()
            .code,
        StoreIdentityBindingErrorCode::IdentityMismatch
    );
    assert_eq!(
        Store::open(&path)
            .unwrap()
            .store_identity()
            .unwrap()
            .store_instance_id,
        "provider:bound"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn concurrent_first_touch_converges_for_same_identity_and_never_overwrites_a_winner() {
    fn race(
        path: &Path,
        identities: [&'static str; 2],
    ) -> Vec<(&'static str, anyhow::Result<String>)> {
        let barrier = Arc::new(Barrier::new(2));
        let mut workers = Vec::new();
        for identity in identities {
            let barrier = Arc::clone(&barrier);
            let path = path.to_owned();
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                let result = Store::open_with_store_instance_id(&path, identity)
                    .and_then(|store| Ok(store.store_identity()?.store_instance_id));
                (identity, result)
            }));
        }
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect()
    }

    let same_dir = temp_dir("same-provider-race");
    let same_path = same_dir.join("store.db");
    let same = race(&same_path, ["provider:same", "provider:same"]);
    assert!(same.iter().all(|(_, result)| {
        matches!(result.as_ref(), Ok(identity) if identity == "provider:same")
    }));

    let different_dir = temp_dir("different-provider-race");
    let different_path = different_dir.join("store.db");
    let different = race(&different_path, ["provider:left", "provider:right"]);
    let winners: Vec<_> = different
        .iter()
        .filter_map(|(candidate, result)| result.as_ref().ok().map(|_| *candidate))
        .collect();
    assert_eq!(winners.len(), 1);
    let loser = different
        .iter()
        .find_map(|(_, result)| result.as_ref().err())
        .unwrap();
    assert_eq!(
        loser
            .downcast_ref::<StoreIdentityBindingError>()
            .unwrap()
            .code,
        StoreIdentityBindingErrorCode::IdentityMismatch
    );
    assert_eq!(
        Store::open(&different_path)
            .unwrap()
            .store_identity()
            .unwrap()
            .store_instance_id,
        winners[0]
    );

    std::fs::remove_dir_all(same_dir).unwrap();
    std::fs::remove_dir_all(different_dir).unwrap();
}

#[test]
fn inspect_store_is_read_only_for_missing_legacy_and_current_paths() {
    let missing_root = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!(
            "open-why-missing-inspect-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
    let missing = missing_root.join("nested/store.db");
    assert!(matches!(
        inspect_store(&missing).unwrap(),
        StoreCompatibility::Missing
    ));
    assert!(!missing_root.exists());

    #[cfg(unix)]
    {
        let guarded_root = temp_dir("missing-inspect-parent-validation");
        let outside = guarded_root.join("outside");
        let valid_parent = guarded_root.join("valid");
        std::fs::create_dir(&outside).unwrap();
        std::fs::create_dir(&valid_parent).unwrap();
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o751)).unwrap();
        std::fs::set_permissions(&valid_parent, std::fs::Permissions::from_mode(0o750)).unwrap();
        let link = guarded_root.join("link");
        symlink(&outside, &link).unwrap();

        assert!(inspect_store(&link.join("missing.db")).is_err());
        assert!(!outside.join("missing.db").exists());
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        assert_eq!(
            std::fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
            0o751
        );

        assert!(matches!(
            inspect_store(&valid_parent.join("missing.db")).unwrap(),
            StoreCompatibility::Missing
        ));
        assert!(!valid_parent.join("missing.db").exists());
        assert_eq!(std::fs::read_dir(&valid_parent).unwrap().count(), 0);
        assert_eq!(
            std::fs::metadata(&valid_parent)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o750
        );
        std::fs::remove_dir_all(guarded_root).unwrap();
    }

    let legacy_dir = temp_dir("legacy-inspect");
    let legacy_path = legacy_dir.join("legacy.db");
    create_legacy(&legacy_path);
    let before = std::fs::metadata(&legacy_path).unwrap().modified().unwrap();
    let before_files: Vec<_> = std::fs::read_dir(&legacy_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    let StoreCompatibility::MigrationRequired {
        from,
        to,
        plan_digest,
    } = inspect_store(&legacy_path).unwrap()
    else {
        panic!("legacy database should require migration");
    };
    assert_eq!((from, to), (0, STORE_SCHEMA_VERSION));
    assert_eq!(plan_digest.len(), 64);
    assert_eq!(
        std::fs::metadata(&legacy_path).unwrap().modified().unwrap(),
        before
    );
    let after_files: Vec<_> = std::fs::read_dir(&legacy_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(after_files, before_files);

    let store = Store::open_with_store_instance_id(&legacy_path, "provider:legacy").unwrap();
    let expected = store.store_identity().unwrap();
    let observer = Connection::open(&legacy_path).unwrap();
    let schema_before: i64 = observer
        .pragma_query_value(None, "schema_version", |record| record.get(0))
        .unwrap();
    let data_before: i64 = observer
        .pragma_query_value(None, "data_version", |record| record.get(0))
        .unwrap();
    drop(store);
    let mtime_before = std::fs::metadata(&legacy_path).unwrap().modified().unwrap();
    let files_before: Vec<_> = std::fs::read_dir(&legacy_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(matches!(
        inspect_store(&legacy_path).unwrap(),
        StoreCompatibility::Compatible { identity } if identity == expected
    ));
    assert_eq!(
        observer
            .pragma_query_value(None, "schema_version", |record| record.get::<_, i64>(0))
            .unwrap(),
        schema_before
    );
    assert_eq!(
        observer
            .pragma_query_value(None, "data_version", |record| record.get::<_, i64>(0))
            .unwrap(),
        data_before
    );
    assert_eq!(
        std::fs::metadata(&legacy_path).unwrap().modified().unwrap(),
        mtime_before
    );
    let files_after: Vec<_> = std::fs::read_dir(&legacy_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(files_after, files_before);
    drop(observer);
    std::fs::remove_dir_all(legacy_dir).unwrap();
}

#[test]
fn inspection_fails_closed_on_newer_partial_checksum_shape_and_metadata_drift() {
    let source_dir = temp_dir("inspection-source");
    let source = source_dir.join("store.db");
    drop(Store::open_with_store_instance_id(&source, "provider:inspection").unwrap());

    let cases = [
        (
            "newer",
            "PRAGMA user_version=2;",
            StoreCompatibilityErrorCode::SchemaNewer,
        ),
        (
            "partial",
            "DROP TABLE open_why_metadata;",
            StoreCompatibilityErrorCode::PartialMigration,
        ),
        (
            "checksum",
            "UPDATE open_why_migrations SET checksum_sha256='00' WHERE sequence=1;",
            StoreCompatibilityErrorCode::ChecksumMismatch,
        ),
        (
            "shape",
            "DROP INDEX idx_decisions_scope;",
            StoreCompatibilityErrorCode::ShapeDrift,
        ),
        (
            "metadata",
            "UPDATE open_why_metadata SET store_instance_id='invalid/provider';",
            StoreCompatibilityErrorCode::SchemaCorrupt,
        ),
    ];
    for (label, mutation, expected_code) in cases {
        let (dir, path) = copy_store(&source, label);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(mutation).unwrap();
        drop(conn);
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        let files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(matches!(
            inspect_store(&path).unwrap(),
            StoreCompatibility::Incompatible { code, .. } if code == expected_code
        ));
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), mtime);
        assert_eq!(
            std::fs::read_dir(&dir)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<Vec<_>>(),
            files
        );
        assert!(Store::open(&path).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    let malformed_dir = temp_dir("malformed");
    let malformed = malformed_dir.join("store.db");
    std::fs::write(&malformed, b"not a sqlite database").unwrap();
    let mtime = std::fs::metadata(&malformed).unwrap().modified().unwrap();
    assert!(matches!(
        inspect_store(&malformed).unwrap(),
        StoreCompatibility::Incompatible {
            code: StoreCompatibilityErrorCode::SchemaCorrupt,
            ..
        }
    ));
    assert_eq!(
        std::fs::metadata(&malformed).unwrap().modified().unwrap(),
        mtime
    );
    assert_eq!(std::fs::read_dir(&malformed_dir).unwrap().count(), 1);
    std::fs::remove_dir_all(malformed_dir).unwrap();

    let rogue_dir = temp_dir("rogue-v0");
    let rogue = rogue_dir.join("store.db");
    Connection::open(&rogue)
        .unwrap()
        .execute_batch("CREATE TABLE decisions (id TEXT PRIMARY KEY);")
        .unwrap();
    assert!(matches!(
        inspect_store(&rogue).unwrap(),
        StoreCompatibility::Incompatible {
            code: StoreCompatibilityErrorCode::ShapeDrift,
            ..
        }
    ));
    assert!(Store::open_with_store_instance_id(&rogue, "provider:rogue").is_err());
    let rogue_check = Connection::open(&rogue).unwrap();
    assert_eq!(
        rogue_check
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name LIKE 'open_why_%'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
    drop(rogue_check);
    std::fs::remove_dir_all(rogue_dir).unwrap();
    std::fs::remove_dir_all(source_dir).unwrap();
}

#[test]
fn inspect_store_fails_closed_on_live_wal_without_touching_any_file() {
    let dir = temp_dir("wal-inspect");
    let path = dir.join("store.db");
    drop(Store::open_with_store_instance_id(&path, "provider:wal").unwrap());
    let conn = Connection::open(&path).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.execute_batch(
        "BEGIN IMMEDIATE;
         PRAGMA user_version=2;
         DROP INDEX idx_decisions_scope;
         COMMIT;",
    )
    .unwrap();
    let wal = PathBuf::from(format!("{}-wal", path.display()));
    let shm = PathBuf::from(format!("{}-shm", path.display()));
    assert!(wal.exists());
    assert!(shm.exists());
    let schema_before: i64 = conn
        .pragma_query_value(None, "schema_version", |record| record.get(0))
        .unwrap();
    let data_before: i64 = conn
        .pragma_query_value(None, "data_version", |record| record.get(0))
        .unwrap();
    let before = [file_state(&path), file_state(&wal), file_state(&shm)];
    assert!(matches!(
        inspect_store(&path).unwrap(),
        StoreCompatibility::Incompatible {
            code: StoreCompatibilityErrorCode::LiveWalIndeterminate,
            ..
        }
    ));
    assert_eq!(
        [file_state(&path), file_state(&wal), file_state(&shm)],
        before
    );
    assert_eq!(
        conn.pragma_query_value(None, "schema_version", |record| record.get::<_, i64>(0))
            .unwrap(),
        schema_before
    );
    assert_eq!(
        conn.pragma_query_value(None, "data_version", |record| record.get::<_, i64>(0))
            .unwrap(),
        data_before
    );
    drop(conn);
    std::fs::remove_dir_all(dir).unwrap();
}
