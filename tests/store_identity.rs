use open_why::{
    inspect_store, CurrentRecordErrorCode, Decision, EvidenceIdentityResolution, ExternalDecision,
    GitRef, RecordIdentityConflict, ScopedCurrentEvidenceErrorCode, ScopedCurrentRecordResolution,
    Store, StoreCompatibility, StoreCompatibilityErrorCode, StoreIdentityBindingError,
    StoreIdentityBindingErrorCode, SupersessionConflict, SupersessionCycle,
    SupersessionTargetNotFound, EVIDENCE_IDENTITY_CONTRACT, MAX_SUPERSESSION_CHAIN,
    MAX_TEMPORAL_VALUE_BYTES, RECORD_DIGEST_CONTRACT, SCOPED_CURRENT_EVIDENCE_CONTRACT,
    STORE_SCHEMA_FAMILY, STORE_SCHEMA_VERSION,
};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

type SupersessionSnapshotRow = (String, String, Option<String>, Option<String>, String);

#[derive(Debug, PartialEq, Eq)]
struct FullDecisionSnapshot {
    id: String,
    scope: String,
    title: String,
    content: String,
    superseded_by: Option<Vec<u8>>,
    valid_from: Option<String>,
    valid_until: Option<String>,
    record_digest: String,
}

#[derive(Debug, PartialEq, Eq)]
struct FullFtsSnapshot {
    rowid: i64,
    scope: String,
    title: String,
    content: String,
    tags: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct FullCaptureSnapshot {
    decisions: Vec<FullDecisionSnapshot>,
    git_refs: Vec<(String, String, String)>,
    fts: Vec<FullFtsSnapshot>,
}

fn full_capture_snapshot(path: &Path) -> FullCaptureSnapshot {
    let observer = Connection::open(path).unwrap();
    let decisions = observer
        .prepare(
            "SELECT id,scope,title,content,CAST(superseded_by AS BLOB),valid_from,valid_until,
                    record_digest_v1
             FROM decisions ORDER BY id",
        )
        .unwrap()
        .query_map([], |record| {
            Ok(FullDecisionSnapshot {
                id: record.get(0)?,
                scope: record.get(1)?,
                title: record.get(2)?,
                content: record.get(3)?,
                superseded_by: record.get(4)?,
                valid_from: record.get(5)?,
                valid_until: record.get(6)?,
                record_digest: record.get(7)?,
            })
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let git_refs = observer
        .prepare(
            "SELECT decision_id,commit_hash,commit_subject
             FROM decision_git_refs ORDER BY decision_id,commit_hash",
        )
        .unwrap()
        .query_map([], |record| {
            Ok((record.get(0)?, record.get(1)?, record.get(2)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let fts = observer
        .prepare(
            "SELECT rowid,scope,title,content,tags
             FROM decisions_fts ORDER BY rowid",
        )
        .unwrap()
        .query_map([], |record| {
            Ok(FullFtsSnapshot {
                rowid: record.get(0)?,
                scope: record.get(1)?,
                title: record.get(2)?,
                content: record.get(3)?,
                tags: record.get(4)?,
            })
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    FullCaptureSnapshot {
        decisions,
        git_refs,
        fts,
    }
}

fn capture_decision(title: &str) -> Decision {
    Decision {
        subject: title.to_owned(),
        body: format!("body for {title}"),
        kind: "decision".to_owned(),
        source: "cycle-test".to_owned(),
        importance: 0.5,
        ..Decision::default()
    }
}

fn assert_public_cycle_rejected(
    path: &Path,
    before: &FullCaptureSnapshot,
    error: anyhow::Error,
    hidden: &[&str],
) {
    assert_eq!(
        error.downcast_ref::<SupersessionCycle>(),
        Some(&SupersessionCycle)
    );
    assert_eq!(
        error.to_string(),
        "supersession_cycle: requested relation would create a cycle"
    );
    for value in hidden {
        if !value.is_empty() {
            assert!(!error.to_string().contains(value));
        }
    }
    assert_eq!(&full_capture_snapshot(path), before);
}

fn assert_cli_cycle_rejected(
    path: &Path,
    before: &FullCaptureSnapshot,
    rejected: std::process::Output,
    hidden: &[&str],
) {
    assert!(!rejected.status.success());
    let error = String::from_utf8(rejected.stderr).unwrap();
    assert!(error.contains("supersession_cycle"));
    for value in hidden {
        if !value.is_empty() {
            assert!(!error.contains(value));
        }
    }
    assert_eq!(&full_capture_snapshot(path), before);
}

fn temp_dir(label: &str) -> PathBuf {
    let serial = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("open-why-{label}-{}-{serial}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn row(id: &str, scope: &str) -> ExternalDecision {
    ExternalDecision {
        id: id.to_owned(),
        kind: "decision".to_owned(),
        title: "Keep identity local".to_owned(),
        content: "A durable store identity prevents path-based collisions.".to_owned(),
        importance: 0.75,
        source: "integration-test".to_owned(),
        author: "tester".to_owned(),
        date: "2026-09-03T12:34:56Z".to_owned(),
        updated_at: None,
        accessed_count: None,
        times_injected: None,
        effectiveness: None,
        tags: Some("[\"identity\",\"sqlite\"]".to_owned()),
        scope: scope.to_owned(),
        valid_from: Some("2026-01-01T00:00:00Z".to_owned()),
        valid_until: Some("2030-01-01T00:00:00Z".to_owned()),
        superseded_by: None,
        fact_key: Some("store-identity".to_owned()),
        git_refs: Vec::new(),
    }
}

fn identity(store: &Store, id: &str, scope: &str) -> open_why::EvidenceIdentity {
    match store.evidence_identity_in_scope(id, scope).unwrap() {
        EvidenceIdentityResolution::Ok { identity } => identity,
        other => panic!("expected evidence identity, got {other:?}"),
    }
}

fn create_legacy(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE decisions (
            id TEXT PRIMARY KEY, kind TEXT NOT NULL, title TEXT NOT NULL, content TEXT NOT NULL,
            importance REAL NOT NULL DEFAULT 0.5, source TEXT NOT NULL DEFAULT '',
            author TEXT NOT NULL DEFAULT '', commit_sha TEXT NOT NULL DEFAULT '',
            date TEXT NOT NULL DEFAULT '', scope TEXT NOT NULL DEFAULT 'global', superseded_by TEXT,
            valid_from TEXT, valid_until TEXT, fact_key TEXT, embedding TEXT,
            content_digest TEXT NOT NULL, source_identity TEXT NOT NULL,
            created_epoch INTEGER NOT NULL DEFAULT 0, updated_at TEXT,
            accessed_count INTEGER NOT NULL DEFAULT 0, times_injected INTEGER NOT NULL DEFAULT 0,
            effectiveness REAL NOT NULL DEFAULT 0.5, tags TEXT,
            times_helpful INTEGER NOT NULL DEFAULT 0
         );
         CREATE UNIQUE INDEX idx_decisions_identity
           ON decisions(source_identity, content_digest);
         CREATE INDEX idx_decisions_scope ON decisions(scope);
         CREATE TABLE decision_git_refs (
            decision_id TEXT NOT NULL, commit_hash TEXT NOT NULL, commit_subject TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (decision_id, commit_hash)
         );
         CREATE INDEX idx_decision_git_refs_commit_hash_decision
           ON decision_git_refs(commit_hash, decision_id);
         CREATE TABLE feedback_log (
            id TEXT PRIMARY KEY, memory_id TEXT NOT NULL, helpful INTEGER NOT NULL,
            delta REAL NOT NULL, created_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE INDEX idx_feedback_log_memory ON feedback_log(memory_id);
         CREATE VIRTUAL TABLE decisions_fts USING fts5(
            scope, title, content, tags, content=decisions, content_rowid=rowid
         );
         CREATE TRIGGER decisions_fts_ai AFTER INSERT ON decisions BEGIN
           INSERT INTO decisions_fts(rowid, scope, title, content, tags)
           VALUES (new.rowid, new.scope, new.title, new.content, new.tags);
         END;
         CREATE TRIGGER decisions_fts_ad AFTER DELETE ON decisions BEGIN
           INSERT INTO decisions_fts(decisions_fts, rowid, scope, title, content, tags)
           VALUES ('delete', old.rowid, old.scope, old.title, old.content, old.tags);
         END;
         CREATE TRIGGER decisions_fts_au AFTER UPDATE ON decisions BEGIN
           INSERT INTO decisions_fts(decisions_fts, rowid, scope, title, content, tags)
           VALUES ('delete', old.rowid, old.scope, old.title, old.content, old.tags);
           INSERT INTO decisions_fts(rowid, scope, title, content, tags)
           VALUES (new.rowid, new.scope, new.title, new.content, new.tags);
         END;
         INSERT INTO decisions
           (id,kind,title,content,importance,source,author,commit_sha,date,scope,
            valid_until,content_digest,source_identity,created_epoch)
         VALUES ('legacy','decision','Legacy','body',0.5,'import','author','',
                 '2025-01-01','repo-a','2027-01-01','old','legacy',1);",
    )
    .unwrap();
}

fn copy_store(source: &Path, label: &str) -> (PathBuf, PathBuf) {
    let dir = temp_dir(label);
    let path = dir.join("store.db");
    std::fs::copy(source, &path).unwrap();
    (dir, path)
}

#[derive(Debug, PartialEq, Eq)]
struct FileState {
    bytes: Vec<u8>,
    len: u64,
    modified: std::time::SystemTime,
}

fn file_state(path: &Path) -> Option<FileState> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(FileState {
        bytes: std::fs::read(path).unwrap(),
        len: metadata.len(),
        modified: metadata.modified().unwrap(),
    })
}

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
    let root = std::env::temp_dir().join(format!(
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
    let missing_root = std::env::temp_dir().join(format!(
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

#[test]
fn evidence_identity_is_sealed_and_stable_across_relations_and_lifecycle() {
    let dir = temp_dir("evidence-stability");
    let path = dir.join("store.db");
    let store = Store::open_with_store_instance_id(&path, "provider:evidence").unwrap();
    let mut original = row("stable", "repo-a");
    store
        .import_external_sealed(std::slice::from_ref(&original))
        .unwrap();
    let before = identity(&store, "stable", "repo-a");
    assert_eq!(before.contract, EVIDENCE_IDENTITY_CONTRACT);
    assert_eq!(before.record_digest_contract, RECORD_DIGEST_CONTRACT);

    original.tags = Some("[\"sqlite\",\"identity\"]".to_owned());
    store
        .import_external_sealed(std::slice::from_ref(&original))
        .unwrap();
    store.link_git("stable", "abc123", "Add identity").unwrap();
    store.feedback("stable", true).unwrap();
    let mutable = Connection::open(&path).unwrap();
    mutable
        .execute(
            "UPDATE decisions
             SET accessed_count=42,times_injected=7,effectiveness=0.9,embedding='[0.1]'
             WHERE id='stable'",
            [],
        )
        .unwrap();
    drop(mutable);
    assert_eq!(identity(&store, "stable", "repo-a"), before);

    let replacement = Decision {
        subject: "Replacement rationale".to_owned(),
        body: "A newer rationale supersedes the old record.".to_owned(),
        source: "capture".to_owned(),
        kind: "decision".to_owned(),
        importance: 0.8,
        ..Decision::default()
    };
    store
        .capture_external(
            &replacement,
            "repo-a",
            "replacement",
            Some("2026-01-02T00:00:00Z"),
            None,
            Some("stable"),
        )
        .unwrap();
    assert_eq!(identity(&store, "stable", "repo-a"), before);

    let ScopedCurrentRecordResolution::Ok {
        contract,
        current_id,
        evidence_identity,
        ..
    } = store
        .get_current_evidence_in_scope("stable", "repo-a")
        .unwrap()
    else {
        panic!("expected scoped current evidence");
    };
    assert_eq!(contract, SCOPED_CURRENT_EVIDENCE_CONTRACT);
    assert_eq!(current_id, "replacement");
    assert_eq!(evidence_identity.record_id, "replacement");

    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn conflicting_reimport_and_direct_update_leave_every_effect_unchanged() {
    let dir = temp_dir("immutable-conflict");
    let path = dir.join("store.db");
    let store = Store::open_with_store_instance_id(&path, "provider:conflict").unwrap();
    let original = row("sealed", "repo-a");
    store
        .import_external_sealed(std::slice::from_ref(&original))
        .unwrap();
    store.link_git("sealed", "abc123", "Original link").unwrap();
    let observer = Connection::open(&path).unwrap();
    let snapshot = || {
        let record: (String, String, String) = observer
            .query_row(
                "SELECT title,content,record_digest_v1 FROM decisions WHERE id='sealed'",
                [],
                |record| Ok((record.get(0)?, record.get(1)?, record.get(2)?)),
            )
            .unwrap();
        let fts: i64 = observer
            .query_row("SELECT count(*) FROM decisions_fts", [], |record| {
                record.get(0)
            })
            .unwrap();
        let links: i64 = observer
            .query_row(
                "SELECT count(*) FROM decision_git_refs WHERE decision_id='sealed'",
                [],
                |record| record.get(0),
            )
            .unwrap();
        let data_version: i64 = observer
            .pragma_query_value(None, "data_version", |record| record.get(0))
            .unwrap();
        (record, fts, links, data_version)
    };
    let before = snapshot();
    let mut conflict = original;
    conflict.content = "changed immutable content".to_owned();
    conflict.git_refs.push(GitRef {
        commit_hash: "must-not-land".to_owned(),
        commit_subject: "Must not land".to_owned(),
    });
    let error = store.import_external_sealed(&[conflict]).unwrap_err();
    assert!(error.downcast_ref::<RecordIdentityConflict>().is_some());
    assert_eq!(snapshot(), before);

    for assignment in [
        "id='sealed-2'",
        "scope='repo-b'",
        "kind='fact'",
        "title='direct mutation'",
        "content='direct mutation'",
        "importance=0.2",
        "source='direct mutation'",
        "author='direct mutation'",
        "commit_sha='direct-mutation'",
        "date='2027-01-01T00:00:00Z'",
        "tags='[\"changed\"]'",
        "fact_key='changed'",
        "valid_from='2027-01-01T00:00:00Z'",
        "declared_valid_until='2028-01-01T00:00:00Z'",
        "record_digest_v1='0000000000000000000000000000000000000000000000000000000000000000'",
    ] {
        let direct = observer.execute(
            &format!("UPDATE decisions SET {assignment} WHERE id='sealed'"),
            [],
        );
        assert!(direct
            .unwrap_err()
            .to_string()
            .contains("identity_conflict"));
        assert_eq!(snapshot(), before, "mutation escaped guard: {assignment}");
    }
    for destructive in [
        "DELETE FROM decisions WHERE id='sealed'",
        "INSERT OR REPLACE INTO decisions SELECT * FROM decisions WHERE id='sealed'",
    ] {
        let result = observer.execute(destructive, []);
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("identity_conflict"));
        assert_eq!(snapshot(), before, "destructive write escaped guard");
    }
    drop(observer);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn public_store_import_replays_exactly_and_rejects_a_batch_before_any_effect() {
    let dir = temp_dir("public-import");
    let path = dir.join("store.db");
    let store = Store::open_with_store_instance_id(&path, "provider:public-import").unwrap();
    let mut original = row("public-sealed", "repo-a");
    original.git_refs.push(GitRef {
        commit_hash: "original-commit".to_owned(),
        commit_subject: "Original relation".to_owned(),
    });
    assert_eq!(
        store
            .import_external(std::slice::from_ref(&original))
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .import_external(std::slice::from_ref(&original))
            .unwrap(),
        0
    );

    let mut new_row = row("must-not-insert", "repo-a");
    new_row.git_refs.push(GitRef {
        commit_hash: "must-not-insert-ref".to_owned(),
        commit_subject: "Must not insert".to_owned(),
    });
    let mut conflict = original.clone();
    conflict.content = "changed immutable body".to_owned();
    conflict.git_refs.push(GitRef {
        commit_hash: "must-not-append".to_owned(),
        commit_subject: "Must not append".to_owned(),
    });
    let error = store.import_external(&[new_row, conflict]).unwrap_err();
    assert!(error.downcast_ref::<RecordIdentityConflict>().is_some());

    let observer = Connection::open(&path).unwrap();
    let records: i64 = observer
        .query_row("SELECT count(*) FROM decisions", [], |record| record.get(0))
        .unwrap();
    let original_body: String = observer
        .query_row(
            "SELECT content FROM decisions WHERE id='public-sealed'",
            [],
            |record| record.get(0),
        )
        .unwrap();
    let refs: Vec<String> = observer
        .prepare(
            "SELECT commit_hash FROM decision_git_refs
             WHERE decision_id='public-sealed' ORDER BY commit_hash",
        )
        .unwrap()
        .query_map([], |record| record.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(records, 1);
    assert_eq!(original_body, original.content);
    assert_eq!(refs, ["original-commit"]);
    drop(observer);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn real_cli_import_reports_created_replay_and_conflict_truthfully() {
    let dir = temp_dir("cli-import");
    let db_path = dir.join("store.db");
    let input_path = dir.join("rows.json");
    let provider = "provider:cli-import";
    let mut original = row("cli-sealed", "repo-a");
    original.git_refs.push(GitRef {
        commit_hash: "cli-original-commit".to_owned(),
        commit_subject: "CLI original relation".to_owned(),
    });
    std::fs::write(&input_path, serde_json::to_vec(&[&original]).unwrap()).unwrap();

    let run_import = || {
        Command::new(env!("CARGO_BIN_EXE_why"))
            .arg("import")
            .arg("--file")
            .arg(&input_path)
            .env("OPEN_WHY_DB", &db_path)
            .env("OPEN_WHY_STORE_INSTANCE_ID", provider)
            .output()
            .unwrap()
    };
    let created = run_import();
    assert!(created.status.success());
    assert_eq!(
        String::from_utf8(created.stdout).unwrap(),
        "imported 1 decisions\n"
    );
    let replayed = run_import();
    assert!(replayed.status.success());
    assert_eq!(
        String::from_utf8(replayed.stdout).unwrap(),
        "imported 0 decisions\n"
    );

    let mut new_row = row("cli-must-not-insert", "repo-a");
    new_row.git_refs.push(GitRef {
        commit_hash: "cli-must-not-insert-ref".to_owned(),
        commit_subject: "Must not insert".to_owned(),
    });
    let mut conflict = original.clone();
    conflict.content = "changed CLI immutable body".to_owned();
    conflict.git_refs.push(GitRef {
        commit_hash: "cli-must-not-append".to_owned(),
        commit_subject: "Must not append".to_owned(),
    });
    std::fs::write(
        &input_path,
        serde_json::to_vec(&[new_row, conflict]).unwrap(),
    )
    .unwrap();
    let rejected = run_import();
    assert!(!rejected.status.success());
    assert!(String::from_utf8(rejected.stderr)
        .unwrap()
        .contains("identity_conflict"));

    let observer = Connection::open(&db_path).unwrap();
    let records: i64 = observer
        .query_row("SELECT count(*) FROM decisions", [], |record| record.get(0))
        .unwrap();
    let original_body: String = observer
        .query_row(
            "SELECT content FROM decisions WHERE id='cli-sealed'",
            [],
            |record| record.get(0),
        )
        .unwrap();
    let refs: Vec<String> = observer
        .prepare(
            "SELECT commit_hash FROM decision_git_refs
             WHERE decision_id='cli-sealed' ORDER BY commit_hash",
        )
        .unwrap()
        .query_map([], |record| record.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(records, 1);
    assert_eq!(original_body, original.content);
    assert_eq!(refs, ["cli-original-commit"]);
    drop(observer);
    std::fs::remove_dir_all(dir).unwrap();
}

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
