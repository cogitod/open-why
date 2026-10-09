//! A vendor-neutral host owns raw events; open-why stores only explicit recorded rationale.
use anyhow::{ensure, Context, Result};
use open_why::{ExternalDecision, ExternalSupersession, ScopedCurrentRecordResolution, Store};
use std::path::Path;

pub fn run(directory: &Path) -> Result<()> {
    // Refuse existing directories so this example cannot open a user's store.
    std::fs::create_dir(directory).context("choose a new demo directory")?;
    let identity = format!(
        "adapter-demo:{}:{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    let store = Store::open_with_store_instance_id(&directory.join("evidence.db"), &identity)?;
    let rows: Vec<ExternalDecision> = serde_json::from_value(serde_json::json!([
        {"id":"event:storage-1","kind":"decision","title":"Notebook storage",
         "content":"SQLite for offline use with one writer.","scope":"example:notebook",
         "date":"2025-01-01T00:00:00Z","source":"example-events/storage-1","author":"Example author"},
        {"id":"event:storage-2","kind":"decision","title":"Shared notebook storage",
         "content":"PostgreSQL because multiple machines now write concurrently.","scope":"example:notebook",
         "date":"2025-02-01T00:00:00Z","source":"example-events/storage-2","author":"Example author"}
    ]))?;
    ensure!(store.import_external(&rows)? == 2, "initial import count");
    ensure!(
        store.import_external(&rows)? == 0,
        "exact replay must be idempotent"
    );
    let transition = ExternalSupersession {
        predecessor_id: rows[0].id.clone(),
        successor_id: rows[1].id.clone(),
        scope: rows[0].scope.clone(),
        valid_until: "2025-02-01T00:00:00Z".into(),
    };
    store.reconcile_external_supersession(&transition)?;
    store.reconcile_external_supersession(&transition)?;
    let current = store.get_current_evidence_in_scope(&rows[0].id, &rows[0].scope)?;
    let ScopedCurrentRecordResolution::Ok {
        current_id,
        evidence_identity,
        ..
    } = current
    else {
        anyhow::bail!("current evidence not found");
    };
    ensure!(current_id == rows[1].id, "supersession not followed");
    ensure!(
        evidence_identity.store_instance_id == identity,
        "store identity changed"
    );
    ensure!(
        matches!(
            store.get_current_evidence_in_scope(&rows[0].id, "other-project")?,
            ScopedCurrentRecordResolution::Error { .. }
        ),
        "foreign scope exposed evidence"
    );
    ensure!(
        matches!(
            store.get_current_evidence_in_scope("missing-event", &rows[0].scope)?,
            ScopedCurrentRecordResolution::Error { .. }
        ),
        "absent evidence invented"
    );
    ensure!(
        !store
            .search_records("PostgreSQL", &[&rows[0].scope], &[], 10)?
            .is_empty(),
        "retrieval failed"
    );
    let snapshot = directory.join("backup.db");
    store.backup_to(&snapshot)?;
    let restored = Store::open_existing_read_only(&snapshot, Some(&identity))?;
    restored.verify_integrity()?;
    ensure!(
        restored.store_identity()? == store.store_identity()?,
        "backup identity changed"
    );
    println!(
        "Adapter demo passed: import/replay, supersession, scoped evidence, retrieval and backup."
    );
    Ok(())
}

fn main() -> Result<()> {
    let directory = std::env::args()
        .nth(1)
        .context("usage: embedded_adapter /absolute/new-demo-directory")?;
    let directory = Path::new(&directory);
    ensure!(directory.is_absolute(), "use an absolute demo directory");
    run(directory)
}
