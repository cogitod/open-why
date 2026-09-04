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

#[cfg(unix)]
use std::os::unix::fs::{symlink, PermissionsExt};

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
    let path = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("open-why-{label}-{}-{serial}", std::process::id()));
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

fn assert_canonical_utc(value: &str) {
    let bytes = value.as_bytes();
    assert_eq!(bytes.len(), 20, "expected second-resolution UTC: {value}");
    assert_eq!(bytes[4], b'-');
    assert_eq!(bytes[7], b'-');
    assert_eq!(bytes[10], b'T');
    assert_eq!(bytes[13], b':');
    assert_eq!(bytes[16], b':');
    assert_eq!(bytes[19], b'Z');
    assert!(bytes
        .iter()
        .enumerate()
        .filter(|(index, _)| ![4, 7, 10, 13, 16, 19].contains(index))
        .all(|(_, byte)| byte.is_ascii_digit()));
}

#[path = "store_identity/evidence_import.rs"]
mod evidence_import;
#[path = "store_identity/feedback.rs"]
mod feedback;
#[path = "store_identity/identity_inspection.rs"]
mod identity_inspection;
#[path = "store_identity/path_security.rs"]
mod path_security;
#[path = "store_identity/retirement_cli.rs"]
mod retirement_cli;
#[path = "store_identity/retirement_public.rs"]
mod retirement_public;
#[path = "store_identity/scoped_current.rs"]
mod scoped_current;
