use super::*;

fn transition(predecessor_id: &str, successor_id: &str, scope: &str) -> ExternalSupersession {
    ExternalSupersession {
        predecessor_id: predecessor_id.to_owned(),
        successor_id: successor_id.to_owned(),
        scope: scope.to_owned(),
        valid_until: "2026-09-01T22:30:09.849Z".to_owned(),
    }
}

fn imported_pair(label: &str) -> (PathBuf, Store) {
    let dir = temp_dir(label);
    let path = dir.join("store.db");
    let store = Store::open_with_store_instance_id(&path, label).unwrap();
    let mut predecessor = row("predecessor", "repo-a");
    predecessor.valid_until = None;
    predecessor.superseded_by = None;
    predecessor.fact_key = None;
    let mut successor = row("successor", "repo-a");
    successor.title = "New rationale".to_owned();
    successor.content = "Replacement body".to_owned();
    successor.valid_from = Some("2026-09-01T22:30:09Z".to_owned());
    successor.valid_until = None;
    successor.fact_key = None;
    store.import_external(&[predecessor, successor]).unwrap();
    (dir, store)
}

#[test]
fn exact_external_supersession_applies_once_and_preserves_sealed_identity() {
    let (dir, store) = imported_pair("external-supersession");
    let before = identity(&store, "predecessor", "repo-a");
    let request = transition("predecessor", "successor", "repo-a");

    store.reconcile_external_supersession(&request).unwrap();
    store.reconcile_external_supersession(&request).unwrap();

    let after = identity(&store, "predecessor", "repo-a");
    assert_eq!(before.record_digest, after.record_digest);
    let record = store.get_record_any("predecessor", true).unwrap().unwrap();
    assert_eq!(record.superseded_by.as_deref(), Some("successor"));
    assert_eq!(
        record.valid_until.as_deref(),
        Some("2026-09-01T22:30:09.849Z")
    );
    match store.get_current_evidence("predecessor") {
        Ok(open_why::CurrentRecordResolution::Ok { record, .. }) => {
            assert_eq!(record.id, "successor");
        }
        other => panic!("expected successor resolution, got {other:?}"),
    }
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn external_supersession_refuses_conflicts_missing_targets_and_cycles() {
    let (dir, store) = imported_pair("external-supersession-errors");
    let missing = store
        .reconcile_external_supersession(&transition("predecessor", "missing", "repo-a"))
        .unwrap_err();
    assert!(missing
        .downcast_ref::<SupersessionTargetNotFound>()
        .is_some());
    let cross_scope = store
        .reconcile_external_supersession(&transition("predecessor", "successor", "repo-b"))
        .unwrap_err();
    assert!(cross_scope
        .downcast_ref::<SupersessionTargetNotFound>()
        .is_some());
    let cycle = store
        .reconcile_external_supersession(&transition("predecessor", "predecessor", "repo-a"))
        .unwrap_err();
    assert!(cycle.downcast_ref::<SupersessionCycle>().is_some());

    let request = transition("predecessor", "successor", "repo-a");
    store.reconcile_external_supersession(&request).unwrap();
    let mut conflicting = request;
    conflicting.valid_until = "2026-09-02T00:00:00Z".to_owned();
    let conflict = store
        .reconcile_external_supersession(&conflicting)
        .unwrap_err();
    assert!(conflict.downcast_ref::<SupersessionConflict>().is_some());
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}
