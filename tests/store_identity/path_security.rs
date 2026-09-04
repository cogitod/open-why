use super::*;

#[cfg(unix)]
#[test]
fn new_store_paths_are_private_without_changing_existing_modes() {
    let root = temp_dir("store-permissions");
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o751)).unwrap();
    let new_parent = root.join("new").join("nested");
    let new_path = new_parent.join("store.db");
    let store = Store::open_with_store_instance_id(&new_path, "provider:new-permissions").unwrap();
    drop(store);

    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&root), 0o751);
    assert_eq!(mode(&root.join("new")), 0o700);
    assert_eq!(mode(&new_parent), 0o700);
    assert_eq!(mode(&new_path), 0o600);

    let existing_parent = root.join("existing");
    std::fs::create_dir(&existing_parent).unwrap();
    std::fs::set_permissions(&existing_parent, std::fs::Permissions::from_mode(0o750)).unwrap();
    let existing_path = existing_parent.join("store.db");
    std::fs::File::create(&existing_path).unwrap();
    std::fs::set_permissions(&existing_path, std::fs::Permissions::from_mode(0o640)).unwrap();
    let store = Store::open_with_store_instance_id(&existing_path, "provider:existing-permissions")
        .unwrap();
    drop(store);
    assert_eq!(mode(&existing_parent), 0o750);
    assert_eq!(mode(&existing_path), 0o640);

    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn fresh_store_rejects_symlinked_parent_and_leaf_without_external_effects() {
    let root = temp_dir("store-symlink-rejection");
    let outside = root.join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o751)).unwrap();
    let parent_link = root.join("parent-link");
    symlink(&outside, &parent_link).unwrap();

    let parent_result = Store::open_with_store_instance_id(
        &parent_link.join("store.db"),
        "provider:symlink-parent",
    );
    assert!(parent_result.is_err());
    assert!(!outside.join("store.db").exists());
    assert_eq!(
        std::fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
        0o751
    );

    let outside_file = outside.join("outside.db");
    std::fs::File::create(&outside_file).unwrap();
    std::fs::set_permissions(&outside_file, std::fs::Permissions::from_mode(0o640)).unwrap();
    let leaf_link = root.join("leaf.db");
    symlink(&outside_file, &leaf_link).unwrap();
    let leaf_result = Store::open_with_store_instance_id(&leaf_link, "provider:symlink-leaf");
    assert!(leaf_result.is_err());
    assert_eq!(
        std::fs::metadata(&outside_file)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o640
    );
    assert_eq!(std::fs::metadata(&outside_file).unwrap().len(), 0);

    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn existing_store_rejects_symlinked_parent_without_external_effects() {
    let root = temp_dir("existing-store-symlink-rejection");
    let outside = root.join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o751)).unwrap();
    let outside_path = outside.join("store.db");
    let provider = "provider:existing-symlink-parent";
    let store = Store::open_with_store_instance_id(&outside_path, provider).unwrap();
    store
        .capture(
            &capture_decision("Existing outside rationale"),
            "scope-a",
            None,
        )
        .unwrap();
    drop(store);
    std::fs::set_permissions(&outside_path, std::fs::Permissions::from_mode(0o640)).unwrap();

    let observer = Connection::open(&outside_path).unwrap();
    let before_version: i64 = observer
        .query_row("PRAGMA data_version", [], |record| record.get(0))
        .unwrap();
    let before_bytes = std::fs::read(&outside_path).unwrap();
    let link = root.join("link");
    symlink(&outside, &link).unwrap();

    let result = Store::open_with_store_instance_id(&link.join("store.db"), provider);
    assert!(result.is_err());
    assert!(inspect_store(&link.join("store.db")).is_err());
    let after_version: i64 = observer
        .query_row("PRAGMA data_version", [], |record| record.get(0))
        .unwrap();
    assert_eq!(after_version, before_version);
    assert_eq!(std::fs::read(&outside_path).unwrap(), before_bytes);
    assert_eq!(
        std::fs::metadata(&outside_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o640
    );
    let record_count: i64 = observer
        .query_row("SELECT count(*) FROM decisions", [], |record| record.get(0))
        .unwrap();
    assert_eq!(record_count, 1);

    drop(observer);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn real_cli_rejects_existing_store_through_symlinked_parent_without_write() {
    let root = temp_dir("existing-store-symlink-cli");
    let outside = root.join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o751)).unwrap();
    let outside_path = outside.join("store.db");
    let provider = "provider:existing-symlink-cli";
    let store = Store::open_with_store_instance_id(&outside_path, provider).unwrap();
    store
        .capture(
            &capture_decision("Existing CLI outside rationale"),
            "scope-a",
            None,
        )
        .unwrap();
    drop(store);
    std::fs::set_permissions(&outside_path, std::fs::Permissions::from_mode(0o640)).unwrap();

    let observer = Connection::open(&outside_path).unwrap();
    let before_version: i64 = observer
        .query_row("PRAGMA data_version", [], |record| record.get(0))
        .unwrap();
    let before_bytes = std::fs::read(&outside_path).unwrap();
    let link = root.join("link");
    symlink(&outside, &link).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_why"))
        .arg("capture")
        .arg("--id")
        .arg("blocked-symlink-write")
        .arg("--title")
        .arg("Blocked symlink write")
        .arg("--content")
        .arg("This content must never reach the outside store.")
        .arg("--scope")
        .arg("scope-a")
        .env("OPEN_WHY_DB", link.join("store.db"))
        .env("OPEN_WHY_STORE_INSTANCE_ID", provider)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let after_version: i64 = observer
        .query_row("PRAGMA data_version", [], |record| record.get(0))
        .unwrap();
    assert_eq!(after_version, before_version);
    assert_eq!(std::fs::read(&outside_path).unwrap(), before_bytes);
    assert_eq!(
        std::fs::metadata(&outside_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o640
    );
    let blocked_count: i64 = observer
        .query_row(
            "SELECT count(*) FROM decisions WHERE id='blocked-symlink-write'",
            [],
            |record| record.get(0),
        )
        .unwrap();
    assert_eq!(blocked_count, 0);

    drop(observer);
    std::fs::remove_dir_all(root).unwrap();
}
