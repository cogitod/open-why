use super::super::*;
use super::support::*;

fn backup_root(label: &str) -> PathBuf {
    let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!(
            "open-why-backup-{label}-{}-{n}",
            std::process::id()
        ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn online_backup_preserves_committed_wal_content_and_evidence_identity() {
    let root = backup_root("round-trip");
    let source_path = root.join("source.db");
    let destination = root.join("restore.db");
    let store =
        Store::open_with_store_instance_id(&source_path, "provider:backup-round-trip").unwrap();
    store
        .conn
        .pragma_update(None, "journal_mode", "WAL")
        .unwrap();
    store
        .conn
        .pragma_update(None, "wal_autocheckpoint", 0)
        .unwrap();
    let mut rationale = decision("Use online snapshots", "Committed WAL rationale", 0.8, None);
    rationale.source = "synthetic-backup-test".to_owned();
    rationale.author = "test-author".to_owned();
    let record_id = store.capture(&rationale, "repo-a", None).unwrap();
    let source_identity = store.store_identity().unwrap();
    let source_evidence = evidence_identity(&store, &record_id, "repo-a");
    let wal_path = PathBuf::from(format!("{}-wal", source_path.display()));
    assert!(
        std::fs::metadata(&wal_path).unwrap().len() > 32,
        "test record must be committed while WAL state is present"
    );

    store.backup_to(&destination).unwrap();
    assert_eq!(store.store_identity().unwrap(), source_identity);
    let restored = Store::open(&destination).unwrap();
    assert_eq!(restored.store_identity().unwrap(), source_identity);
    match restored
        .get_current_evidence_in_scope(&record_id, "repo-a")
        .unwrap()
    {
        ScopedCurrentRecordResolution::Ok {
            record,
            evidence_identity,
            ..
        } => {
            assert_eq!(record.title, rationale.subject);
            assert_eq!(record.content, rationale.body);
            assert_eq!(evidence_identity, source_evidence);
        }
        other => panic!("restored record was unavailable: {other:?}"),
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&destination)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    drop(restored);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn backup_refuses_existing_destination_without_mutating_it() {
    let store = temp_store();
    let root = backup_root("existing");
    let destination = root.join("existing.db");
    let sentinel = b"existing destination bytes";
    std::fs::write(&destination, sentinel).unwrap();

    let error = store.backup_to(&destination).unwrap_err();
    assert!(format!("{error:#}").contains("store path already exists"));
    assert_eq!(std::fs::read(&destination).unwrap(), sentinel);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_backup_removes_its_partial_destination() {
    let store = temp_store();
    let root = backup_root("failure");
    let destination = root.join("partial.db");

    let error = store
        .backup_to_with(&destination, |_, target| {
            target.execute_batch("CREATE TABLE partial(value TEXT);")?;
            anyhow::bail!("injected backup failure")
        })
        .unwrap_err();
    assert!(error.to_string().contains("injected backup failure"));
    assert!(!destination.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(unix, any(target_vendor = "apple", target_os = "linux")))]
#[test]
fn backup_rejects_symlink_leaf_and_parent_escapes() {
    use std::os::unix::fs::symlink;

    let store = temp_store();
    let root = backup_root("symlinks");
    let outside = root.join("outside");
    std::fs::create_dir(&outside).unwrap();
    let outside_file = outside.join("outside.db");
    std::fs::write(&outside_file, b"outside sentinel").unwrap();

    let leaf = root.join("leaf.db");
    symlink(&outside_file, &leaf).unwrap();
    assert!(store.backup_to(&leaf).is_err());
    assert_eq!(std::fs::read(&outside_file).unwrap(), b"outside sentinel");

    let parent = root.join("linked-parent");
    symlink(&outside, &parent).unwrap();
    assert!(store.backup_to(&parent.join("escaped.db")).is_err());
    assert!(!outside.join("escaped.db").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(unix, any(target_vendor = "apple", target_os = "linux")))]
#[test]
fn backup_creates_private_parent_directories() {
    use std::os::unix::fs::PermissionsExt;

    let store = temp_store();
    let root = backup_root("permissions");
    let private_parent = root.join("nested").join("backups");
    let destination = private_parent.join("snapshot.db");
    store.backup_to(&destination).unwrap();
    assert_eq!(
        std::fs::metadata(root.join("nested"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&private_parent)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    std::fs::remove_dir_all(root).unwrap();
}
