use super::*;

#[test]
fn scoped_current_and_identity_hide_scope_oracles_and_foreign_successors() {
    let foreign_dir = temp_dir("scope-foreign");
    let foreign_path = foreign_dir.join("store.db");
    let foreign = Store::open_with_store_instance_id(&foreign_path, "provider:foreign").unwrap();
    foreign
        .import_external(&[row("same-id", "repo-b")])
        .unwrap();
    let absent_dir = temp_dir("scope-absent");
    let absent_path = absent_dir.join("store.db");
    let absent = Store::open_with_store_instance_id(&absent_path, "provider:absent").unwrap();

    let wrong = foreign
        .evidence_identity_in_scope("same-id", "repo-a")
        .unwrap();
    let missing = absent
        .evidence_identity_in_scope("same-id", "repo-a")
        .unwrap();
    assert_eq!(
        serde_json::to_value(wrong).unwrap(),
        serde_json::to_value(missing).unwrap()
    );

    let mut root = row("root", "repo-a");
    root.superseded_by = Some("foreign-successor".to_owned());
    root.valid_until = Some("2026-02-01T00:00:00Z".to_owned());
    foreign.import_external(&[root]).unwrap();
    foreign
        .import_external(&[row("foreign-successor", "repo-b")])
        .unwrap();
    let resolution = foreign
        .get_current_evidence_in_scope("root", "repo-a")
        .unwrap();
    let ScopedCurrentRecordResolution::Error {
        contract,
        code,
        message,
        ..
    } = resolution
    else {
        panic!("foreign successor must fail closed");
    };
    assert_eq!(contract, SCOPED_CURRENT_EVIDENCE_CONTRACT);
    assert_eq!(code, ScopedCurrentEvidenceErrorCode::BrokenChain);
    assert_eq!(
        message,
        "supersession chain is unavailable in the requested scope"
    );
    assert!(!message.contains("foreign-successor"));

    drop(foreign);
    drop(absent);
    std::fs::remove_dir_all(foreign_dir).unwrap();
    std::fs::remove_dir_all(absent_dir).unwrap();
}

#[test]
fn scoped_current_uses_its_own_identity_conflict_contract_only() {
    let dir = temp_dir("scoped-contract");
    let path = dir.join("store.db");
    let store = Store::open_with_store_instance_id(&path, "provider:scoped-contract").unwrap();
    store
        .import_external_sealed(&[row("sealed-current", "repo-a")])
        .unwrap();
    let corrupt = Connection::open(&path).unwrap();
    let guard: String = corrupt
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type='trigger' AND name='decisions_identity_update_guard'",
            [],
            |record| record.get(0),
        )
        .unwrap();
    corrupt
        .execute_batch("DROP TRIGGER decisions_identity_update_guard;")
        .unwrap();
    corrupt
        .execute(
            "UPDATE decisions SET record_digest_v1=?1 WHERE id='sealed-current'",
            ["0000000000000000000000000000000000000000000000000000000000000000"],
        )
        .unwrap();
    corrupt.execute_batch(&guard).unwrap();
    drop(corrupt);

    assert!(matches!(
        store.get_current_evidence("sealed-current").unwrap(),
        open_why::CurrentRecordResolution::Ok { .. }
    ));
    let ScopedCurrentRecordResolution::Error {
        contract,
        code,
        message,
        ..
    } = store
        .get_current_evidence_in_scope("sealed-current", "repo-a")
        .unwrap()
    else {
        panic!("scoped identity mismatch must fail closed");
    };
    assert_eq!(contract, SCOPED_CURRENT_EVIDENCE_CONTRACT);
    assert_eq!(code, ScopedCurrentEvidenceErrorCode::IdentityConflict);
    assert_eq!(
        message,
        "current record identity conflicts with its sealed evidence"
    );
    assert!(!message.contains("provider:scoped-contract"));

    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}
