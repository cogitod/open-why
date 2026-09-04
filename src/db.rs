use crate::embed::Embedder;
use crate::store::{
    CommitLinkItem, CommitLinksErrorCode, CommitLinksResolution, CurrentRecordErrorCode,
    CurrentRecordResolution, Decision, EvidenceIdentity, EvidenceIdentityErrorCode,
    EvidenceIdentityResolution, ExternalDecision, GitRef, RationaleHistoryErrorCode,
    RationaleHistoryRecord, RationaleHistoryResolution, Record, RecordIdentityConflict,
    ScopedCommitLinkErrorCode, ScopedCommitLinkOutcome, ScopedCommitLinkResolution,
    ScopedCurrentEvidenceErrorCode, ScopedCurrentRecordResolution, StoreCompatibility,
    StoreCompatibilityErrorCode, StoreIdentity, StoreIdentityBindingError,
    StoreIdentityBindingErrorCode, SupersessionConflict, SupersessionCycle,
    SupersessionTargetNotFound, COMMIT_LINKS_CONTRACT, CURRENT_RATIONALE_CONTRACT,
    EVIDENCE_IDENTITY_CONTRACT, MAX_COMMIT_LINKS_PAGE_RECORDS, MAX_COMMIT_LINKS_PAGE_SOURCE_BYTES,
    MAX_COMMIT_LINK_HASH_BYTES, MAX_COMMIT_LINK_RECORD_ID_BYTES, MAX_COMMIT_LINK_SCOPE_BYTES,
    MAX_COMMIT_LINK_SUBJECT_BYTES, MAX_HISTORY_PAGE_GIT_REFS, MAX_HISTORY_PAGE_RECORDS,
    MAX_HISTORY_PAGE_SOURCE_BYTES, MAX_STORE_INSTANCE_ID_BYTES, MAX_SUPERSESSION_CHAIN,
    MAX_TEMPORAL_VALUE_BYTES, RATIONALE_HISTORY_CONTRACT, RECORD_DIGEST_CONTRACT,
    SCOPED_COMMIT_LINK_WRITE_CONTRACT, SCOPED_CURRENT_EVIDENCE_CONTRACT, STORE_SCHEMA_FAMILY,
    STORE_SCHEMA_VERSION,
};
use anyhow::{Context, Result};
use rusqlite::{
    params, Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

mod ranking;
use ranking::{rank, rank_by, RankRow};

const CORE_SCHEMA_V1_SQL: &str = "CREATE TABLE IF NOT EXISTS decisions (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    importance REAL NOT NULL DEFAULT 0.5,
    source TEXT NOT NULL DEFAULT '',
    author TEXT NOT NULL DEFAULT '',
    commit_sha TEXT NOT NULL DEFAULT '',
    date TEXT NOT NULL DEFAULT '',
    scope TEXT NOT NULL DEFAULT 'global',
    superseded_by TEXT,
    valid_from TEXT,
    valid_until TEXT,
    fact_key TEXT,
    embedding TEXT,
    content_digest TEXT NOT NULL,
    source_identity TEXT NOT NULL,
    created_epoch INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT,
    accessed_count INTEGER NOT NULL DEFAULT 0,
    times_injected INTEGER NOT NULL DEFAULT 0,
    effectiveness REAL NOT NULL DEFAULT 0.5,
    tags TEXT,
    times_helpful INTEGER NOT NULL DEFAULT 0,
    declared_valid_until TEXT,
    record_digest_v1 TEXT
 );
 CREATE UNIQUE INDEX IF NOT EXISTS idx_decisions_identity
   ON decisions(source_identity, content_digest);
 CREATE INDEX IF NOT EXISTS idx_decisions_scope ON decisions(scope);
 CREATE TABLE IF NOT EXISTS decision_git_refs (
    decision_id TEXT NOT NULL,
    commit_hash TEXT NOT NULL,
    commit_subject TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (decision_id, commit_hash)
 );
 CREATE INDEX IF NOT EXISTS idx_decision_git_refs_commit_hash_decision
   ON decision_git_refs(commit_hash, decision_id);";

const FEEDBACK_SCHEMA_V1_SQL: &str = "CREATE TABLE IF NOT EXISTS feedback_log (
    id TEXT PRIMARY KEY,
    memory_id TEXT NOT NULL,
    helpful INTEGER NOT NULL,
    delta REAL NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
 );
 CREATE INDEX IF NOT EXISTS idx_feedback_log_memory ON feedback_log(memory_id);";

const FTS_SCHEMA_V1_SQL: &str = "CREATE VIRTUAL TABLE IF NOT EXISTS decisions_fts USING fts5(
    scope, title, content, tags,
    content=decisions, content_rowid=rowid
 );";

const FTS_TRIGGERS_V1_SQL: &str = "CREATE TRIGGER IF NOT EXISTS decisions_fts_ai
 AFTER INSERT ON decisions BEGIN
   INSERT INTO decisions_fts(rowid, scope, title, content, tags)
   VALUES (new.rowid, new.scope, new.title, new.content, new.tags);
 END;
 CREATE TRIGGER IF NOT EXISTS decisions_fts_ad AFTER DELETE ON decisions BEGIN
   INSERT INTO decisions_fts(decisions_fts, rowid, scope, title, content, tags)
   VALUES ('delete', old.rowid, old.scope, old.title, old.content, old.tags);
 END;
 CREATE TRIGGER IF NOT EXISTS decisions_fts_au AFTER UPDATE ON decisions BEGIN
   INSERT INTO decisions_fts(decisions_fts, rowid, scope, title, content, tags)
   VALUES ('delete', old.rowid, old.scope, old.title, old.content, old.tags);
   INSERT INTO decisions_fts(rowid, scope, title, content, tags)
   VALUES (new.rowid, new.scope, new.title, new.content, new.tags);
 END;";

const IDENTITY_SCHEMA_V1_SQL: &str = "CREATE TABLE IF NOT EXISTS open_why_migrations (
    sequence INTEGER PRIMARY KEY,
    migration_id TEXT NOT NULL UNIQUE,
    checksum_sha256 TEXT NOT NULL,
    applied_at TEXT NOT NULL
 );
 CREATE TABLE IF NOT EXISTS open_why_metadata (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    schema_family TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    schema_sha256 TEXT NOT NULL,
    migration_plan_digest TEXT NOT NULL,
    store_instance_id TEXT NOT NULL UNIQUE
 );";

const IDENTITY_TRIGGERS_V1_SQL: &str =
    "CREATE TRIGGER IF NOT EXISTS decisions_identity_insert_guard
 BEFORE INSERT ON decisions
 WHEN EXISTS(SELECT 1 FROM decisions WHERE id=NEW.id)
   OR NEW.record_digest_v1 IS NULL
   OR length(NEW.record_digest_v1) != 64
   OR NEW.record_digest_v1 GLOB '*[^0-9a-f]*'
 BEGIN
   SELECT RAISE(ABORT, 'identity_conflict');
 END;
 CREATE TRIGGER IF NOT EXISTS decisions_identity_update_guard
 BEFORE UPDATE OF id,scope,kind,title,content,importance,source,author,commit_sha,date,tags,
                  fact_key,valid_from,declared_valid_until,record_digest_v1
 ON decisions
 WHEN NEW.id IS NOT OLD.id
   OR NEW.scope IS NOT OLD.scope
   OR NEW.kind IS NOT OLD.kind
   OR NEW.title IS NOT OLD.title
   OR NEW.content IS NOT OLD.content
   OR NEW.importance IS NOT OLD.importance
   OR NEW.source IS NOT OLD.source
   OR NEW.author IS NOT OLD.author
   OR NEW.commit_sha IS NOT OLD.commit_sha
   OR NEW.date IS NOT OLD.date
   OR NEW.tags IS NOT OLD.tags
   OR NEW.fact_key IS NOT OLD.fact_key
   OR NEW.valid_from IS NOT OLD.valid_from
   OR NEW.declared_valid_until IS NOT OLD.declared_valid_until
   OR NEW.record_digest_v1 IS NOT OLD.record_digest_v1
 BEGIN
   SELECT RAISE(ABORT, 'identity_conflict');
 END;
 CREATE TRIGGER IF NOT EXISTS decisions_identity_delete_guard
 BEFORE DELETE ON decisions BEGIN
   SELECT RAISE(ABORT, 'identity_conflict');
 END;";

const LEGACY_SCHEMA_V0_SQL: &str = "CREATE TABLE decisions (
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
 CREATE UNIQUE INDEX idx_decisions_identity ON decisions(source_identity, content_digest);
 CREATE INDEX idx_decisions_scope ON decisions(scope);
 CREATE TABLE decision_git_refs (
    decision_id TEXT NOT NULL, commit_hash TEXT NOT NULL, commit_subject TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')), PRIMARY KEY (decision_id, commit_hash)
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
 END;";

const MIGRATION_STEPS: &[(&str, &str)] = &[
    ("0001-core-store", CORE_SCHEMA_V1_SQL),
    ("0002-feedback", FEEDBACK_SCHEMA_V1_SQL),
    ("0003-search", FTS_SCHEMA_V1_SQL),
    ("0004-search-triggers", FTS_TRIGGERS_V1_SQL),
    ("0005-identity-foundation", IDENTITY_SCHEMA_V1_SQL),
    ("0006-identity-guards", IDENTITY_TRIGGERS_V1_SQL),
];

const REQUIRED_DECISION_COLUMNS: &[&str] = &[
    "accessed_count",
    "author",
    "commit_sha",
    "content",
    "content_digest",
    "created_epoch",
    "date",
    "declared_valid_until",
    "effectiveness",
    "embedding",
    "fact_key",
    "id",
    "importance",
    "kind",
    "record_digest_v1",
    "scope",
    "source",
    "source_identity",
    "superseded_by",
    "tags",
    "times_helpful",
    "times_injected",
    "title",
    "updated_at",
    "valid_from",
    "valid_until",
];

const REQUIRED_OBJECTS: &[(&str, &str)] = &[
    ("table", "decisions"),
    ("table", "decision_git_refs"),
    ("table", "feedback_log"),
    ("table", "open_why_metadata"),
    ("table", "open_why_migrations"),
    ("table", "decisions_fts"),
    ("index", "idx_decisions_identity"),
    ("index", "idx_decisions_scope"),
    ("index", "idx_decision_git_refs_commit_hash_decision"),
    ("index", "idx_feedback_log_memory"),
    ("trigger", "decisions_fts_ai"),
    ("trigger", "decisions_fts_ad"),
    ("trigger", "decisions_fts_au"),
    ("trigger", "decisions_identity_insert_guard"),
    ("trigger", "decisions_identity_update_guard"),
    ("trigger", "decisions_identity_delete_guard"),
];

pub fn default_path() -> PathBuf {
    std::env::var("OPEN_WHY_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| crate::store::cache_dir().join("open-why.db"))
}

/// The "why" core store. Owns the decision record (temporal + provenance + git linkage).
pub struct Store {
    conn: Connection,
    embedder: Option<Box<dyn Embedder>>,
    _store_parent: Option<std::fs::File>,
}

struct HistoryNode {
    id: String,
    scope: String,
    superseded_by: Option<String>,
    valid_from: Option<String>,
    valid_until: Option<String>,
}

struct CurrentNode {
    id: String,
    superseded_by: Option<String>,
    valid_from: Option<String>,
    valid_until: Option<String>,
}

struct CurrentEvidenceRead {
    resolution: CurrentRecordResolution,
    identity: Option<EvidenceIdentity>,
}

#[derive(Clone)]
struct RecordDigestRow {
    id: String,
    scope: String,
    kind: String,
    title: String,
    content: String,
    importance: f64,
    source: String,
    author: String,
    commit_sha: String,
    date: String,
    tags: Option<String>,
    fact_key: Option<String>,
    valid_from: Option<String>,
    declared_valid_until: Option<String>,
    sealed_digest: Option<String>,
}

struct HistoryPageRequest<'a> {
    id: &'a str,
    scope: &'a str,
    page_cursor: Option<&'a str>,
    limit: usize,
    as_of: i64,
    chain_cap: usize,
}

struct ExternalCaptureRequest<'a> {
    decision: &'a Decision,
    scope: &'a str,
    id: &'a str,
    valid_from: Option<&'a str>,
    fact_key: Option<&'a str>,
    supersedes: Option<&'a str>,
}

/// Inspect an existing store without creating or migrating any filesystem or
/// database state.
pub fn inspect_store(path: &Path) -> Result<StoreCompatibility> {
    let Some(prepared) = crate::private_store_path::prepare(path, false, false)? else {
        return Ok(StoreCompatibility::Missing);
    };
    let anchored_path = prepared.sqlite_path();
    if store_may_have_live_wal(anchored_path)? {
        return Ok(incompatible(
            StoreCompatibilityErrorCode::LiveWalIndeterminate,
            "store may have committed state in a live WAL and cannot be inspected without side effects",
            None,
        ));
    }
    let uri = immutable_sqlite_uri(anchored_path);
    let inspect_flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_URI;
    let inspect_flags = crate::private_store_path::sqlite_open_flags(inspect_flags);
    let conn = prepared
        .open_connection(|_| Connection::open_with_flags(uri, inspect_flags))
        .with_context(|| format!("inspect {}", path.display()))?;
    conn.pragma_update(None, "query_only", true)?;
    let tx = conn.unchecked_transaction()?;
    let compatibility = inspect_connection(&tx);
    tx.rollback()?;
    if store_may_have_live_wal(anchored_path)? {
        return Ok(incompatible(
            StoreCompatibilityErrorCode::LiveWalIndeterminate,
            "store entered WAL mode during read-only inspection",
            None,
        ));
    }
    Ok(compatibility)
}

impl Store {
    /// Open a store without an embedder (lexical-first). Kept as the explicit no-embedder entry
    /// point; every command path uses `open_default` so the semantic arm is active uniformly.
    #[allow(dead_code)]
    pub fn open(path: &Path) -> Result<Store> {
        Self::open_with_embedder_and_identity(path, None, None)
    }

    /// Open or migrate a store with an explicit provider-owned identity. The
    /// identity is required for first binding and must match on later bound opens.
    pub fn open_with_store_instance_id(path: &Path, store_instance_id: &str) -> Result<Store> {
        Self::open_with_embedder_and_identity(path, None, Some(store_instance_id))
    }

    /// Open the default store, wiring an embedder from the environment when one is configured
    /// (`OPEN_WHY_EMBED_MODEL_PATH` or `OPEN_WHY_EMBED_URL`). This is the entry point every CLI
    /// command and the MCP server use, so the semantic arm is active uniformly.
    pub fn open_default() -> Result<Store> {
        let store_instance_id = std::env::var("OPEN_WHY_STORE_INSTANCE_ID").ok();
        Self::open_with_embedder_and_identity(
            &default_path(),
            crate::embed::from_env()?,
            store_instance_id.as_deref(),
        )
    }

    pub fn open_with_embedder(path: &Path, embedder: Option<Box<dyn Embedder>>) -> Result<Store> {
        Self::open_with_embedder_and_identity(path, embedder, None)
    }

    pub fn open_with_embedder_and_store_instance_id(
        path: &Path,
        embedder: Option<Box<dyn Embedder>>,
        store_instance_id: &str,
    ) -> Result<Store> {
        Self::open_with_embedder_and_identity(path, embedder, Some(store_instance_id))
    }

    fn open_with_embedder_and_identity(
        path: &Path,
        embedder: Option<Box<dyn Embedder>>,
        store_instance_id: Option<&str>,
    ) -> Result<Store> {
        let prepared = match crate::private_store_path::prepare(path, false, true)? {
            Some(prepared) => prepared,
            None => {
                require_store_instance_id(store_instance_id)?;
                crate::private_store_path::prepare(path, true, true)?
                    .context("new store path was not prepared")?
            }
        };
        #[cfg(unix)]
        let open_flags = crate::private_store_path::sqlite_open_flags(
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_URI,
        );
        #[cfg(not(unix))]
        let open_flags = OpenFlags::default();
        let conn = prepared
            .open_connection(|sqlite_path| Connection::open_with_flags(sqlite_path, open_flags))
            .with_context(|| format!("open {}", path.display()))?;
        let store_parent = prepared.into_parent_guard();
        let store = Store {
            conn,
            embedder,
            _store_parent: store_parent,
        };
        store.migrate_with_provider_identity(store_instance_id)?;
        Ok(store)
    }

    /// Best-effort embedding of the searchable text: `title\ncontent`, then the
    /// space-joined tag array when present. Returns the JSON vector
    /// when an embedder is configured and succeeds; `None` keeps the row lexical.
    fn embed_text(&self, title: &str, content: &str, tags: Option<&str>) -> Option<String> {
        let embedder = self.embedder.as_ref()?;
        let mut text = String::new();
        let t = title.trim();
        if !t.is_empty() {
            text.push_str(t);
            text.push('\n');
        }
        text.push_str(content);
        if let Some(raw) = tags {
            if let Ok(v) = serde_json::from_str::<Vec<String>>(raw) {
                if !v.is_empty() {
                    text.push('\n');
                    text.push_str(&v.join(" "));
                }
            }
        }
        let vec = embedder.embed(&text).ok()?;
        serde_json::to_string(&vec).ok()
    }

    fn query_embedding(&self, query: &str) -> Option<Vec<f32>> {
        self.embedder.as_ref()?.embed(query).ok()
    }

    fn migrate_with_provider_identity(&self, store_instance_id: Option<&str>) -> Result<()> {
        self.migrate_with_hook(store_instance_id, |_| Ok(()))
    }

    fn migrate_with_hook(
        &self,
        store_instance_id: Option<&str>,
        before_commit: impl FnOnce(&Connection) -> Result<()>,
    ) -> Result<()> {
        match inspect_connection(&self.conn) {
            StoreCompatibility::Compatible { identity } => {
                return verify_store_binding(&identity, store_instance_id)
            }
            StoreCompatibility::Missing => anyhow::bail!("store path disappeared during migration"),
            StoreCompatibility::Uninitialized | StoreCompatibility::MigrationRequired { .. } => {}
            StoreCompatibility::Incompatible { code, message, .. } => {
                anyhow::bail!("store compatibility {code:?}: {message}")
            }
        }

        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let migration_required = match inspect_connection(&tx) {
            StoreCompatibility::Compatible { identity } => {
                verify_store_binding(&identity, store_instance_id)?;
                tx.rollback()?;
                return Ok(());
            }
            StoreCompatibility::Uninitialized => false,
            StoreCompatibility::MigrationRequired { .. } => true,
            StoreCompatibility::Missing => {
                anyhow::bail!("store path disappeared during migration")
            }
            StoreCompatibility::Incompatible { code, message, .. } => {
                anyhow::bail!("store compatibility {code:?}: {message}")
            }
        };
        let store_instance_id = require_store_instance_id(store_instance_id)?;
        if migration_required {
            Self::rebuild_legacy_v0_on(&tx)?;
        }
        Self::create_core_schema_on(&tx)?;
        Self::create_feedback_schema_on(&tx)?;
        Self::ensure_fts_on(&tx)?;
        Self::create_identity_schema_on(&tx)?;

        tx.execute(
            "UPDATE decisions
             SET declared_valid_until=valid_until
             WHERE declared_valid_until IS NULL AND valid_until IS NOT NULL",
            [],
        )?;
        Self::backfill_record_digests_on(&tx)?;
        Self::ensure_identity_triggers_on(&tx)?;

        Self::append_migration_ledger_on(&tx)?;
        let schema_sha256 = expected_schema_sha256_v1()?;
        anyhow::ensure!(
            schema_sha256_on(&tx)? == schema_sha256,
            "migrated store schema differs from the build-known v1 schema"
        );
        tx.execute(
            "INSERT INTO open_why_metadata
               (singleton,schema_family,schema_version,schema_sha256,migration_plan_digest,store_instance_id)
             VALUES (1,?1,?2,?3,?4,?5)",
            params![
                STORE_SCHEMA_FAMILY,
                STORE_SCHEMA_VERSION,
                schema_sha256,
                migration_plan_digest(),
                store_instance_id
            ],
        )?;
        tx.pragma_update(None, "user_version", STORE_SCHEMA_VERSION)?;
        before_commit(&tx)?;

        match inspect_connection(&tx) {
            StoreCompatibility::Compatible { .. } => {}
            other => anyhow::bail!("migrated store failed validation: {other:?}"),
        }
        tx.commit()?;
        Ok(())
    }

    fn rebuild_legacy_v0_on(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "DROP TRIGGER IF EXISTS decisions_fts_ai;
             DROP TRIGGER IF EXISTS decisions_fts_ad;
             DROP TRIGGER IF EXISTS decisions_fts_au;
             DROP TABLE IF EXISTS decisions_fts;
             ALTER TABLE decisions RENAME TO decisions_v0;
             DROP INDEX idx_decisions_identity;
             DROP INDEX idx_decisions_scope;",
        )?;
        Self::create_core_schema_on(conn)?;
        conn.execute_batch(
            "INSERT INTO decisions (
                rowid,id,kind,title,content,importance,source,author,commit_sha,date,scope,
                superseded_by,valid_from,valid_until,fact_key,embedding,content_digest,
                source_identity,created_epoch,updated_at,accessed_count,times_injected,
                effectiveness,tags,times_helpful,declared_valid_until,record_digest_v1
             )
             SELECT rowid,id,kind,title,content,importance,source,author,commit_sha,date,scope,
                superseded_by,valid_from,valid_until,fact_key,embedding,content_digest,
                source_identity,created_epoch,updated_at,accessed_count,times_injected,
                effectiveness,tags,times_helpful,valid_until,NULL
             FROM decisions_v0;
             DROP TABLE decisions_v0;",
        )?;
        Ok(())
    }

    fn append_migration_ledger_on(conn: &Connection) -> Result<()> {
        for (index, (migration_id, payload)) in MIGRATION_STEPS.iter().enumerate() {
            let sequence = i64::try_from(index + 1)?;
            let checksum = sha256_hex(payload.as_bytes());
            let existing: Option<(String, String)> = conn
                .query_row(
                    "SELECT migration_id,checksum_sha256 FROM open_why_migrations
                     WHERE sequence=?1",
                    params![sequence],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            match existing {
                Some((id, found)) if id == *migration_id && found == checksum => {}
                Some(_) => anyhow::bail!("migration ledger conflicts at sequence {sequence}"),
                None => {
                    conn.execute(
                        "INSERT INTO open_why_migrations
                           (sequence,migration_id,checksum_sha256,applied_at)
                         VALUES (?1,?2,?3,datetime('now'))",
                        params![sequence, migration_id, checksum],
                    )?;
                }
            }
        }
        Ok(())
    }

    fn create_core_schema_on(conn: &Connection) -> Result<()> {
        conn.execute_batch(CORE_SCHEMA_V1_SQL)?;
        Ok(())
    }

    fn create_feedback_schema_on(conn: &Connection) -> Result<()> {
        conn.execute_batch(FEEDBACK_SCHEMA_V1_SQL)?;
        Ok(())
    }

    fn create_identity_schema_on(conn: &Connection) -> Result<()> {
        conn.execute_batch(IDENTITY_SCHEMA_V1_SQL)?;
        Ok(())
    }

    /// Native FTS5 external-content lexical index with `scope`, `title`, `content`, and
    /// `tags` columns, synchronized by triggers,
    /// ranked by `bm25(decisions_fts, 0, 10, 5, 1)`: scope weight 0, title 10, content 5,
    /// tags 1. This makes the lexical arm byte-for-byte the same engine the TS side calls.
    fn ensure_fts_on(conn: &Connection) -> Result<()> {
        conn.execute_batch(FTS_SCHEMA_V1_SQL)?;
        Self::ensure_fts_triggers_on(conn)?;
        // Backfill stores created before the FTS index existed. Detect it by the inverted
        // index being empty while the content table has rows. The FTS5 external-content
        // `'rebuild'` command is unreliable against a TEXT-primary-key content table, so
        // backfill with the same explicit insert shape the triggers use.
        let idx_count: i64 =
            conn.query_row("SELECT count(*) FROM decisions_fts_idx", [], |r| r.get(0))?;
        let content_count: i64 =
            conn.query_row("SELECT count(*) FROM decisions", [], |r| r.get(0))?;
        if idx_count == 0 && content_count > 0 {
            conn.execute_batch("DROP TABLE IF EXISTS decisions_fts;")?;
            conn.execute_batch(FTS_SCHEMA_V1_SQL)?;
            Self::ensure_fts_triggers_on(conn)?;
            conn.execute_batch(
                "INSERT INTO decisions_fts(rowid, scope, title, content, tags)
                 SELECT rowid, scope, title, content, tags FROM decisions;",
            )?;
        }
        Ok(())
    }

    fn ensure_fts_triggers_on(conn: &Connection) -> Result<()> {
        conn.execute_batch(FTS_TRIGGERS_V1_SQL)?;
        Ok(())
    }

    fn ensure_identity_triggers_on(conn: &Connection) -> Result<()> {
        conn.execute_batch(IDENTITY_TRIGGERS_V1_SQL)?;
        Ok(())
    }

    fn backfill_record_digests_on(conn: &Connection) -> Result<()> {
        let rows = Self::record_digest_rows_on(conn)?;
        for row in rows {
            let sealed = record_digest_v1(&row)?;
            conn.execute(
                "UPDATE decisions SET record_digest_v1=?1 WHERE id=?2",
                params![sealed, row.id],
            )?;
        }
        Ok(())
    }

    fn record_digest_rows_on(conn: &Connection) -> Result<Vec<RecordDigestRow>> {
        let mut stmt = conn.prepare(
            "SELECT id,scope,kind,title,content,importance,source,author,commit_sha,date,tags,fact_key,
                    valid_from,declared_valid_until,record_digest_v1
             FROM decisions ORDER BY id",
        )?;
        let rows = stmt.query_map([], record_digest_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    fn record_digest_row_in_scope_on(
        conn: &Connection,
        id: &str,
        scope: &str,
    ) -> Result<Option<RecordDigestRow>> {
        Ok(conn
            .query_row(
                "SELECT id,scope,kind,title,content,importance,source,author,commit_sha,date,tags,fact_key,
                        valid_from,declared_valid_until,record_digest_v1
                 FROM decisions WHERE id=?1 AND scope=?2",
                params![id, scope],
                record_digest_row,
            )
            .optional()?)
    }

    fn record_digest_row_by_id_on(conn: &Connection, id: &str) -> Result<Option<RecordDigestRow>> {
        Ok(conn
            .query_row(
                "SELECT id,scope,kind,title,content,importance,source,author,commit_sha,date,tags,fact_key,
                        valid_from,declared_valid_until,record_digest_v1
                 FROM decisions WHERE id=?1",
                params![id],
                record_digest_row,
            )
            .optional()?)
    }

    fn evidence_identity_on(
        conn: &Connection,
        id: &str,
        scope: &str,
    ) -> Result<EvidenceIdentityResolution> {
        let fail = |code, message| EvidenceIdentityResolution::Error {
            contract: EVIDENCE_IDENTITY_CONTRACT,
            code,
            message,
            retryable: false,
        };
        let Some(row) = Self::record_digest_row_in_scope_on(conn, id, scope)? else {
            return Ok(fail(
                EvidenceIdentityErrorCode::NotFound,
                "record was not found in the requested scope".to_owned(),
            ));
        };
        let Some(sealed) = row.sealed_digest.as_deref() else {
            return Ok(fail(
                EvidenceIdentityErrorCode::IdentityConflict,
                "record identity does not match its sealed evidence".to_owned(),
            ));
        };
        let calculated = record_digest_v1(&row).ok();
        if calculated.as_deref() != Some(sealed) {
            return Ok(fail(
                EvidenceIdentityErrorCode::IdentityConflict,
                "record identity does not match its sealed evidence".to_owned(),
            ));
        }
        let store_instance_id: String = conn.query_row(
            "SELECT store_instance_id FROM open_why_metadata WHERE singleton=1",
            [],
            |record| record.get(0),
        )?;
        Ok(EvidenceIdentityResolution::Ok {
            identity: EvidenceIdentity {
                contract: EVIDENCE_IDENTITY_CONTRACT,
                record_digest_contract: RECORD_DIGEST_CONTRACT,
                store_instance_id,
                scope: row.scope,
                record_id: row.id,
                record_digest: sealed.to_owned(),
            },
        })
    }

    /// Return the persistent provider-owned store and verified schema identity.
    pub fn store_identity(&self) -> Result<StoreIdentity> {
        let tx = self.conn.unchecked_transaction()?;
        let compatibility = inspect_connection(&tx);
        tx.rollback()?;
        match compatibility {
            StoreCompatibility::Compatible { identity } => Ok(identity),
            StoreCompatibility::Incompatible { code, message, .. } => {
                anyhow::bail!("store compatibility {code:?}: {message}")
            }
            other => anyhow::bail!("open store is not schema-compatible: {other:?}"),
        }
    }

    /// Return the sealed identity for an exact record in an exact scope.
    pub fn evidence_identity_in_scope(
        &self,
        id: &str,
        scope: &str,
    ) -> Result<EvidenceIdentityResolution> {
        let tx = self.conn.unchecked_transaction()?;
        let resolution = Self::evidence_identity_on(&tx, id, scope)?;
        tx.rollback()?;
        Ok(resolution)
    }

    /// Capture one decision. Idempotent: re-capturing the same (identity, content)
    /// returns the existing id. `supersedes` retires an older decision (point-in-time).
    pub fn capture(&self, d: &Decision, scope: &str, supersedes: Option<&str>) -> Result<String> {
        let identity = format!("capture:{scope}:{}:{}", d.kind, d.subject);
        let content_digest = digest(&format!("{}\n{}", d.subject, d.body));
        let id = digest(&format!("{identity}\n{content_digest}"));
        let importance = d.importance.clamp(0.0, 1.0);
        let commit = if d.kind == "commit" {
            d.sha.clone()
        } else {
            String::new()
        };
        let now = now_epoch();
        let now_str = epoch_to_iso(now);
        let tx = self.conn.unchecked_transaction()?;
        let existing = Self::record_digest_row_by_id_on(&tx, &id)?;
        let observed_at = existing
            .as_ref()
            .map(|row| row.date.clone())
            .unwrap_or_else(|| now_str.clone());
        let candidate = RecordDigestRow {
            id: id.clone(),
            scope: scope.to_owned(),
            kind: d.kind.clone(),
            title: d.subject.clone(),
            content: d.body.clone(),
            importance,
            source: d.source.clone(),
            author: d.author.clone(),
            commit_sha: commit.clone(),
            date: observed_at.clone(),
            tags: None,
            fact_key: None,
            valid_from: None,
            declared_valid_until: None,
            sealed_digest: None,
        };
        let record_digest = record_digest_v1(&candidate)?;
        let exists = match existing {
            Some(existing) => {
                ensure_exact_record_replay(&existing, &candidate)?;
                true
            }
            None => false,
        };
        let retirement = match supersedes.filter(|sid| !sid.is_empty()) {
            Some(sid) => Some((sid, pending_retirement_time_on(&tx, sid, scope, &id, now)?)),
            None => None,
        };
        if !exists {
            tx.execute(
                "INSERT OR IGNORE INTO decisions
               (id, kind, title, content, importance, source, author, commit_sha, date, scope,
                content_digest, source_identity, created_epoch, record_digest_v1)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
                params![
                    id,
                    d.kind,
                    d.subject,
                    d.body,
                    importance,
                    d.source,
                    d.author,
                    commit,
                    observed_at,
                    scope,
                    content_digest,
                    identity,
                    now,
                    record_digest
                ],
            )?;
        }
        if let Some((sid, Some(retirement))) = retirement {
            let affected = tx.execute(
                "UPDATE decisions SET superseded_by=?1, valid_until=?2
                 WHERE id=?3 AND scope=?4 AND superseded_by IS NULL
                   AND valid_from IS ?5 AND valid_until IS ?6",
                params![
                    id,
                    retirement.retirement_at,
                    sid,
                    scope,
                    retirement.expected_valid_from,
                    retirement.expected_valid_until
                ],
            )?;
            if affected != 1 {
                return Err(SupersessionConflict.into());
            }
        }
        let stored = Self::record_digest_row_by_id_on(&tx, &id)?
            .context("capture insert did not persist its deterministic record id")?;
        ensure_exact_record_replay(&stored, &candidate)?;
        tx.commit()?;
        Ok(id)
    }

    /// Capture a decision with an externally minted stable ID and an
    /// explicit validity start. Idempotent by the external id: re-capturing the same id
    /// returns it without a duplicate. `supersedes` retires an older decision.
    /// `fact_key` and title matches retire the current same-key / same-title record
    /// using the same point-in-time supersession rule as ordinary capture.
    pub fn capture_external(
        &self,
        d: &Decision,
        scope: &str,
        id: &str,
        valid_from: Option<&str>,
        fact_key: Option<&str>,
        supersedes: Option<&str>,
    ) -> Result<String> {
        self.capture_external_with_pre_retirement_hook(
            ExternalCaptureRequest {
                decision: d,
                scope,
                id,
                valid_from,
                fact_key,
                supersedes,
            },
            |_| Ok(()),
        )
    }

    fn capture_external_with_pre_retirement_hook<F>(
        &self,
        request: ExternalCaptureRequest<'_>,
        before_retirements: F,
    ) -> Result<String>
    where
        F: FnOnce(&Transaction<'_>) -> Result<()>,
    {
        let ExternalCaptureRequest {
            decision: d,
            scope,
            id,
            valid_from,
            fact_key,
            supersedes,
        } = request;
        if valid_from.is_some_and(|value| iso_to_epoch(value).is_none()) {
            return Err(CurrentRecordErrorCode::InvalidTemporalData.into());
        }
        let content_digest = digest(&format!("{}\n{}", d.subject, d.body));
        let importance = d.importance.clamp(0.0, 1.0);
        let commit = if d.kind == "commit" {
            d.sha.clone()
        } else {
            String::new()
        };
        let now = now_epoch();
        let now_str = epoch_to_iso(now);
        let vfrom = valid_from
            .map(String::from)
            .unwrap_or_else(|| now_str.clone());
        let identity = format!("external:{scope}:{id}");
        let fact_key = fact_key.filter(|k| !k.is_empty()).map(String::from);
        let tx = self.conn.unchecked_transaction()?;
        let existing = Self::record_digest_row_by_id_on(&tx, id)?;
        let observed_at = existing
            .as_ref()
            .map(|row| row.date.clone())
            .unwrap_or_else(|| now_str.clone());
        let effective_valid_from = match valid_from {
            Some(_) => Some(vfrom.clone()),
            None => existing
                .as_ref()
                .and_then(|row| row.valid_from.clone())
                .or_else(|| Some(vfrom.clone())),
        };
        let candidate = RecordDigestRow {
            id: id.to_owned(),
            scope: scope.to_owned(),
            kind: d.kind.clone(),
            title: d.subject.clone(),
            content: d.body.clone(),
            importance,
            source: d.source.clone(),
            author: d.author.clone(),
            commit_sha: commit.clone(),
            date: observed_at.clone(),
            tags: None,
            fact_key: fact_key.clone(),
            valid_from: effective_valid_from.clone(),
            declared_valid_until: None,
            sealed_digest: None,
        };
        let record_digest = record_digest_v1(&candidate)?;
        let exists = match existing {
            Some(existing) => {
                ensure_exact_record_replay(&existing, &candidate)?;
                true
            }
            None => false,
        };
        // Retire predecessors: the explicit supersedes id, then any current record that
        // shares the fact_key or the (kind, title).
        let mut predecessors: Vec<String> = supersedes
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .into_iter()
            .collect();
        let keyed: Vec<String> = match fact_key.as_deref() {
            Some(key) => tx
                .prepare(
                    "SELECT id FROM decisions WHERE scope=?1 AND kind=?2 AND fact_key=?3
                   AND id != ?4 AND superseded_by IS NULL AND valid_until IS NULL",
                )?
                .query_map(params![scope, d.kind, key, id], |r| r.get(0))?
                .filter_map(|r| r.ok())
                .collect(),
            None => Vec::new(),
        };
        let titled: Vec<String> = tx
            .prepare(
                "SELECT id FROM decisions WHERE scope=?1 AND kind=?2 AND title=?3
               AND id != ?4 AND superseded_by IS NULL AND valid_until IS NULL",
            )?
            .query_map(params![scope, d.kind, d.subject, id], |r| r.get(0))?
            .filter_map(|r| r.ok())
            .collect();
        predecessors.extend(keyed);
        predecessors.extend(titled);
        predecessors.sort();
        predecessors.dedup();
        let retirements = predecessors
            .into_iter()
            .map(|old| {
                pending_retirement_time_on(&tx, &old, scope, id, now)
                    .map(|retirement_at| (old, retirement_at))
            })
            .collect::<Result<Vec<_>>>()?;
        if !exists {
            let embedding = self.embed_text(&d.subject, &d.body, None);
            tx.execute(
                "INSERT OR IGNORE INTO decisions
               (id, kind, title, content, importance, source, author, commit_sha, date, scope,
                valid_from, fact_key, embedding, updated_at, content_digest, source_identity,
                created_epoch, record_digest_v1)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
                params![
                    id,
                    d.kind,
                    d.subject,
                    d.body,
                    importance,
                    d.source,
                    d.author,
                    commit,
                    observed_at,
                    scope,
                    effective_valid_from,
                    fact_key,
                    embedding,
                    now_str,
                    content_digest,
                    identity,
                    now,
                    record_digest
                ],
            )?;
        }
        before_retirements(&tx)?;
        for (old, retirement) in retirements {
            if let Some(retirement) = retirement {
                let affected = tx.execute(
                    "UPDATE decisions SET superseded_by=?1, valid_until=?2
                     WHERE id=?3 AND scope=?4 AND superseded_by IS NULL
                       AND valid_from IS ?5 AND valid_until IS ?6",
                    params![
                        id,
                        retirement.retirement_at,
                        old,
                        scope,
                        retirement.expected_valid_from,
                        retirement.expected_valid_until
                    ],
                )?;
                if affected != 1 {
                    return Err(SupersessionConflict.into());
                }
            }
        }
        let stored = Self::record_digest_row_by_id_on(&tx, id)?
            .context("capture insert did not persist its external record id")?;
        ensure_exact_record_replay(&stored, &candidate)?;
        tx.commit()?;
        Ok(id.to_string())
    }

    /// Bulk-import externally-minted decisions, preserving ids, temporal windows,
    /// supersession, and git linkage. Exact immutable envelopes replay; a changed
    /// envelope for an existing ID fails before any record or relation effect.
    pub fn import_external(&self, rows: &[ExternalDecision]) -> Result<usize> {
        self.import_external_exact(rows)
    }

    /// Compatibility alias for the strict import contract.
    pub fn import_external_sealed(&self, rows: &[ExternalDecision]) -> Result<usize> {
        self.import_external_exact(rows)
    }

    fn import_external_exact(&self, rows: &[ExternalDecision]) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        let mut prepared = Vec::with_capacity(rows.len());
        let mut candidates = HashMap::new();
        for row in rows {
            let candidate = record_digest_row_from_external(row);
            let candidate_digest = record_digest_v1(&candidate)?;
            let duplicate = match candidates.get(&row.id) {
                Some(previous) if previous == &candidate_digest => true,
                Some(_) => return Err(RecordIdentityConflict.into()),
                None => {
                    candidates.insert(row.id.clone(), candidate_digest.clone());
                    false
                }
            };
            let exists = duplicate
                || match Self::record_digest_row_by_id_on(&tx, &row.id)? {
                    Some(existing) => {
                        ensure_exact_record_replay(&existing, &candidate)?;
                        true
                    }
                    None => false,
                };
            prepared.push((row, candidate_digest, exists));
        }
        {
            let mut stmt = tx.prepare(
                "INSERT INTO decisions
                   (id, kind, title, content, importance, source, author, commit_sha, date, scope,
                    superseded_by, valid_from, valid_until, fact_key, embedding, updated_at,
                    accessed_count, times_injected, effectiveness, tags, content_digest,
                    source_identity, created_epoch, declared_valid_until, record_digest_v1)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,'',?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24)",
            )?;
            for (r, record_digest, exists) in &prepared {
                if *exists {
                    continue;
                }
                let content_digest = digest(&format!("{}\n{}", r.title, r.content));
                let identity = format!("external:{}:{}", r.scope, r.id);
                let epoch = iso_to_epoch(&r.date).unwrap_or(now_epoch());
                let embedding = self.embed_text(&r.title, &r.content, r.tags.as_deref());
                let updated_at = r.updated_at.clone().unwrap_or_else(|| r.date.clone());
                stmt.execute(params![
                    r.id,
                    r.kind,
                    r.title,
                    r.content,
                    r.importance.clamp(0.0, 1.0),
                    r.source,
                    r.author,
                    r.date,
                    r.scope,
                    r.superseded_by,
                    r.valid_from,
                    r.valid_until,
                    r.fact_key,
                    embedding,
                    updated_at,
                    r.accessed_count.unwrap_or(0),
                    r.times_injected.unwrap_or(0),
                    r.effectiveness.unwrap_or(0.5),
                    r.tags,
                    content_digest,
                    identity,
                    epoch,
                    r.valid_until,
                    record_digest
                ])?;
            }
        }
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO decision_git_refs (decision_id, commit_hash, commit_subject)
                 VALUES (?1,?2,?3)",
            )?;
            for (r, _, _) in &prepared {
                for g in &r.git_refs {
                    stmt.execute(params![r.id, g.commit_hash, g.commit_subject])?;
                }
            }
        }
        tx.commit()?;
        Ok(prepared.iter().filter(|(_, _, exists)| !exists).count())
    }

    /// Search active decisions across scopes and hybrid-rank them. `kinds` is an optional
    /// type facet (`decision`/`fact`/`reference`/…); an empty slice applies no facet.
    pub fn search(
        &self,
        query: &str,
        scopes: &[&str],
        kinds: &[String],
        limit: usize,
    ) -> Result<Vec<Decision>> {
        self.search_with(query, scopes, kinds, limit, false)
    }

    /// `search` with supersession control. `include_superseded` relaxes the active-only filter so
    /// retired decisions surface too, providing the historical arm of "what changed and why".
    pub fn search_with(
        &self,
        query: &str,
        scopes: &[&str],
        kinds: &[String],
        limit: usize,
        include_superseded: bool,
    ) -> Result<Vec<Decision>> {
        if scopes.is_empty() {
            return Ok(Vec::new());
        }
        let (rows, rowids) = self.select_decisions(scopes, kinds, include_superseded)?;
        let lexical =
            self.lexical_indices(query, &rowids, scopes, kinds, limit, include_superseded)?;
        let qe = self.query_embedding(query);
        Ok(rank(
            query,
            qe.as_deref(),
            rows,
            lexical,
            now_epoch(),
            limit,
        ))
    }

    /// Fetch candidate rows with their integer rowids, in scope and kind order. The
    /// rowid is the join key between the semantic candidates and the FTS5 lexical index.
    fn select_decisions(
        &self,
        scopes: &[&str],
        kinds: &[String],
        include_superseded: bool,
    ) -> Result<(Vec<Decision>, Vec<i64>)> {
        let validity = if include_superseded {
            ""
        } else {
            " AND superseded_by IS NULL
              AND (valid_from IS NULL OR unixepoch(valid_from) <= unixepoch('now'))
              AND (valid_until IS NULL OR unixepoch(valid_until) > unixepoch('now'))"
        };
        let placeholders = vec!["?"; scopes.len()].join(",");
        let kind_clause = if kinds.is_empty() {
            String::new()
        } else {
            format!(" AND kind IN ({})", vec!["?"; kinds.len()].join(","))
        };
        let sql = format!(
            "SELECT rowid,kind,title,content,importance,source,author,commit_sha,date,updated_at,
                    COALESCE(accessed_count,0)+COALESCE(times_injected,0), effectiveness, embedding
             FROM decisions
             WHERE 1=1{validity}
               AND scope IN ({placeholders}){kind_clause}"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut scope_params: Vec<&dyn rusqlite::ToSql> =
            scopes.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        for k in kinds {
            scope_params.push(k as &dyn rusqlite::ToSql);
        }
        let rows = stmt.query_map(scope_params.as_slice(), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                Decision {
                    kind: r.get(1)?,
                    subject: r.get(2)?,
                    body: r.get(3)?,
                    importance: r.get(4)?,
                    source: r.get(5)?,
                    author: r.get(6)?,
                    sha: r.get(7)?,
                    date: r.get(8)?,
                    updated_at: r.get::<_, Option<String>>(9)?.unwrap_or_default(),
                    access_count: r.get(10)?,
                    effectiveness: r.get(11)?,
                    embedding: parse_embedding(r.get::<_, Option<String>>(12)?),
                },
            ))
        })?;
        let mut decisions = Vec::new();
        let mut rowids = Vec::new();
        for row in rows {
            let (rowid, d) = row?;
            rowids.push(rowid);
            decisions.push(d);
        }
        Ok((decisions, rowids))
    }

    /// Lexical arm ordering: the rowids of the FTS5 `bm25()` best-first match, narrow-then-broad,
    /// mapped to indices into `rowids` for reciprocal-rank fusion.
    fn lexical_indices(
        &self,
        query: &str,
        rowids: &[i64],
        scopes: &[&str],
        kinds: &[String],
        limit: usize,
        include_superseded: bool,
    ) -> Result<Vec<usize>> {
        let index: HashMap<i64, usize> = rowids.iter().enumerate().map(|(i, &r)| (r, i)).collect();
        let ordered = self.lexical_rowids(query, scopes, kinds, limit, include_superseded)?;
        Ok(ordered
            .iter()
            .filter_map(|r| index.get(r).copied())
            .collect())
    }

    /// Run the FTS5 lexical query (narrow-then-broad over quoted terms) and return the matched
    /// rowids ordered by `bm25(decisions_fts, 0, 10, 5, 1)`.
    fn lexical_rowids(
        &self,
        query: &str,
        scopes: &[&str],
        kinds: &[String],
        limit: usize,
        include_superseded: bool,
    ) -> Result<Vec<i64>> {
        let terms = crate::search::tokenize(query);
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let quoted: Vec<String> = terms
            .iter()
            .map(|t| format!("\"{}\"", t.replace('"', "")))
            .collect();
        let narrow_floor = limit.min(5);
        let overfetch = limit.saturating_mul(10).max(limit);

        let validity = if include_superseded {
            ""
        } else {
            " AND d.superseded_by IS NULL
              AND (d.valid_from IS NULL OR unixepoch(d.valid_from) <= unixepoch('now'))
              AND (d.valid_until IS NULL OR unixepoch(d.valid_until) > unixepoch('now'))"
        };
        let placeholders = vec!["?"; scopes.len()].join(",");
        let kind_clause = if kinds.is_empty() {
            String::new()
        } else {
            format!(" AND d.kind IN ({})", vec!["?"; kinds.len()].join(","))
        };
        let sql = format!(
            "SELECT d.rowid FROM decisions_fts
             JOIN decisions d ON d.rowid = decisions_fts.rowid
             WHERE decisions_fts MATCH ?1{validity}
               AND d.scope IN ({placeholders}){kind_clause}
             ORDER BY bm25(decisions_fts, 0, 10, 5, 1)
             LIMIT ?"
        );

        let run = |match_expr: &str| -> Result<Vec<i64>> {
            let mut stmt = self.conn.prepare(&sql)?;
            let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            params.push(Box::new(match_expr.to_string()));
            for s in scopes {
                params.push(Box::new((*s).to_string()));
            }
            for k in kinds {
                params.push(Box::new(k.clone()));
            }
            params.push(Box::new(overfetch as i64));
            let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
            let rows = stmt.query_map(refs.as_slice(), |r| r.get::<_, i64>(0))?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        };

        if quoted.len() > 1 {
            let narrow = run(&quoted.join(" AND "))?;
            if narrow.len() >= narrow_floor {
                return Ok(narrow);
            }
            return run(&format!("({})", quoted.join(" OR ")));
        }
        run(&quoted.join(" OR "))
    }

    pub fn get(&self, id: &str) -> Result<Option<Decision>> {
        Ok(self
            .conn
            .query_row(
                "SELECT kind,title,content,importance,source,author,commit_sha,date
                 FROM decisions WHERE id=?1 AND superseded_by IS NULL
                   AND (valid_from IS NULL OR unixepoch(valid_from) <= unixepoch('now'))
                   AND (valid_until IS NULL OR unixepoch(valid_until) > unixepoch('now'))",
                params![id],
                |r| {
                    Ok(Decision {
                        kind: r.get(0)?,
                        subject: r.get(1)?,
                        body: r.get(2)?,
                        importance: r.get(3)?,
                        source: r.get(4)?,
                        author: r.get(5)?,
                        sha: r.get(6)?,
                        date: r.get(7)?,
                        ..Decision::default()
                    })
                },
            )
            .optional()?)
    }

    pub fn linked_commits(&self, decision_id: &str) -> Result<Vec<(String, String)>> {
        Self::linked_commits_on(&self.conn, decision_id)
    }

    fn linked_commits_on(conn: &Connection, decision_id: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = conn.prepare(
            "SELECT commit_hash, commit_subject FROM decision_git_refs
             WHERE decision_id=?1 ORDER BY created_at DESC, commit_hash ASC",
        )?;
        let rows = stmt.query_map(params![decision_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn get_commit_links(
        &self,
        scope: &str,
        commit: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<CommitLinksResolution> {
        anyhow::ensure!(
            (1..=MAX_COMMIT_LINKS_PAGE_RECORDS).contains(&limit),
            "commit-link page limit must be from 1 to {MAX_COMMIT_LINKS_PAGE_RECORDS}"
        );
        self.get_commit_links_with_hook(scope, commit, cursor, limit, || Ok(()))
    }

    fn get_commit_links_with_hook(
        &self,
        scope: &str,
        commit: &str,
        cursor: Option<&str>,
        limit: usize,
        after_snapshot: impl FnOnce() -> Result<()>,
    ) -> Result<CommitLinksResolution> {
        debug_assert!((1..=MAX_COMMIT_LINKS_PAGE_RECORDS).contains(&limit));
        let fail = |code, message: &str| CommitLinksResolution::Error {
            contract: COMMIT_LINKS_CONTRACT,
            code,
            message: message.to_owned(),
            retryable: false,
        };
        let transaction = self.conn.unchecked_transaction()?;

        if let Some(cursor) = cursor {
            let cursor_exists: bool = transaction.query_row(
                "SELECT EXISTS(
                     SELECT 1
                     FROM decision_git_refs AS refs
                     JOIN decisions AS decisions ON decisions.id=refs.decision_id
                     WHERE decisions.scope=?1 AND refs.commit_hash=?2
                       AND refs.decision_id=?3
                 )",
                params![scope, commit, cursor],
                |row| row.get(0),
            )?;
            if !cursor_exists {
                return Ok(fail(
                    CommitLinksErrorCode::InvalidCursor,
                    "cursor is not an authorized direct link for this exact scope and commit",
                ));
            }
        }

        // This bounded aggregate establishes the read snapshot and validates
        // every string that can enter the selected page before hydrating it.
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
        let (selected_count, max_id_bytes, max_subject_bytes, selected_bytes): (
            i64,
            i64,
            i64,
            i64,
        ) = transaction.query_row(
            "SELECT COUNT(*),
                    COALESCE(MAX(record_id_bytes),0),
                    COALESCE(MAX(subject_bytes),0),
                    COALESCE(SUM(record_id_bytes + subject_bytes),0)
             FROM (
                 SELECT length(CAST(refs.decision_id AS BLOB)) AS record_id_bytes,
                        length(CAST(refs.commit_subject AS BLOB)) AS subject_bytes
                 FROM decision_git_refs AS refs
                 JOIN decisions AS decisions ON decisions.id=refs.decision_id
                 WHERE decisions.scope=?1 AND refs.commit_hash=?2
                   AND (?3 IS NULL OR refs.decision_id >= ?3)
                 ORDER BY refs.decision_id ASC
                 LIMIT ?4
             )",
            params![scope, commit, cursor, limit_i64],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
        if selected_count == 0 {
            return Ok(fail(
                CommitLinksErrorCode::NotFound,
                "no direct rationale links were found in the requested scope",
            ));
        }
        if usize::try_from(max_id_bytes).unwrap_or(usize::MAX) > MAX_COMMIT_LINK_RECORD_ID_BYTES
            || usize::try_from(max_subject_bytes).unwrap_or(usize::MAX)
                > MAX_COMMIT_LINK_SUBJECT_BYTES
            || usize::try_from(selected_bytes).unwrap_or(usize::MAX)
                > MAX_COMMIT_LINKS_PAGE_SOURCE_BYTES
        {
            return Ok(fail(
                CommitLinksErrorCode::ResponseTooLarge,
                "commit links response exceeds the bounded exact-read budget",
            ));
        }

        after_snapshot()?;

        let mut statement = transaction.prepare(
            "SELECT refs.decision_id,refs.commit_subject
             FROM decision_git_refs AS refs
             JOIN decisions AS decisions ON decisions.id=refs.decision_id
             WHERE decisions.scope=?1 AND refs.commit_hash=?2
               AND (?3 IS NULL OR refs.decision_id >= ?3)
             ORDER BY refs.decision_id ASC
             LIMIT ?4",
        )?;
        let items = statement
            .query_map(params![scope, commit, cursor, limit_i64], |row| {
                Ok(CommitLinkItem {
                    record_id: row.get(0)?,
                    commit_subject: row.get(1)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);

        let next: Option<(String, i64)> = transaction
            .query_row(
                "SELECT refs.decision_id,length(CAST(refs.decision_id AS BLOB))
                 FROM decision_git_refs AS refs
                 JOIN decisions AS decisions ON decisions.id=refs.decision_id
                 WHERE decisions.scope=?1 AND refs.commit_hash=?2
                   AND (?3 IS NULL OR refs.decision_id >= ?3)
                 ORDER BY refs.decision_id ASC
                 LIMIT 1 OFFSET ?4",
                params![scope, commit, cursor, limit_i64],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let next_cursor = match next {
            Some((_, id_bytes))
                if usize::try_from(id_bytes).unwrap_or(usize::MAX)
                    > MAX_COMMIT_LINK_RECORD_ID_BYTES =>
            {
                return Ok(fail(
                    CommitLinksErrorCode::ResponseTooLarge,
                    "commit links response exceeds the bounded exact-read budget",
                ));
            }
            Some((id, _)) => Some(id),
            None => None,
        };

        Ok(CommitLinksResolution::Ok {
            contract: COMMIT_LINKS_CONTRACT,
            scope: scope.to_owned(),
            commit: commit.to_owned(),
            items,
            next_cursor,
        })
    }

    /// Resolve a stable record ID to the current, evidence-bearing end of its
    /// supersession chain.
    ///
    /// Resolve an exact stable ID at the production clock instant. Failures are
    /// typed so absence cannot be confused with damaged supersession history.
    pub fn get_current_evidence(&self, id: &str) -> Result<CurrentRecordResolution> {
        self.get_current_evidence_at(id, now_epoch(), MAX_SUPERSESSION_CHAIN)
    }

    /// Resolve an exact scoped record at the production clock and return the
    /// current record together with its verified immutable evidence identity.
    pub fn get_current_evidence_in_scope(
        &self,
        id: &str,
        scope: &str,
    ) -> Result<ScopedCurrentRecordResolution> {
        let read = self.get_current_evidence_at_with_scope_and_hook(
            id,
            Some(scope),
            now_epoch(),
            MAX_SUPERSESSION_CHAIN,
            true,
            || Ok(()),
        )?;
        Ok(scoped_current_resolution(read))
    }

    /// Clock-injected implementation used by the MCP server and deterministic tests.
    /// MCP callers never supply `as_of`; the server owns that clock authority.
    pub(crate) fn get_current_evidence_at(
        &self,
        id: &str,
        as_of: i64,
        chain_cap: usize,
    ) -> Result<CurrentRecordResolution> {
        Ok(self
            .get_current_evidence_at_with_scope_and_hook(id, None, as_of, chain_cap, false, || {
                Ok(())
            })?
            .resolution)
    }

    /// Resolve an exact record for an untrusted scoped caller without revealing
    /// whether an unavailable chain node exists in another scope.
    pub(crate) fn get_current_evidence_in_scope_at(
        &self,
        id: &str,
        scope: &str,
        as_of: i64,
        chain_cap: usize,
    ) -> Result<CurrentRecordResolution> {
        Ok(self
            .get_current_evidence_at_with_scope_and_hook(
                id,
                Some(scope),
                as_of,
                chain_cap,
                false,
                || Ok(()),
            )?
            .resolution)
    }

    fn get_current_evidence_at_with_scope_and_hook(
        &self,
        id: &str,
        scope: Option<&str>,
        as_of: i64,
        chain_cap: usize,
        include_identity: bool,
        after_root_lookup: impl FnOnce() -> Result<()>,
    ) -> Result<CurrentEvidenceRead> {
        let as_of_iso = epoch_to_iso(as_of);
        let fail = |code, message: String| CurrentRecordResolution::Error {
            contract: CURRENT_RATIONALE_CONTRACT,
            as_of: as_of_iso.clone(),
            requested_id: id.to_string(),
            code,
            message,
            retryable: false,
        };

        // One read transaction owns root authorization, every successor hop,
        // temporal validation, current-record hydration, and Git evidence. In
        // WAL mode concurrent commits become visible only to the next call.
        let transaction = self.conn.unchecked_transaction()?;
        let mut chain = Vec::new();
        let mut cursor = id.to_string();
        let mut seen = std::collections::HashSet::new();
        let mut after_root_lookup = Some(after_root_lookup);
        loop {
            if !seen.insert(cursor.clone()) {
                return Ok(CurrentEvidenceRead {
                    resolution: fail(
                        CurrentRecordErrorCode::Cycle,
                        format!("supersession cycle reaches `{cursor}`"),
                    ),
                    identity: None,
                });
            }
            let Some(node) = Self::current_node_on(&transaction, &cursor, scope)? else {
                let (code, message) = if chain.is_empty() {
                    (
                        CurrentRecordErrorCode::NotFound,
                        match scope {
                            Some(scope) => {
                                format!("record `{id}` was not found in scope `{scope}`")
                            }
                            None => format!("record `{id}` was not found"),
                        },
                    )
                } else {
                    (
                        CurrentRecordErrorCode::BrokenChain,
                        match scope {
                            Some(_) => "supersession chain is unavailable in the requested scope"
                                .to_owned(),
                            None => format!("supersession successor `{cursor}` was not found"),
                        },
                    )
                };
                return Ok(CurrentEvidenceRead {
                    resolution: fail(code, message),
                    identity: None,
                });
            };
            if chain.is_empty() {
                after_root_lookup
                    .take()
                    .expect("root lookup hook runs once")()?;
            }

            for (field, raw) in [
                ("valid_from", node.valid_from.as_deref()),
                ("valid_until", node.valid_until.as_deref()),
            ] {
                if let Some(raw) = raw.filter(|value| !value.is_empty()) {
                    if Self::temporal_epoch_on(&transaction, raw)?.is_none() {
                        return Ok(CurrentEvidenceRead {
                            resolution: fail(
                                CurrentRecordErrorCode::InvalidTemporalData,
                                format!("record `{}` has invalid {field} `{raw}`", node.id),
                            ),
                            identity: None,
                        });
                    }
                }
            }
            if let (Some(valid_from), Some(valid_until)) = (
                node.valid_from.as_deref().filter(|value| !value.is_empty()),
                node.valid_until
                    .as_deref()
                    .filter(|value| !value.is_empty()),
            ) {
                let from =
                    Self::temporal_epoch_on(&transaction, valid_from)?.expect("validated above");
                let until =
                    Self::temporal_epoch_on(&transaction, valid_until)?.expect("validated above");
                if from >= until {
                    return Ok(CurrentEvidenceRead {
                        resolution: fail(
                            CurrentRecordErrorCode::InvalidTemporalData,
                            format!("record `{}` has a non-positive validity interval", node.id),
                        ),
                        identity: None,
                    });
                }
            }

            let next = node
                .superseded_by
                .as_deref()
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
            chain.push(node);
            if let Some(next) = next {
                if chain.len() >= chain_cap {
                    return Ok(CurrentEvidenceRead {
                        resolution: fail(
                            CurrentRecordErrorCode::TraversalLimit,
                            format!("supersession chain exceeds {chain_cap} records"),
                        ),
                        identity: None,
                    });
                }
                cursor = next;
                continue;
            }
            break;
        }

        let current = chain.last().expect("a fetched chain is non-empty");
        if let Some(valid_from) = current.valid_from.as_deref().filter(|v| !v.is_empty()) {
            let epoch =
                Self::temporal_epoch_on(&transaction, valid_from)?.expect("validated above");
            if as_of < epoch {
                return Ok(CurrentEvidenceRead {
                    resolution: fail(
                        CurrentRecordErrorCode::NotYetValid,
                        format!("record `{}` is not current at {as_of_iso}", current.id),
                    ),
                    identity: None,
                });
            }
        }
        if let Some(valid_until) = current.valid_until.as_deref().filter(|v| !v.is_empty()) {
            let epoch =
                Self::temporal_epoch_on(&transaction, valid_until)?.expect("validated above");
            if as_of >= epoch {
                return Ok(CurrentEvidenceRead {
                    resolution: fail(
                        CurrentRecordErrorCode::ExpiredWithoutSuccessor,
                        format!(
                            "record `{}` expired without a successor at `{valid_until}`",
                            current.id
                        ),
                    ),
                    identity: None,
                });
            }
        }

        let record = Self::get_record_any_on(&transaction, &current.id, true)?
            .expect("authorized current metadata remains visible in its read snapshot");
        let git_refs = Self::linked_commits_on(&transaction, &current.id)?
            .into_iter()
            .map(|(commit_hash, commit_subject)| GitRef {
                commit_hash,
                commit_subject,
            })
            .collect();
        let identity = if include_identity {
            match Self::evidence_identity_on(&transaction, &current.id, &record.scope)? {
                EvidenceIdentityResolution::Ok { identity } => Some(identity),
                EvidenceIdentityResolution::Error { .. } => None,
            }
        } else {
            None
        };
        Ok(CurrentEvidenceRead {
            resolution: CurrentRecordResolution::Ok {
                contract: CURRENT_RATIONALE_CONTRACT,
                as_of: as_of_iso,
                requested_id: id.to_string(),
                current_id: current.id.clone(),
                record: Box::new(record),
                git_refs,
                supersession_chain: chain.into_iter().map(|node| node.id).collect(),
            },
            identity,
        })
    }

    fn current_node_on(
        conn: &Connection,
        id: &str,
        scope: Option<&str>,
    ) -> Result<Option<CurrentNode>> {
        let read = |row: &rusqlite::Row<'_>| {
            Ok(CurrentNode {
                id: row.get(0)?,
                superseded_by: row.get(1)?,
                valid_from: row.get(2)?,
                valid_until: row.get(3)?,
            })
        };
        match scope {
            Some(scope) => Ok(conn
                .query_row(
                    "SELECT id,superseded_by,valid_from,valid_until
                     FROM decisions WHERE id=?1 AND scope=?2",
                    params![id, scope],
                    read,
                )
                .optional()?),
            None => Ok(conn
                .query_row(
                    "SELECT id,superseded_by,valid_from,valid_until
                     FROM decisions WHERE id=?1",
                    params![id],
                    read,
                )
                .optional()?),
        }
    }

    /// Return one evidence-bearing page from the exact forward supersession
    /// chain rooted at `id`.
    pub fn get_rationale_history(
        &self,
        id: &str,
        scope: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<RationaleHistoryResolution> {
        anyhow::ensure!(
            (1..=MAX_HISTORY_PAGE_RECORDS).contains(&limit),
            "history page limit must be from 1 to {MAX_HISTORY_PAGE_RECORDS}"
        );
        self.get_rationale_history_at(
            id,
            scope,
            cursor,
            limit,
            now_epoch(),
            MAX_SUPERSESSION_CHAIN,
        )
    }

    /// Clock- and traversal-cap-injected implementation used by the MCP server
    /// and deterministic tests. Callers must validate `limit` against
    /// `MAX_HISTORY_PAGE_RECORDS` before entering this exact read.
    pub(crate) fn get_rationale_history_at(
        &self,
        id: &str,
        scope: &str,
        page_cursor: Option<&str>,
        limit: usize,
        as_of: i64,
        chain_cap: usize,
    ) -> Result<RationaleHistoryResolution> {
        self.get_rationale_history_at_with_hook(
            HistoryPageRequest {
                id,
                scope,
                page_cursor,
                limit,
                as_of,
                chain_cap,
            },
            || Ok(()),
        )
    }

    fn get_rationale_history_at_with_hook(
        &self,
        request: HistoryPageRequest<'_>,
        after_metadata: impl FnOnce() -> Result<()>,
    ) -> Result<RationaleHistoryResolution> {
        let HistoryPageRequest {
            id,
            scope,
            page_cursor,
            limit,
            as_of,
            chain_cap,
        } = request;
        debug_assert!((1..=MAX_HISTORY_PAGE_RECORDS).contains(&limit));
        let as_of_iso = epoch_to_iso(as_of);
        let fail = |code, message: String| RationaleHistoryResolution::Error {
            contract: RATIONALE_HISTORY_CONTRACT,
            as_of: as_of_iso.clone(),
            requested_id: id.to_owned(),
            code,
            message,
            retryable: false,
        };

        // One read transaction owns chain discovery, cursor validation, budget
        // preflight, full-record hydration, and evidence hydration. In WAL mode a
        // writer may commit concurrently, but this page remains one SQLite snapshot.
        let transaction = self.conn.unchecked_transaction()?;
        let mut chain = Vec::new();
        let mut cursor = id.to_owned();
        let mut seen = std::collections::HashSet::new();
        loop {
            if !seen.insert(cursor.clone()) {
                return Ok(fail(
                    RationaleHistoryErrorCode::Cycle,
                    format!("supersession cycle reaches `{cursor}`"),
                ));
            }
            let Some(node) = Self::history_node_on(&transaction, &cursor)? else {
                let (code, message) = if chain.is_empty() {
                    (
                        RationaleHistoryErrorCode::NotFound,
                        format!("record `{id}` was not found in scope `{scope}`"),
                    )
                } else {
                    (
                        RationaleHistoryErrorCode::BrokenChain,
                        "supersession chain is unavailable in the requested scope".to_owned(),
                    )
                };
                return Ok(fail(code, message));
            };
            if node.scope != scope {
                let (code, message) = if chain.is_empty() {
                    (
                        RationaleHistoryErrorCode::NotFound,
                        format!("record `{id}` was not found in scope `{scope}`"),
                    )
                } else {
                    (
                        RationaleHistoryErrorCode::BrokenChain,
                        "supersession chain is unavailable in the requested scope".to_owned(),
                    )
                };
                return Ok(fail(code, message));
            }

            for (field, raw) in [
                ("valid_from", node.valid_from.as_deref()),
                ("valid_until", node.valid_until.as_deref()),
            ] {
                if let Some(raw) = raw.filter(|value| !value.is_empty()) {
                    if Self::temporal_epoch_on(&transaction, raw)?.is_none() {
                        return Ok(fail(
                            RationaleHistoryErrorCode::InvalidTemporalData,
                            format!("record `{}` has invalid {field} `{raw}`", node.id),
                        ));
                    }
                }
            }
            if let (Some(valid_from), Some(valid_until)) = (
                node.valid_from.as_deref().filter(|value| !value.is_empty()),
                node.valid_until
                    .as_deref()
                    .filter(|value| !value.is_empty()),
            ) {
                let from =
                    Self::temporal_epoch_on(&transaction, valid_from)?.expect("validated above");
                let until =
                    Self::temporal_epoch_on(&transaction, valid_until)?.expect("validated above");
                if from >= until {
                    return Ok(fail(
                        RationaleHistoryErrorCode::InvalidTemporalData,
                        format!("record `{}` has a non-positive validity interval", node.id),
                    ));
                }
            }

            // History v1 validates each record's timestamp syntax and positive
            // interval independently. It deliberately does not certify temporal
            // continuity or non-overlap between adjacent records.
            let next = node
                .superseded_by
                .as_deref()
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
            chain.push(node);
            if let Some(next) = next {
                if chain.len() >= chain_cap {
                    return Ok(fail(
                        RationaleHistoryErrorCode::TraversalLimit,
                        format!("supersession chain exceeds {chain_cap} records"),
                    ));
                }
                cursor = next;
                continue;
            }
            break;
        }

        let page_start_id = page_cursor.unwrap_or(id);
        let Some(start) = chain.iter().position(|node| node.id == page_start_id) else {
            return Ok(fail(
                RationaleHistoryErrorCode::InvalidCursor,
                "cursor is not on the supersession chain rooted at the requested record".to_owned(),
            ));
        };
        let end = (start + limit).min(chain.len());
        let complete = end == chain.len();
        let next_cursor = (!complete).then(|| chain[end].id.clone());
        after_metadata()?;

        let selected_ids: Vec<&str> = chain[start..end]
            .iter()
            .map(|node| node.id.as_str())
            .collect();
        let mut source_bytes = 0_usize;
        let mut git_ref_count = 0_usize;
        for selected_id in &selected_ids {
            let (record_bytes, refs, ref_bytes) =
                Self::history_budget_on(&transaction, selected_id)?;
            source_bytes = source_bytes
                .saturating_add(record_bytes)
                .saturating_add(ref_bytes);
            git_ref_count = git_ref_count.saturating_add(refs);
            if source_bytes > MAX_HISTORY_PAGE_SOURCE_BYTES
                || git_ref_count > MAX_HISTORY_PAGE_GIT_REFS
            {
                return Ok(fail(
                    RationaleHistoryErrorCode::ResponseTooLarge,
                    "exact history page exceeds the cumulative source budget".to_owned(),
                ));
            }
        }

        let mut records = Vec::with_capacity(selected_ids.len());
        for selected_id in selected_ids {
            let record = Self::get_record_any_on(&transaction, selected_id, true)?
                .expect("selected history metadata remains visible in its read snapshot");
            let git_refs = Self::linked_commits_on(&transaction, selected_id)?
                .into_iter()
                .map(|(commit_hash, commit_subject)| GitRef {
                    commit_hash,
                    commit_subject,
                })
                .collect();
            records.push(RationaleHistoryRecord {
                record: Box::new(record),
                git_refs,
            });
        }

        Ok(RationaleHistoryResolution::Ok {
            contract: RATIONALE_HISTORY_CONTRACT,
            as_of: as_of_iso,
            requested_id: id.to_owned(),
            page_start_id: page_start_id.to_owned(),
            records,
            next_cursor,
            complete,
        })
    }

    fn history_node_on(conn: &Connection, id: &str) -> Result<Option<HistoryNode>> {
        Ok(conn
            .query_row(
                "SELECT id,scope,superseded_by,valid_from,valid_until
                 FROM decisions WHERE id=?1",
                params![id],
                |row| {
                    Ok(HistoryNode {
                        id: row.get(0)?,
                        scope: row.get(1)?,
                        superseded_by: row.get(2)?,
                        valid_from: row.get(3)?,
                        valid_until: row.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    fn history_budget_on(conn: &Connection, id: &str) -> Result<(usize, usize, usize)> {
        let record_bytes: i64 = conn.query_row(
            "SELECT length(CAST(id AS BLOB)) + length(CAST(kind AS BLOB))
                    + length(CAST(title AS BLOB)) + length(CAST(content AS BLOB))
                    + length(CAST(source AS BLOB)) + length(CAST(author AS BLOB))
                    + length(CAST(commit_sha AS BLOB)) + length(CAST(date AS BLOB))
                    + length(CAST(scope AS BLOB))
                    + COALESCE(length(CAST(superseded_by AS BLOB)),0)
                    + COALESCE(length(CAST(valid_from AS BLOB)),0)
                    + COALESCE(length(CAST(valid_until AS BLOB)),0)
                    + COALESCE(length(CAST(updated_at AS BLOB)),0)
             FROM decisions WHERE id=?1",
            params![id],
            |row| row.get(0),
        )?;
        let (git_ref_count, git_ref_bytes): (i64, i64) = conn.query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(length(CAST(commit_hash AS BLOB))
                               + length(CAST(commit_subject AS BLOB))),0)
             FROM decision_git_refs WHERE decision_id=?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok((
            usize::try_from(record_bytes).unwrap_or(usize::MAX),
            usize::try_from(git_ref_count).unwrap_or(usize::MAX),
            usize::try_from(git_ref_bytes).unwrap_or(usize::MAX),
        ))
    }

    pub(crate) fn temporal_epoch(&self, value: &str) -> Result<Option<i64>> {
        Self::temporal_epoch_on(&self.conn, value)
    }

    fn temporal_epoch_on(conn: &Connection, value: &str) -> Result<Option<i64>> {
        Ok(conn.query_row("SELECT unixepoch(?1)", params![value], |row| row.get(0))?)
    }

    pub fn temporal_value_is_valid(&self, value: &str) -> Result<bool> {
        Ok(self.temporal_epoch(value)?.is_some())
    }

    pub fn record_belongs_to_scope(&self, id: &str, scope: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM decisions WHERE id=?1 AND scope=?2",
            params![id, scope],
            |row| row.get(0),
        )?;
        Ok(count == 1)
    }

    /// Search active decisions across scopes and return full records (id + temporal
    /// window) in hybrid-ranked order. Structured counterpart of `search`.
    pub fn search_records(
        &self,
        query: &str,
        scopes: &[&str],
        kinds: &[String],
        limit: usize,
    ) -> Result<Vec<Record>> {
        self.search_records_with(query, scopes, kinds, limit, false)
    }

    /// `search_records` with supersession control. With `include_superseded`, retired decisions
    /// surface too and carry their `superseded_by` / `valid_until` so a caller can follow the chain.
    pub fn search_records_with(
        &self,
        query: &str,
        scopes: &[&str],
        kinds: &[String],
        limit: usize,
        include_superseded: bool,
    ) -> Result<Vec<Record>> {
        Ok(self
            .rank_records(query, scopes, kinds, limit, include_superseded)?
            .0)
    }

    /// `search_records_with` returning per-result ranking explanations alongside.
    pub fn search_records_explain(
        &self,
        query: &str,
        scopes: &[&str],
        kinds: &[String],
        limit: usize,
        include_superseded: bool,
    ) -> Result<Explained> {
        let (records, explanations) =
            self.rank_records(query, scopes, kinds, limit, include_superseded)?;
        Ok(records.into_iter().zip(explanations).collect())
    }

    /// Search and split into `(results, drops)`: the top `limit` and the next `drop_count`
    /// near-miss candidates, each with its ranking explanation. The drops are the candidates
    /// that fused but lost the top-N slice: "what didn't make it, and by how much".
    pub fn search_records_drops(
        &self,
        query: &str,
        scopes: &[&str],
        kinds: &[String],
        limit: usize,
        include_superseded: bool,
        drop_count: usize,
    ) -> Result<(Explained, Explained)> {
        let (records, explanations) =
            self.rank_records(query, scopes, kinds, limit + drop_count, include_superseded)?;
        let pairs: Vec<(Record, RankExplanation)> = records.into_iter().zip(explanations).collect();
        let (results, drops) = pairs.split_at(pairs.len().min(limit));
        Ok((results.to_vec(), drops.to_vec()))
    }

    fn rank_records(
        &self,
        query: &str,
        scopes: &[&str],
        kinds: &[String],
        limit: usize,
        include_superseded: bool,
    ) -> Result<(Vec<Record>, Vec<RankExplanation>)> {
        if scopes.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let (rows, rowids) = self.select_records(scopes, kinds, include_superseded)?;
        let lexical =
            self.lexical_indices(query, &rowids, scopes, kinds, limit, include_superseded)?;
        let qe = self.query_embedding(query);
        Ok(rank_by(
            query,
            qe.as_deref(),
            rows,
            lexical,
            now_epoch(),
            limit,
            |d| RankRow {
                importance: d.importance,
                kind: &d.kind,
                date: &d.date,
                updated_at: if d.updated_at.is_empty() {
                    None
                } else {
                    Some(&d.updated_at)
                },
                access_count: d.access_count,
                effectiveness: d.effectiveness,
                embedding: d.embedding.as_deref(),
                title: &d.title,
                content: &d.content,
            },
        ))
    }

    fn select_records(
        &self,
        scopes: &[&str],
        kinds: &[String],
        include_superseded: bool,
    ) -> Result<(Vec<Record>, Vec<i64>)> {
        let validity = if include_superseded {
            ""
        } else {
            " AND superseded_by IS NULL
              AND (valid_from IS NULL OR unixepoch(valid_from) <= unixepoch('now'))
              AND (valid_until IS NULL OR unixepoch(valid_until) > unixepoch('now'))"
        };
        let placeholders = vec!["?"; scopes.len()].join(",");
        let kind_clause = if kinds.is_empty() {
            String::new()
        } else {
            format!(" AND kind IN ({})", vec!["?"; kinds.len()].join(","))
        };
        let sql = format!(
            "SELECT rowid,id,kind,title,content,importance,source,author,commit_sha,date,scope,
                    superseded_by,valid_from,valid_until,updated_at,
                    COALESCE(accessed_count,0)+COALESCE(times_injected,0), effectiveness, embedding
             FROM decisions
             WHERE 1=1{validity}
               AND scope IN ({placeholders}){kind_clause}"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut scope_params: Vec<&dyn rusqlite::ToSql> =
            scopes.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        for k in kinds {
            scope_params.push(k as &dyn rusqlite::ToSql);
        }
        let rows = stmt.query_map(scope_params.as_slice(), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                Record {
                    id: r.get(1)?,
                    kind: r.get(2)?,
                    title: r.get(3)?,
                    content: r.get(4)?,
                    importance: r.get(5)?,
                    source: r.get(6)?,
                    author: r.get(7)?,
                    commit_sha: r.get(8)?,
                    date: r.get(9)?,
                    scope: r.get(10)?,
                    superseded_by: r.get(11)?,
                    valid_from: r.get(12)?,
                    valid_until: r.get(13)?,
                    updated_at: r.get::<_, Option<String>>(14)?.unwrap_or_default(),
                    access_count: r.get(15)?,
                    effectiveness: r.get(16)?,
                    embedding: parse_embedding(r.get::<_, Option<String>>(17)?),
                },
            ))
        })?;
        let mut records = Vec::new();
        let mut rowids = Vec::new();
        for row in rows {
            let (rowid, rec) = row?;
            rowids.push(rowid);
            records.push(rec);
        }
        Ok((records, rowids))
    }

    pub fn get_record(&self, id: &str) -> Result<Option<Record>> {
        self.get_record_any(id, false)
    }

    /// Fetch a record by id, optionally reaching past supersession (historical mode). The
    /// `superseded_by` / `valid_until` fields describe where the record sits in its chain.
    pub fn get_record_any(&self, id: &str, include_superseded: bool) -> Result<Option<Record>> {
        Self::get_record_any_on(&self.conn, id, include_superseded)
    }

    fn get_record_any_on(
        conn: &Connection,
        id: &str,
        include_superseded: bool,
    ) -> Result<Option<Record>> {
        let validity = if include_superseded {
            ""
        } else {
            " AND superseded_by IS NULL
              AND (valid_from IS NULL OR unixepoch(valid_from) <= unixepoch('now'))
              AND (valid_until IS NULL OR unixepoch(valid_until) > unixepoch('now'))"
        };
        let sql = format!(
            "SELECT id,kind,title,content,importance,source,author,commit_sha,date,scope,
                    superseded_by,valid_from,valid_until,updated_at,
                    COALESCE(accessed_count,0)+COALESCE(times_injected,0), effectiveness
             FROM decisions WHERE id=?1{validity}"
        );
        Ok(conn
            .query_row(&sql, params![id], |r| {
                Ok(Record {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    title: r.get(2)?,
                    content: r.get(3)?,
                    importance: r.get(4)?,
                    source: r.get(5)?,
                    author: r.get(6)?,
                    commit_sha: r.get(7)?,
                    date: r.get(8)?,
                    scope: r.get(9)?,
                    superseded_by: r.get(10)?,
                    valid_from: r.get(11)?,
                    valid_until: r.get(12)?,
                    updated_at: r.get::<_, Option<String>>(13)?.unwrap_or_default(),
                    access_count: r.get(14)?,
                    effectiveness: r.get(15)?,
                    embedding: None,
                })
            })
            .optional()?)
    }

    /// Walk the supersession chain forward from `id`:
    /// `[id, superseded_by(id), superseded_by(...)]`
    /// until a record with no successor. Returns at most `cap` records; an unknown id yields empty.
    pub fn supersession_chain(&self, id: &str, cap: usize) -> Result<Vec<Record>> {
        let mut out = Vec::new();
        let mut cursor = id.to_string();
        let mut seen = std::collections::HashSet::new();
        while out.len() < cap && seen.insert(cursor.clone()) {
            match self.get_record_any(&cursor, true)? {
                Some(rec) => {
                    let next = rec.superseded_by.clone();
                    out.push(rec);
                    match next {
                        Some(n) if !n.is_empty() => cursor = n,
                        _ => break,
                    }
                }
                None => break,
            }
        }
        Ok(out)
    }

    /// Compatibility-only commit linking for trusted callers that already own
    /// store authority.
    ///
    /// # Deprecated
    ///
    /// This authority-bypassing API remains for semantic-version compatibility.
    /// Untrusted integrations must use `link_git_in_scope`.
    pub fn link_git(
        &self,
        decision_id: &str,
        commit_hash: &str,
        commit_subject: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO decision_git_refs (decision_id, commit_hash, commit_subject)
             VALUES (?1,?2,?3)",
            params![decision_id, commit_hash, commit_subject],
        )?;
        Ok(())
    }

    /// Atomically link a commit through a sealed, store-bound evidence identity.
    pub fn link_git_in_scope(
        &self,
        evidence_identity: &EvidenceIdentity,
        commit_hash: &str,
        commit_subject: &str,
    ) -> ScopedCommitLinkResolution {
        if commit_hash.is_empty()
            || commit_hash.len() > MAX_COMMIT_LINK_HASH_BYTES
            || commit_subject.len() > MAX_COMMIT_LINK_SUBJECT_BYTES
        {
            return scoped_commit_link_error(ScopedCommitLinkErrorCode::InvalidRequest, false);
        }
        if !valid_evidence_identity_shape(evidence_identity) {
            return scoped_commit_link_error(ScopedCommitLinkErrorCode::EvidenceUnavailable, false);
        }

        match self.link_git_in_scope_inner(evidence_identity, commit_hash, commit_subject) {
            Ok(resolution) => resolution,
            Err(error) => scoped_commit_link_error(
                ScopedCommitLinkErrorCode::StoreUnavailable,
                store_error_is_retryable(&error),
            ),
        }
    }

    fn link_git_in_scope_inner(
        &self,
        supplied: &EvidenceIdentity,
        commit_hash: &str,
        commit_subject: &str,
    ) -> Result<ScopedCommitLinkResolution> {
        let transaction = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let store_instance_id: String = transaction.query_row(
            "SELECT store_instance_id FROM open_why_metadata WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        let metadata: Option<(String, String, Option<String>)> = transaction
            .query_row(
                "SELECT id,scope,record_digest_v1 FROM decisions WHERE id=?1 AND scope=?2",
                params![supplied.record_id, supplied.scope],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((record_id, scope, Some(sealed_digest))) = metadata else {
            transaction.rollback()?;
            return Ok(scoped_commit_link_error(
                ScopedCommitLinkErrorCode::EvidenceUnavailable,
                false,
            ));
        };
        if store_instance_id != supplied.store_instance_id
            || scope != supplied.scope
            || record_id != supplied.record_id
            || sealed_digest != supplied.record_digest
        {
            transaction.rollback()?;
            return Ok(scoped_commit_link_error(
                ScopedCommitLinkErrorCode::EvidenceUnavailable,
                false,
            ));
        }

        let Some(row) = Self::record_digest_row_in_scope_on(&transaction, &record_id, &scope)?
        else {
            transaction.rollback()?;
            return Ok(scoped_commit_link_error(
                ScopedCommitLinkErrorCode::EvidenceUnavailable,
                false,
            ));
        };
        if record_digest_v1(&row).ok().as_deref() != Some(sealed_digest.as_str()) {
            transaction.rollback()?;
            return Ok(scoped_commit_link_error(
                ScopedCommitLinkErrorCode::EvidenceUnavailable,
                false,
            ));
        }

        let authoritative_identity = EvidenceIdentity {
            contract: EVIDENCE_IDENTITY_CONTRACT,
            record_digest_contract: RECORD_DIGEST_CONTRACT,
            store_instance_id,
            scope,
            record_id,
            record_digest: sealed_digest,
        };
        let existing_git_ref: Option<GitRef> = transaction
            .query_row(
                "SELECT commit_hash,commit_subject FROM decision_git_refs
                 WHERE decision_id=?1 AND commit_hash=?2",
                params![authoritative_identity.record_id, commit_hash],
                |row| {
                    Ok(GitRef {
                        commit_hash: row.get(0)?,
                        commit_subject: row.get(1)?,
                    })
                },
            )
            .optional()?;
        if let Some(git_ref) = existing_git_ref {
            if git_ref.commit_subject != commit_subject {
                transaction.rollback()?;
                return Ok(scoped_commit_link_error(
                    ScopedCommitLinkErrorCode::LinkConflict,
                    false,
                ));
            }
            transaction.rollback()?;
            return Ok(ScopedCommitLinkResolution::Ok {
                contract: SCOPED_COMMIT_LINK_WRITE_CONTRACT,
                outcome: ScopedCommitLinkOutcome::ExactReplay,
                evidence_identity: authoritative_identity,
                git_ref,
            });
        }

        let affected = transaction.execute(
            "INSERT INTO decision_git_refs (decision_id,commit_hash,commit_subject)
             VALUES (?1,?2,?3)",
            params![
                authoritative_identity.record_id,
                commit_hash,
                commit_subject
            ],
        )?;
        if affected != 1 {
            transaction.rollback()?;
            anyhow::bail!("commit-link insert did not affect exactly one row");
        }
        let git_ref = transaction.query_row(
            "SELECT commit_hash,commit_subject FROM decision_git_refs
             WHERE decision_id=?1 AND commit_hash=?2",
            params![authoritative_identity.record_id, commit_hash],
            |row| {
                Ok(GitRef {
                    commit_hash: row.get(0)?,
                    commit_subject: row.get(1)?,
                })
            },
        )?;
        transaction.commit()?;
        Ok(ScopedCommitLinkResolution::Ok {
            contract: SCOPED_COMMIT_LINK_WRITE_CONTRACT,
            outcome: ScopedCommitLinkOutcome::Created,
            evidence_identity: authoritative_identity,
            git_ref,
        })
    }

    /// Bulk-import mined decisions (commits + ADRs) into a scope. Idempotent.
    pub fn import_decisions(&self, scope: &str, decisions: &[Decision]) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        let mut prepared = Vec::with_capacity(decisions.len());
        for decision in decisions {
            let identity = if decision.kind == "commit" {
                format!("git:{scope}:commit:{}", decision.sha)
            } else {
                format!("git:{scope}:file:{}", decision.source)
            };
            let content_digest = digest(&format!("{}\n{}", decision.subject, decision.body));
            let id = if decision.kind == "commit" && !decision.sha.is_empty() {
                decision.sha.clone()
            } else {
                digest(&format!("{identity}\n{content_digest}"))
            };
            let record = RecordDigestRow {
                id: id.clone(),
                scope: scope.to_owned(),
                kind: decision.kind.clone(),
                title: decision.subject.clone(),
                content: decision.body.clone(),
                importance: decision.importance.clamp(0.0, 1.0),
                source: decision.source.clone(),
                author: decision.author.clone(),
                commit_sha: if decision.kind == "commit" {
                    decision.sha.clone()
                } else {
                    String::new()
                },
                date: decision.date.clone(),
                tags: None,
                fact_key: None,
                valid_from: None,
                declared_valid_until: None,
                sealed_digest: None,
            };
            let record_digest = record_digest_v1(&record)?;
            let exists = Self::record_digest_row_by_id_on(&tx, &id)?.is_some();
            prepared.push((
                decision,
                id,
                identity,
                content_digest,
                record_digest,
                exists,
            ));
        }
        {
            let mut stmt = tx.prepare(
                "INSERT INTO decisions
                   (id, kind, title, content, importance, source, author, commit_sha, date, scope,
                    content_digest, source_identity, created_epoch, record_digest_v1)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            )?;
            for (d, id, identity, content_digest, record_digest, exists) in &prepared {
                if *exists {
                    continue;
                }
                let commit = if d.kind == "commit" {
                    d.sha.clone()
                } else {
                    String::new()
                };
                let importance = d.importance.clamp(0.0, 1.0);
                let epoch = iso_to_epoch(&d.date).unwrap_or(0);
                stmt.execute(params![
                    id,
                    d.kind,
                    d.subject,
                    d.body,
                    importance,
                    d.source,
                    d.author,
                    commit,
                    d.date,
                    scope,
                    content_digest,
                    identity,
                    epoch,
                    record_digest
                ])?;
            }
        }
        tx.commit()?;
        Ok(decisions.len())
    }

    pub fn count_for_scope(&self, scope: &str) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM decisions WHERE scope=?1 AND superseded_by IS NULL
               AND (valid_from IS NULL OR unixepoch(valid_from) <= unixepoch('now'))
               AND (valid_until IS NULL OR unixepoch(valid_until) > unixepoch('now'))",
            params![scope],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// Record explicit retrieval feedback on a decision, closing the usage-to-quality
    /// loop. A helpful verdict raises the record's effectiveness and a not-helpful verdict lowers
    /// it. The delta lands on the effective value (ungraded prior 0.5), clamped to
    /// `[0.01, 1.0]`, and `updated_at` is
    /// bumped so the verdict also moves recency. Returns the new effectiveness, or `None` when the
    /// id is unknown or superseded.
    pub fn feedback(&self, id: &str, helpful: bool) -> Result<Option<f64>> {
        let delta = if helpful { 0.05 } else { -0.03 };
        let now = epoch_to_iso(now_epoch());
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let updated = tx.execute(
            "UPDATE decisions SET
               times_helpful = COALESCE(times_helpful, 0) + ?1,
               effectiveness = MIN(1.0, MAX(0.01, effectiveness + ?2)),
               updated_at = ?3
             WHERE id = ?4 AND superseded_by IS NULL
               AND (valid_from IS NULL OR unixepoch(valid_from) <= unixepoch(?3))
               AND (valid_until IS NULL OR unixepoch(valid_until) > unixepoch(?3))",
            params![if helpful { 1 } else { 0 }, delta, now, id],
        )?;
        if updated == 0 {
            tx.rollback()?;
            return Ok(None);
        }
        let mut logged = false;
        for _ in 0..4 {
            if tx.execute(
                "INSERT OR IGNORE INTO feedback_log
                   (id, memory_id, helpful, delta, created_at)
                 VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3, ?4)",
                params![id, if helpful { 1 } else { 0 }, delta, now],
            )? == 1
            {
                logged = true;
                break;
            }
        }
        anyhow::ensure!(logged, "could not allocate a unique feedback log identity");
        let eff: f64 = tx.query_row(
            "SELECT effectiveness FROM decisions WHERE id=?1",
            params![id],
            |r| r.get(0),
        )?;
        tx.commit()?;
        Ok(Some(eff))
    }
}

fn inspect_connection(conn: &Connection) -> StoreCompatibility {
    inspect_connection_inner(conn).unwrap_or_else(|_| StoreCompatibility::Incompatible {
        code: StoreCompatibilityErrorCode::SchemaCorrupt,
        message: "store schema could not be read safely".to_owned(),
        found_version: None,
    })
}

fn immutable_sqlite_uri(path: &Path) -> String {
    let mut uri = String::from("file:");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b':' | b'.' | b'_' | b'-' | b'~' => {
                uri.push(char::from(byte))
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri.push_str("?immutable=1");
    uri
}

fn store_may_have_live_wal(path: &Path) -> Result<bool> {
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", path.display()));
        if sidecar.exists() {
            return Ok(true);
        }
    }
    let mut header = [0_u8; 20];
    let mut file = std::fs::File::open(path)?;
    if file.read(&mut header)? < header.len() || &header[..16] != b"SQLite format 3\0" {
        return Ok(false);
    }
    Ok(header[18] == 2 || header[19] == 2)
}

fn require_store_instance_id(store_instance_id: Option<&str>) -> Result<&str> {
    let Some(store_instance_id) = store_instance_id else {
        return Err(StoreIdentityBindingError {
            code: StoreIdentityBindingErrorCode::IdentityRequired,
            message: "an explicit provider store identity is required for initial binding",
        }
        .into());
    };
    if !valid_store_instance_id(store_instance_id) {
        return Err(StoreIdentityBindingError {
            code: StoreIdentityBindingErrorCode::InvalidIdentity,
            message: "provider store identity must be 1 to 128 safe ASCII bytes",
        }
        .into());
    }
    Ok(store_instance_id)
}

fn verify_store_binding(identity: &StoreIdentity, expected: Option<&str>) -> Result<()> {
    if let Some(expected) = expected {
        require_store_instance_id(Some(expected))?;
        if identity.store_instance_id != expected {
            return Err(StoreIdentityBindingError {
                code: StoreIdentityBindingErrorCode::IdentityMismatch,
                message: "provider store identity does not match the bound store",
            }
            .into());
        }
    }
    Ok(())
}

fn inspect_connection_inner(conn: &Connection) -> Result<StoreCompatibility> {
    let version: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let object_count: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_schema
         WHERE name NOT LIKE 'sqlite_%' AND name NOT LIKE 'decisions_fts_%'",
        [],
        |row| row.get(0),
    )?;
    let has_metadata = object_exists(conn, "table", "open_why_metadata")?;
    let has_ledger = object_exists(conn, "table", "open_why_migrations")?;

    if version == 0 {
        if has_metadata || has_ledger {
            return Ok(incompatible(
                StoreCompatibilityErrorCode::PartialMigration,
                "store contains partial identity migration state",
                Some(version),
            ));
        }
        if object_count == 0 {
            return Ok(StoreCompatibility::Uninitialized);
        }
        if schema_sha256_on(conn)? == expected_legacy_schema_sha256_v0()? {
            return Ok(StoreCompatibility::MigrationRequired {
                from: 0,
                to: STORE_SCHEMA_VERSION,
                plan_digest: migration_plan_digest(),
            });
        }
        return Ok(incompatible(
            StoreCompatibilityErrorCode::ShapeDrift,
            "database is not a recognized open-why store",
            Some(version),
        ));
    }
    if version > STORE_SCHEMA_VERSION {
        return Ok(incompatible(
            StoreCompatibilityErrorCode::SchemaNewer,
            "store schema is newer than this open-why build",
            Some(version),
        ));
    }
    if version != STORE_SCHEMA_VERSION || !has_metadata || !has_ledger {
        return Ok(incompatible(
            StoreCompatibilityErrorCode::PartialMigration,
            "store identity migration is incomplete",
            Some(version),
        ));
    }

    let metadata_count: i64 =
        conn.query_row("SELECT count(*) FROM open_why_metadata", [], |row| {
            row.get(0)
        })?;
    if metadata_count != 1 {
        return Ok(incompatible(
            StoreCompatibilityErrorCode::SchemaCorrupt,
            "store identity metadata is not a singleton",
            Some(version),
        ));
    }
    let (family, metadata_version, stored_schema, stored_plan, store_instance_id): (
        String,
        u32,
        String,
        String,
        String,
    ) = conn.query_row(
        "SELECT schema_family,schema_version,schema_sha256,migration_plan_digest,
                store_instance_id
         FROM open_why_metadata WHERE singleton=1",
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    if metadata_version > STORE_SCHEMA_VERSION {
        return Ok(incompatible(
            StoreCompatibilityErrorCode::SchemaNewer,
            "store metadata is newer than this open-why build",
            Some(metadata_version),
        ));
    }
    if family != STORE_SCHEMA_FAMILY || metadata_version != version {
        return Ok(incompatible(
            StoreCompatibilityErrorCode::SchemaCorrupt,
            "store schema identity metadata is inconsistent",
            Some(version),
        ));
    }
    if !valid_store_instance_id(&store_instance_id) {
        return Ok(incompatible(
            StoreCompatibilityErrorCode::SchemaCorrupt,
            "store instance identity is invalid",
            Some(version),
        ));
    }
    if stored_plan != migration_plan_digest() {
        return Ok(incompatible(
            StoreCompatibilityErrorCode::ChecksumMismatch,
            "store migration plan digest does not match",
            Some(version),
        ));
    }
    let mut stmt = conn.prepare(
        "SELECT sequence,migration_id,checksum_sha256
         FROM open_why_migrations ORDER BY sequence",
    )?;
    let ledger = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, usize>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if ledger.len() != MIGRATION_STEPS.len() {
        return Ok(incompatible(
            StoreCompatibilityErrorCode::PartialMigration,
            "store migration ledger is incomplete",
            Some(version),
        ));
    }
    for (index, ((sequence, id, checksum), (expected_id, specification))) in
        ledger.iter().zip(MIGRATION_STEPS).enumerate()
    {
        if *sequence != index + 1
            || id != expected_id
            || checksum != &sha256_hex(specification.as_bytes())
        {
            return Ok(incompatible(
                StoreCompatibilityErrorCode::ChecksumMismatch,
                "store migration ledger checksum does not match",
                Some(version),
            ));
        }
    }
    let expected_schema = expected_schema_sha256_v1()?;
    if !required_shape_is_valid(conn)?
        || stored_schema != expected_schema
        || schema_sha256_on(conn)? != expected_schema
    {
        return Ok(incompatible(
            StoreCompatibilityErrorCode::ShapeDrift,
            "store schema shape does not match its declared identity",
            Some(version),
        ));
    }

    Ok(StoreCompatibility::Compatible {
        identity: StoreIdentity {
            store_instance_id,
            schema_family: STORE_SCHEMA_FAMILY,
            schema_version: STORE_SCHEMA_VERSION,
            schema_sha256: stored_schema,
        },
    })
}

fn incompatible(
    code: StoreCompatibilityErrorCode,
    message: &str,
    found_version: Option<u32>,
) -> StoreCompatibility {
    StoreCompatibility::Incompatible {
        code,
        message: message.to_owned(),
        found_version,
    }
}

fn object_exists(conn: &Connection, kind: &str, name: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE type=?1 AND name=?2",
        params![kind, name],
        |row| row.get(0),
    )?;
    Ok(count == 1)
}

fn required_shape_is_valid(conn: &Connection) -> Result<bool> {
    for (kind, name) in REQUIRED_OBJECTS {
        if !object_exists(conn, kind, name)? {
            return Ok(false);
        }
    }
    let expected: HashSet<&str> = REQUIRED_DECISION_COLUMNS.iter().copied().collect();
    let mut stmt = conn.prepare("SELECT name FROM pragma_table_info('decisions')")?;
    let actual = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    if actual.len() != expected.len() || !expected.iter().all(|name| actual.contains(*name)) {
        return Ok(false);
    }
    for (table, expected_columns) in [
        (
            "decision_git_refs",
            &["commit_hash", "commit_subject", "created_at", "decision_id"][..],
        ),
        (
            "feedback_log",
            &["created_at", "delta", "helpful", "id", "memory_id"][..],
        ),
        (
            "open_why_metadata",
            &[
                "migration_plan_digest",
                "schema_family",
                "schema_sha256",
                "schema_version",
                "singleton",
                "store_instance_id",
            ][..],
        ),
        (
            "open_why_migrations",
            &["applied_at", "checksum_sha256", "migration_id", "sequence"][..],
        ),
    ] {
        let sql = format!("SELECT name FROM pragma_table_info('{table}') ORDER BY name");
        let mut stmt = conn.prepare(&sql)?;
        let columns = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if columns != expected_columns {
            return Ok(false);
        }
    }
    Ok(true)
}

fn schema_sha256_on(conn: &Connection) -> Result<String> {
    let mut canonical = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT type,name,tbl_name,COALESCE(sql,'') FROM sqlite_schema
         WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name,tbl_name",
    )?;
    let objects = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (kind, name, table, sql) in objects {
        append_required(&mut canonical, "object_type", kind.as_bytes());
        append_required(&mut canonical, "object_name", name.as_bytes());
        append_required(&mut canonical, "object_table", table.as_bytes());
        append_required(
            &mut canonical,
            "object_sql",
            normalize_schema_sql(&sql).as_bytes(),
        );
    }
    let mut tables = conn.prepare(
        "SELECT name FROM sqlite_schema
         WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;
    let tables = tables
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for table in tables {
        append_required(&mut canonical, "foreign_key_table", table.as_bytes());
        let escaped = table.replace('\'', "''");
        let mut foreign_keys = conn.prepare(&format!(
            "SELECT id,seq,\"table\",\"from\",\"to\",on_update,on_delete,match
             FROM pragma_foreign_key_list('{escaped}') ORDER BY id,seq"
        ))?;
        let rows = foreign_keys
            .query_map([], |row| {
                Ok(format!(
                    "{}|{}|{}|{}|{}|{}|{}|{}",
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for row in rows {
            append_required(&mut canonical, "foreign_key", row.as_bytes());
        }
    }
    Ok(sha256_hex(&canonical))
}

fn expected_schema_sha256_v1() -> Result<String> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(CORE_SCHEMA_V1_SQL)?;
    conn.execute_batch(FEEDBACK_SCHEMA_V1_SQL)?;
    conn.execute_batch(FTS_SCHEMA_V1_SQL)?;
    conn.execute_batch(FTS_TRIGGERS_V1_SQL)?;
    conn.execute_batch(IDENTITY_SCHEMA_V1_SQL)?;
    conn.execute_batch(IDENTITY_TRIGGERS_V1_SQL)?;
    schema_sha256_on(&conn)
}

fn expected_legacy_schema_sha256_v0() -> Result<String> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(LEGACY_SCHEMA_V0_SQL)?;
    schema_sha256_on(&conn)
}

fn normalize_schema_sql(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn migration_plan_digest() -> String {
    let mut canonical = Vec::new();
    for (id, specification) in MIGRATION_STEPS {
        append_required(&mut canonical, "migration_id", id.as_bytes());
        append_required(
            &mut canonical,
            "checksum_sha256",
            sha256_hex(specification.as_bytes()).as_bytes(),
        );
    }
    sha256_hex(&canonical)
}

fn valid_store_instance_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_STORE_INSTANCE_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn valid_evidence_identity_shape(identity: &EvidenceIdentity) -> bool {
    identity.contract == EVIDENCE_IDENTITY_CONTRACT
        && identity.record_digest_contract == RECORD_DIGEST_CONTRACT
        && valid_store_instance_id(&identity.store_instance_id)
        && !identity.scope.is_empty()
        && identity.scope.len() <= MAX_COMMIT_LINK_SCOPE_BYTES
        && !identity.record_id.is_empty()
        && identity.record_id.len() <= MAX_COMMIT_LINK_RECORD_ID_BYTES
        && identity.record_digest.len() == 64
        && identity
            .record_digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn scoped_commit_link_error(
    code: ScopedCommitLinkErrorCode,
    retryable: bool,
) -> ScopedCommitLinkResolution {
    let message = match code {
        ScopedCommitLinkErrorCode::InvalidRequest => "commit link request is invalid",
        ScopedCommitLinkErrorCode::EvidenceUnavailable => "sealed evidence identity is unavailable",
        ScopedCommitLinkErrorCode::LinkConflict => {
            "commit link already exists with a different subject"
        }
        ScopedCommitLinkErrorCode::StoreUnavailable => "commit link store is unavailable",
    };
    ScopedCommitLinkResolution::Error {
        contract: SCOPED_COMMIT_LINK_WRITE_CONTRACT,
        code,
        message: message.to_owned(),
        retryable,
    }
}

pub(crate) fn store_error_is_retryable(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<rusqlite::Error>(),
            Some(rusqlite::Error::SqliteFailure(inner, _))
                if matches!(
                    inner.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                )
        )
    })
}

fn record_digest_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RecordDigestRow> {
    Ok(RecordDigestRow {
        id: row.get(0)?,
        scope: row.get(1)?,
        kind: row.get(2)?,
        title: row.get(3)?,
        content: row.get(4)?,
        importance: row.get(5)?,
        source: row.get(6)?,
        author: row.get(7)?,
        commit_sha: row.get(8)?,
        date: row.get(9)?,
        tags: row.get(10)?,
        fact_key: row.get(11)?,
        valid_from: row.get(12)?,
        declared_valid_until: row.get(13)?,
        sealed_digest: row.get(14)?,
    })
}

fn record_digest_row_from_external(row: &ExternalDecision) -> RecordDigestRow {
    RecordDigestRow {
        id: row.id.clone(),
        scope: row.scope.clone(),
        kind: row.kind.clone(),
        title: row.title.clone(),
        content: row.content.clone(),
        importance: row.importance.clamp(0.0, 1.0),
        source: row.source.clone(),
        author: row.author.clone(),
        commit_sha: String::new(),
        date: row.date.clone(),
        tags: row.tags.clone(),
        fact_key: row.fact_key.clone(),
        valid_from: row.valid_from.clone(),
        declared_valid_until: row.valid_until.clone(),
        sealed_digest: None,
    }
}

fn ensure_exact_record_replay(
    existing: &RecordDigestRow,
    candidate: &RecordDigestRow,
) -> Result<()> {
    let stored_digest = record_digest_v1(existing)?;
    let candidate_digest = record_digest_v1(candidate)?;
    if existing.sealed_digest.as_deref() != Some(stored_digest.as_str())
        || existing.sealed_digest.as_deref() != Some(candidate_digest.as_str())
    {
        return Err(RecordIdentityConflict.into());
    }
    Ok(())
}

struct PendingRetirement {
    retirement_at: String,
    expected_valid_from: Option<String>,
    expected_valid_until: Option<String>,
}

fn pending_retirement_time_on(
    conn: &Connection,
    id: &str,
    scope: &str,
    successor_id: &str,
    requested_epoch: i64,
) -> Result<Option<PendingRetirement>> {
    let state: Option<(Option<String>, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT valid_from,superseded_by,valid_until
             FROM decisions WHERE id=?1 AND scope=?2",
            params![id, scope],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((valid_from, superseded_by, valid_until)) = state else {
        return Err(SupersessionTargetNotFound.into());
    };
    if superseded_by.as_deref() == Some(successor_id) {
        return Ok(None);
    }
    if superseded_by.is_some() {
        return Err(SupersessionConflict.into());
    }
    ensure_acyclic_retirement_on(conn, successor_id, id, scope)?;
    let retirement_epoch = match valid_from.as_deref() {
        Some(value) => {
            let from = iso_to_epoch(value).ok_or(CurrentRecordErrorCode::InvalidTemporalData)?;
            let after_from = from
                .checked_add(1)
                .ok_or(CurrentRecordErrorCode::InvalidTemporalData)?;
            requested_epoch.max(after_from)
        }
        None => requested_epoch,
    };
    let retirement_at = epoch_to_iso(retirement_epoch);
    if iso_to_epoch(&retirement_at) != Some(retirement_epoch) {
        return Err(CurrentRecordErrorCode::InvalidTemporalData.into());
    }
    Ok(Some(PendingRetirement {
        retirement_at,
        expected_valid_from: valid_from,
        expected_valid_until: valid_until,
    }))
}

fn ensure_acyclic_retirement_on(
    conn: &Connection,
    successor_id: &str,
    predecessor_id: &str,
    scope: &str,
) -> Result<()> {
    if successor_id == predecessor_id {
        return Err(SupersessionCycle.into());
    }

    let mut cursor = successor_id.to_owned();
    let mut seen = std::collections::HashSet::new();
    for depth in 0..MAX_SUPERSESSION_CHAIN {
        if cursor == predecessor_id || !seen.insert(cursor.clone()) {
            return Err(SupersessionCycle.into());
        }
        let next: Option<Option<Vec<u8>>> = conn
            .query_row(
                "SELECT CAST(superseded_by AS BLOB)
                 FROM decisions WHERE id=?1 AND scope=?2",
                params![cursor, scope],
                |row| row.get(0),
            )
            .optional()?;
        let Some(next) = next else {
            if depth == 0 {
                return Ok(());
            }
            return Err(SupersessionCycle.into());
        };
        if depth + 1 >= MAX_SUPERSESSION_CHAIN {
            return Err(SupersessionCycle.into());
        }
        let Some(next) = next else {
            return Ok(());
        };
        let next = String::from_utf8(next).map_err(|_| SupersessionCycle)?;
        if next.is_empty() {
            return Ok(());
        }
        cursor = next;
    }
    Err(SupersessionCycle.into())
}

fn record_digest_v1(row: &RecordDigestRow) -> Result<String> {
    anyhow::ensure!(row.importance.is_finite(), "importance must be finite");
    let mut canonical = Vec::new();
    append_required(
        &mut canonical,
        "contract",
        RECORD_DIGEST_CONTRACT.as_bytes(),
    );
    append_required(&mut canonical, "repository_scope", row.scope.as_bytes());
    append_required(&mut canonical, "record_id", row.id.as_bytes());
    append_required(&mut canonical, "kind", row.kind.as_bytes());
    append_required(&mut canonical, "title", row.title.as_bytes());
    append_required(&mut canonical, "content", row.content.as_bytes());
    let importance = if row.importance == 0.0 {
        0.0
    } else {
        row.importance
    };
    append_required(
        &mut canonical,
        "importance_f64_be",
        &importance.to_bits().to_be_bytes(),
    );
    append_required(&mut canonical, "source", row.source.as_bytes());
    append_required(&mut canonical, "author", row.author.as_bytes());
    append_required(&mut canonical, "commit_sha", row.commit_sha.as_bytes());
    append_required(&mut canonical, "observed_at", row.date.as_bytes());
    append_tags(&mut canonical, row.tags.as_deref())?;
    append_optional(&mut canonical, "fact_key", row.fact_key.as_deref());
    append_optional(
        &mut canonical,
        "declared_valid_from",
        row.valid_from.as_deref(),
    );
    append_optional(
        &mut canonical,
        "declared_valid_until",
        row.declared_valid_until.as_deref(),
    );
    Ok(sha256_hex(&canonical))
}

fn append_tags(canonical: &mut Vec<u8>, raw: Option<&str>) -> Result<()> {
    append_required(canonical, "tags", &[]);
    match raw {
        None => canonical.push(0),
        Some(raw) => {
            canonical.push(1);
            let mut tags: Vec<String> =
                serde_json::from_str(raw).context("tags must be a JSON array")?;
            tags.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
            canonical.extend_from_slice(&u64::try_from(tags.len())?.to_be_bytes());
            for tag in tags {
                canonical.extend_from_slice(&u64::try_from(tag.len())?.to_be_bytes());
                canonical.extend_from_slice(tag.as_bytes());
            }
        }
    }
    Ok(())
}

fn append_required(canonical: &mut Vec<u8>, name: &str, value: &[u8]) {
    canonical.extend_from_slice(&(name.len() as u64).to_be_bytes());
    canonical.extend_from_slice(name.as_bytes());
    canonical.push(1);
    canonical.extend_from_slice(&(value.len() as u64).to_be_bytes());
    canonical.extend_from_slice(value);
}

fn append_optional(canonical: &mut Vec<u8>, name: &str, value: Option<&str>) {
    canonical.extend_from_slice(&(name.len() as u64).to_be_bytes());
    canonical.extend_from_slice(name.as_bytes());
    match value {
        None => canonical.push(0),
        Some(value) => {
            canonical.push(1);
            canonical.extend_from_slice(&(value.len() as u64).to_be_bytes());
            canonical.extend_from_slice(value.as_bytes());
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn scoped_current_resolution(read: CurrentEvidenceRead) -> ScopedCurrentRecordResolution {
    match read.resolution {
        CurrentRecordResolution::Ok {
            contract: _,
            as_of,
            requested_id,
            current_id,
            record,
            git_refs,
            supersession_chain,
        } => match read.identity {
            Some(evidence_identity) => ScopedCurrentRecordResolution::Ok {
                contract: SCOPED_CURRENT_EVIDENCE_CONTRACT,
                as_of,
                requested_id,
                current_id,
                record,
                git_refs,
                supersession_chain,
                evidence_identity,
            },
            None => ScopedCurrentRecordResolution::Error {
                contract: SCOPED_CURRENT_EVIDENCE_CONTRACT,
                as_of,
                requested_id,
                code: ScopedCurrentEvidenceErrorCode::IdentityConflict,
                message: "current record identity conflicts with its sealed evidence".to_owned(),
                retryable: false,
            },
        },
        CurrentRecordResolution::Error {
            contract: _,
            as_of,
            requested_id,
            code,
            message,
            retryable,
        } => ScopedCurrentRecordResolution::Error {
            contract: SCOPED_CURRENT_EVIDENCE_CONTRACT,
            as_of,
            requested_id,
            code: match code {
                CurrentRecordErrorCode::NotFound => ScopedCurrentEvidenceErrorCode::NotFound,
                CurrentRecordErrorCode::NotYetValid => ScopedCurrentEvidenceErrorCode::NotYetValid,
                CurrentRecordErrorCode::ExpiredWithoutSuccessor => {
                    ScopedCurrentEvidenceErrorCode::ExpiredWithoutSuccessor
                }
                CurrentRecordErrorCode::BrokenChain => ScopedCurrentEvidenceErrorCode::BrokenChain,
                CurrentRecordErrorCode::Cycle => ScopedCurrentEvidenceErrorCode::Cycle,
                CurrentRecordErrorCode::TraversalLimit => {
                    ScopedCurrentEvidenceErrorCode::TraversalLimit
                }
                CurrentRecordErrorCode::InvalidTemporalData => {
                    ScopedCurrentEvidenceErrorCode::InvalidTemporalData
                }
            },
            message,
            retryable,
        },
    }
}

/// Per-result ranking explanation for why a row ranked where it did. Exposed by `--explain`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RankExplanation {
    pub similarity: f64,
    pub importance: f64,
    pub effectiveness: f64,
    pub age_days: f64,
    pub recency_decay: f64,
    pub hybrid_score: f64,
    pub semantic_rank: Option<usize>,
    pub lexical_rank: Option<usize>,
    pub rrf_score: f64,
}

/// A search result set paired with its ranking explanation, as returned by the `--explain`
/// and `--explain-drops` paths.
pub type Explained = Vec<(Record, RankExplanation)>;

fn parse_embedding(raw: Option<String>) -> Option<Vec<f32>> {
    raw.and_then(|s| serde_json::from_str::<Vec<f32>>(&s).ok())
}

fn digest(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn iso_to_epoch(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() > MAX_TEMPORAL_VALUE_BYTES {
        return None;
    }
    let canonical_suffix = match b.len() {
        20 => b[19] == b'Z',
        22.. => {
            b[19] == b'.'
                && b.last() == Some(&b'Z')
                && b[20..b.len() - 1].iter().all(u8::is_ascii_digit)
        }
        _ => false,
    };
    if !canonical_suffix
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    const DIGIT_POSITIONS: [usize; 14] = [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18];
    if !DIGIT_POSITIONS
        .iter()
        .all(|position| b[*position].is_ascii_digit())
    {
        return None;
    }
    let n = |i: usize| i64::from(b[i] - b'0');
    let y = n(0) * 1000 + n(1) * 100 + n(2) * 10 + n(3);
    let mo = n(5) * 10 + n(6);
    let d = n(8) * 10 + n(9);
    let h = n(11) * 10 + n(12);
    let mi = n(14) * 10 + n(15);
    let se = n(17) * 10 + n(18);
    if !(1970..=9999).contains(&y) || !(1..=12).contains(&mo) || h > 23 || mi > 59 || se > 59 {
        return None;
    }
    let leap_year = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days_in_month = match mo {
        2 if leap_year => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if !(1..=days_in_month).contains(&d) {
        return None;
    }
    let days = days_from_civil(y, mo as u32, d as u32);
    Some(days * 86_400 + h * 3600 + mi * 60 + se)
}

pub(crate) fn epoch_to_iso(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let h = rem / 3600;
    let mi = (rem % 3600) / 60;
    let se = rem % 60;
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{se:02}Z")
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests;
