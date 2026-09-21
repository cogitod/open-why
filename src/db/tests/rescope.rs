use super::support::{history_row, temp_store};
use crate::db::digest::record_digest_v1;
use crate::Store;

/// Every record the caller moves must come out of the move still verifiable: scope changed, and
/// the identity digest recomputed to match, so nothing downstream sees a record whose seal has
/// stopped describing it.
#[test]
fn rescope_moves_records_and_reseals_their_identity() {
    let store = temp_store();
    store
        .import_external(&[
            history_row("a", None, "1", "first body sentinel"),
            history_row("b", None, "1", "second body sentinel"),
        ])
        .unwrap();

    let moved = store.rescope("1", "/repos/alpha").unwrap();

    assert_eq!(moved, 2);
    assert_eq!(store.count_for_scope("/repos/alpha").unwrap(), 2);
    assert_eq!(store.count_for_scope("1").unwrap(), 0);
    for id in ["a", "b"] {
        assert!(store.record_belongs_to_scope(id, "/repos/alpha").unwrap());
        let row = Store::record_digest_row_in_scope_on(&store.conn, id, "/repos/alpha")
            .unwrap()
            .expect("record is in its new scope");
        let stored: String = store
            .conn
            .query_row(
                "SELECT record_digest_v1 FROM decisions WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            stored,
            record_digest_v1(&row).unwrap(),
            "record {id} must still verify against its own fields"
        );
    }
}

/// The guard is dropped only for the duration of the move. If it did not come back, every later
/// scope edit would go unnoticed, which is the failure the guard exists to prevent.
#[test]
fn rescope_restores_the_identity_guard() {
    let store = temp_store();
    store
        .import_external(&[history_row("a", None, "1", "body sentinel")])
        .unwrap();
    store.rescope("1", "/repos/alpha").unwrap();

    let error = store
        .conn
        .execute("UPDATE decisions SET scope='/repos/beta' WHERE id='a'", [])
        .unwrap_err();

    assert!(
        error.to_string().contains("identity_conflict"),
        "the guard must be back after a rescope, got: {error}"
    );
}

/// Repeated runs have to be safe: the second one finds nothing and writes nothing.
#[test]
fn rescope_is_idempotent_and_ignores_an_empty_source() {
    let store = temp_store();
    store
        .import_external(&[history_row("a", None, "1", "body sentinel")])
        .unwrap();

    assert_eq!(store.rescope("1", "/repos/alpha").unwrap(), 1);
    assert_eq!(store.rescope("1", "/repos/alpha").unwrap(), 0);
    assert_eq!(store.rescope("never-used", "/repos/alpha").unwrap(), 0);
    assert_eq!(store.count_for_scope("/repos/alpha").unwrap(), 1);
}

/// A scope moved onto itself changes nothing, and an empty scope is a caller mistake rather than
/// a silent no-op, because it would otherwise look like a successful move of zero records.
#[test]
fn rescope_refuses_an_empty_scope_and_no_ops_on_itself() {
    let store = temp_store();
    store
        .import_external(&[history_row("a", None, "1", "body sentinel")])
        .unwrap();

    assert_eq!(store.rescope("1", "1").unwrap(), 0);
    assert!(store.rescope("", "/repos/alpha").is_err());
    assert!(store.rescope("1", "").is_err());
    assert_eq!(store.count_for_scope("1").unwrap(), 1);
}

/// A moved record must be reachable by the search path the move exists to restore.
#[test]
fn a_moved_record_is_searchable_under_its_new_scope() {
    let store = temp_store();
    store
        .import_external(&[history_row("a", None, "1", "distinctive rescope sentinel")])
        .unwrap();

    store.rescope("1", "/repos/alpha").unwrap();

    let found = store
        .search_records("distinctive rescope sentinel", &["/repos/alpha"], &[], 5)
        .unwrap();
    assert_eq!(found.len(), 1, "the lexical index must follow the move");
    assert_eq!(found[0].id, "a");
}
