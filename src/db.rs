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
    #[cfg(unix)]
    let inspect_flags = inspect_flags | OpenFlags::SQLITE_OPEN_NOFOLLOW;
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
        let open_flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
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

/// RRF fusion constant (Cormack et al. 2009).
const RRF_K: f64 = 60.0;
/// BM25 leads the inline fusion (arXiv 2605.15184, Table 1).
const BM25_WEIGHT: f64 = 1.5;
/// Calibrated hybrid rerank weights (similarity / importance / effectiveness).
const RERANK_W_SIM: f64 = 0.65;
const RERANK_W_IMPORTANCE: f64 = 0.25;
const RERANK_W_EFFECTIVENESS: f64 = 0.10;
/// Floor under recency decay: an old-but-best match
/// must stay reachable rather than being buried to zero by age alone.
const RECENCY_DECAY_FLOOR: f64 = 0.3;
const RECENCY_HALF_LIFE_DAYS: f64 = 7.0;
const RECENCY_HALF_LIFE_DECISION_DAYS: f64 = 2.0;
/// Query-conditional recency weighting.
const RECENCY_BOOST: f64 = 2.5;
const RECENCY_SUPPRESS: f64 = 0.3;

/// Ebbinghaus recency decay with a floor: `2^(-age/halfLife)`, clamped at RECENCY_DECAY_FLOOR.
fn recency_decay(age_days: f64, half_life_days: f64) -> f64 {
    if half_life_days <= 0.0 || half_life_days.is_nan() || !age_days.is_finite() {
        return RECENCY_DECAY_FLOOR;
    }
    (2.0f64.powf(-age_days.max(0.0) / half_life_days)).max(RECENCY_DECAY_FLOOR)
}

fn contains_word(haystack: &str, word: &str) -> bool {
    haystack
        .split(|c: char| !c.is_alphanumeric())
        .any(|t| t == word)
}

/// Query-conditional recency multiplier. Word-boundary match (via tokenization) so `now` does
/// not match inside `snow`, `as of` / `used to` are phrase matches.
fn recency_weight_for(query: &str) -> f64 {
    let lower = query.to_lowercase();
    const CURRENT_WORDS: &[&str] = &["current", "currently", "latest", "now", "today", "present"];
    const PAST_WORDS: &[&str] = &[
        "originally",
        "first",
        "initial",
        "initially",
        "previously",
        "formerly",
        "history",
        "historical",
        "past",
        "earlier",
        "before",
    ];
    const CURRENT_PHRASES: &[&str] = &["as of", "most recent", "up to date", "up-to-date"];
    const PAST_PHRASES: &[&str] = &["used to"];
    if CURRENT_PHRASES.iter().any(|p| lower.contains(p))
        || CURRENT_WORDS.iter().any(|w| contains_word(&lower, w))
    {
        return RECENCY_BOOST;
    }
    if PAST_PHRASES.iter().any(|p| lower.contains(p))
        || PAST_WORDS.iter().any(|w| contains_word(&lower, w))
    {
        return RECENCY_SUPPRESS;
    }
    1.0
}

/// The fields `rank_by` needs per row. References borrow from the row for the duration of the
/// scoring pass only; only primitives are copied out.
struct RankRow<'a> {
    importance: f64,
    kind: &'a str,
    date: &'a str,
    updated_at: Option<&'a str>,
    access_count: i64,
    effectiveness: f64,
    embedding: Option<&'a [f32]>,
    title: &'a str,
    content: &'a str,
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

/// Hybrid rerank using reciprocal-rank fusion of a
/// semantic arm (sorted by hybrid score) and a lexical arm (the FTS5 `bm25()` order supplied by
/// the caller, already narrow-then-broad), then slice. Recency enters through the semantic arm's
/// hybrid score. It is floored so age cannot bury a best match and never acts as a multiplicative gate on
/// the fused score.
fn rank(
    query: &str,
    query_embedding: Option<&[f32]>,
    rows: Vec<Decision>,
    lexical_order: Vec<usize>,
    now: i64,
    limit: usize,
) -> Vec<Decision> {
    rank_by(
        query,
        query_embedding,
        rows,
        lexical_order,
        now,
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
            title: &d.subject,
            content: &d.body,
        },
    )
    .0
}

fn rank_by<T>(
    query: &str,
    query_embedding: Option<&[f32]>,
    rows: Vec<T>,
    lexical_order: Vec<usize>,
    now: i64,
    limit: usize,
    fields: impl Fn(&T) -> RankRow<'_>,
) -> (Vec<T>, Vec<RankExplanation>) {
    let recency_mult = recency_weight_for(query);

    // Semantic score capsule. The lexical arm is the native FTS5 bm25() order supplied by the
    // caller; this computes only what the semantic arm needs.
    struct Capsule {
        sim: f64,
        embedded: bool,
        importance: f64,
        age_days: f64,
        half_life: f64,
        access_count: i64,
        effectiveness: f64,
        lexical_gate_score: f64,
    }
    let has_query_emb = query_embedding.is_some();
    let capsules: Vec<Capsule> = rows
        .iter()
        .map(|d| {
            let f = fields(d);
            let (sim, embedded) = match (query_embedding, f.embedding) {
                (Some(q), Some(e)) => (crate::embed::cosine(q, e) as f64, true),
                _ => (0.0, false),
            };
            let age_src = f.updated_at.unwrap_or(f.date);
            let age_days = iso_to_epoch(age_src)
                .map(|ep| ((now - ep) as f64 / 86_400.0).max(0.0))
                .unwrap_or(0.0);
            let half_life = if f.kind == "decision" {
                RECENCY_HALF_LIFE_DECISION_DAYS
            } else {
                RECENCY_HALF_LIFE_DAYS
            };
            let lexical_text = if f.title.is_empty() {
                f.content.to_string()
            } else {
                format!("{}\n{}", f.title, f.content)
            };
            let lexical_gate_score =
                crate::relevance::lexical_score(query, f.content, &lexical_text);
            Capsule {
                sim,
                embedded,
                importance: f.importance,
                age_days,
                half_life,
                access_count: f.access_count,
                effectiveness: f.effectiveness,
                lexical_gate_score,
            }
        })
        .collect();

    let hybrid = |c: &Capsule| -> f64 {
        // Ebbinghaus with spaced-repetition stability: more accesses widen the half-life, so a
        // frequently-surfaced memory decays slower than its raw age would suggest.
        let stability = c.half_life * (1.0 + (1.0 + c.access_count as f64).ln());
        let decay = recency_decay(c.age_days, stability);
        (RERANK_W_SIM * c.sim
            + RERANK_W_IMPORTANCE * c.importance
            + RERANK_W_EFFECTIVENESS * c.effectiveness)
            * decay
            * recency_mult
    };

    let n = capsules.len();

    // Semantic arm: keep only the nearest-by-cosine rows (the semantic
    // neighbourhood), then order THAT set by hybrid score. Ordering the whole corpus by hybrid
    // score would let recency/importance crowd out semantically-far rows before fusion.
    let semantic_order: Vec<usize> = if has_query_emb {
        let mut embedded: Vec<usize> = (0..n).filter(|&i| capsules[i].embedded).collect();
        embedded.sort_by(|&a, &b| {
            capsules[b]
                .sim
                .partial_cmp(&capsules[a].sim)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let k = (limit.saturating_mul(30)).max(256);
        embedded.truncate(k);
        embedded.sort_by(|&a, &b| {
            hybrid(&capsules[b])
                .partial_cmp(&hybrid(&capsules[a]))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        embedded
    } else {
        Vec::new()
    };

    // Reciprocal rank fusion.
    let mut scores = vec![0.0f64; n];
    let mut semantic_rank: Vec<Option<usize>> = vec![None; n];
    let mut lexical_rank: Vec<Option<usize>> = vec![None; n];
    for (rank, &i) in semantic_order.iter().enumerate() {
        scores[i] += 1.0 / (RRF_K + rank as f64 + 1.0);
        semantic_rank[i] = Some(rank);
    }
    for (rank, &i) in lexical_order.iter().enumerate() {
        scores[i] += BM25_WEIGHT / (RRF_K + rank as f64 + 1.0);
        lexical_rank[i] = Some(rank);
    }

    if std::env::var("OPEN_WHY_DEBUG_RANK").is_ok() {
        eprintln!(
            "[rank] query={query} semantic={} lexical={}",
            semantic_order.len(),
            lexical_order.len()
        );
        for (rank, &i) in semantic_order.iter().take(12).enumerate() {
            let c = &capsules[i];
            eprintln!(
                "  SEM[{rank}] sim={:.3} imp={:.2} age={:.0} fused={:.5}",
                c.sim, c.importance, c.age_days, scores[i]
            );
        }
        for (rank, &i) in lexical_order.iter().take(12).enumerate() {
            let c = &capsules[i];
            eprintln!(
                "  LEX[{rank}] sim={:.3} lex_gate={:.4} fused={:.5}",
                c.sim, c.lexical_gate_score, scores[i]
            );
        }
    }

    // Fused candidate set = union of the two arms, best-fused first.
    let mut order: Vec<usize> = semantic_order
        .iter()
        .copied()
        .chain(lexical_order.iter().copied())
        .collect();
    order.sort_unstable();
    order.dedup();
    // Post-fusion relevance gate: drop candidates
    // that cleared BM25/RRF fusion but are not actually relevant to the query, before the
    // final score sort, so a filtered-out noise row can't block a genuine match from the
    // top-N slice. Must run on the full fused set, not just the eventual top `limit`.
    order.retain(|&i| crate::relevance::passes(capsules[i].sim, capsules[i].lexical_gate_score));
    order.sort_by(|&a, &b| {
        scores[b]
            .partial_cmp(&scores[a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    order.truncate(limit);

    let mut row_vec: Vec<Option<T>> = rows.into_iter().map(Some).collect();
    let mut out = Vec::with_capacity(order.len());
    let mut explanations = Vec::with_capacity(order.len());
    for i in order {
        if let Some(r) = row_vec[i].take() {
            let c = &capsules[i];
            let stability = c.half_life * (1.0 + (1.0 + c.access_count as f64).ln());
            let decay = recency_decay(c.age_days, stability);
            out.push(r);
            explanations.push(RankExplanation {
                similarity: c.sim,
                importance: c.importance,
                effectiveness: c.effectiveness,
                age_days: c.age_days,
                recency_decay: decay,
                hybrid_score: hybrid(c),
                semantic_rank: semantic_rank[i],
                lexical_rank: lexical_rank[i],
                rrf_score: scores[i],
            });
        }
    }
    (out, explanations)
}

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
mod tests {
    use super::*;
    use crate::embed::{cosine, Embedder};
    use crate::store::Decision;

    struct FakeEmbedder;
    impl Embedder for FakeEmbedder {
        fn embed(&self, text: &str) -> Result<Vec<f32>> {
            Ok(match text {
                "cat" | "feline" => vec![1.0, 0.0],
                "dog" => vec![0.0, 1.0],
                _ => vec![0.0, 0.0],
            })
        }
    }

    fn decision(
        subject: &str,
        body: &str,
        importance: f64,
        embedding: Option<Vec<f32>>,
    ) -> Decision {
        Decision {
            sha: String::new(),
            author: String::new(),
            date: "2026-01-01T00:00:00Z".to_string(),
            updated_at: String::new(),
            subject: subject.to_string(),
            body: body.to_string(),
            source: String::new(),
            importance,
            kind: "decision".to_string(),
            access_count: 0,
            effectiveness: 0.5,
            embedding,
        }
    }

    fn history_row(
        id: &str,
        successor: Option<&str>,
        scope: &str,
        content: &str,
    ) -> ExternalDecision {
        ExternalDecision {
            id: id.to_owned(),
            kind: "decision".to_owned(),
            title: format!("record {id}"),
            content: content.to_owned(),
            importance: 0.5,
            source: "synthetic".to_owned(),
            author: "tester".to_owned(),
            date: "2026-01-01".to_owned(),
            updated_at: None,
            accessed_count: None,
            times_injected: None,
            effectiveness: None,
            tags: None,
            scope: scope.to_owned(),
            valid_from: Some("2026-01-01T00:00:00Z".to_owned()),
            valid_until: successor.map(|_| "2026-02-01T00:00:00Z".to_owned()),
            superseded_by: successor.map(str::to_owned),
            fact_key: None,
            git_refs: vec![GitRef {
                commit_hash: format!("commit-{id}"),
                commit_subject: format!("Apply {id}"),
            }],
        }
    }

    static TMP_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn temp_store() -> Store {
        // A monotonic counter guarantees a unique dir even when parallel tests collide on the
        // same nanosecond timestamp.
        let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("open-why-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Store::open_with_embedder_and_store_instance_id(
            &dir.join("t.db"),
            None,
            &format!("provider:test:{n}"),
        )
        .unwrap()
    }

    fn evidence_identity(store: &Store, id: &str, scope: &str) -> EvidenceIdentity {
        match store.evidence_identity_in_scope(id, scope).unwrap() {
            EvidenceIdentityResolution::Ok { identity } => identity,
            resolution => panic!("expected evidence identity, got {resolution:?}"),
        }
    }

    #[test]
    fn cosine_is_bounded_and_symmetric() {
        assert_eq!(cosine(&[1.0, 0.0], &[1.0, 0.0]), 1.0);
        assert_eq!(cosine(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
        assert_eq!(cosine(&[1.0, 0.0], &[]), 0.0);
    }

    #[test]
    fn lexical_first_without_query_embedding() {
        let rows = vec![
            decision("sqlite local record", "single file", 0.5, None),
            decision("postgres", "row level security", 0.5, None),
        ];
        // FTS5 lexical arm: only row 0 matches "sqlite".
        let ranked = rank("sqlite", None, rows, vec![0], 1700000000, 10);
        assert_eq!(ranked[0].subject, "sqlite local record");
    }

    #[test]
    fn semantic_similarity_surfaces_a_row_with_no_lexical_overlap() {
        // "feline" shares no token with "cat", but its embedding matches. Semantic
        // similarity must rank it first and must not require a lexical hit.
        let rows = vec![
            decision(
                "feline",
                "a small domesticated animal",
                0.5,
                Some(vec![1.0, 0.0]),
            ),
            decision("dog", "a loyal companion", 0.5, Some(vec![0.0, 1.0])),
        ];
        let q = FakeEmbedder.embed("cat").unwrap();
        // No lexical overlap: the FTS5 arm returns nothing, the semantic arm must carry.
        let ranked = rank("cat", Some(&q), rows, Vec::new(), 1700000000, 10);
        assert_eq!(ranked[0].subject, "feline");
    }

    #[test]
    fn missing_embedding_falls_back_to_lexical_proxy() {
        // A row with no embedding still ranks via the lexical (FTS5) arm and is not dropped.
        let rows = vec![decision("postgres", "row level security", 0.5, None)];
        let ranked = rank("postgres", Some(&[1.0, 0.0]), rows, vec![0], 1700000000, 10);
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].subject, "postgres");
    }

    #[test]
    fn recency_decay_floors_at_0_3() {
        // Age must never bury a correct answer at zero; the decay asymptotes at 0.3.
        assert!((recency_decay(0.0, 7.0) - 1.0).abs() < 1e-9);
        assert!((recency_decay(1_000.0, 7.0) - RECENCY_DECAY_FLOOR).abs() < 1e-9);
        // Non-positive half-life returns the floor rather than dividing by zero.
        assert_eq!(recency_decay(10.0, 0.0), RECENCY_DECAY_FLOOR);
    }

    #[test]
    fn recency_decay_uses_spaced_repetition_stability() {
        // A frequently-accessed memory decays slower than its raw age would suggest:
        // stability = half-life × (1 + ln(1 + access_count)).
        let age = 20.0;
        let flat = recency_decay(age, 7.0); // access_count = 0
        let stability = 7.0 * (1.0 + (1.0 + 100.0f64).ln());
        let spaced = recency_decay(age, stability);
        assert!(spaced > flat, "spaced={spaced} should exceed flat={flat}");
    }

    #[test]
    fn query_conditional_recency_weights() {
        assert!((recency_weight_for("the latest lane policy") - RECENCY_BOOST).abs() < 1e-9);
        assert!((recency_weight_for("how it used to work") - RECENCY_SUPPRESS).abs() < 1e-9);
        assert!((recency_weight_for("worktree corruption") - 1.0).abs() < 1e-9);
    }

    #[test]
    fn fts5_lexical_narrow_then_broad_prefers_all_terms() {
        // FTS5 lexical arm: for a two-term query, the all-terms (AND) arm wins when it yields
        // >= min(limit, 5) rows, so the partial-match row is excluded from the lexical arm.
        let store = temp_store();
        for i in 0..5 {
            store
                .capture(
                    &decision(&format!("sqlite postgres {i}"), "both", 0.5, None),
                    "global",
                    None,
                )
                .unwrap();
        }
        store
            .capture(
                &decision("sqlite sqlite sqlite sqlite", "no second token", 0.5, None),
                "global",
                None,
            )
            .unwrap();
        let hits = store
            .search("sqlite postgres", &["global"], &[], 10)
            .unwrap();
        assert_eq!(hits.len(), 5);
        assert!(hits.iter().all(|h| h.subject.contains("postgres")));
    }

    #[test]
    fn fts5_lexical_narrow_then_broad_falls_back() {
        // Only one all-terms row (< narrow floor), so the arm broadens to OR and the
        // partial-match row still surfaces.
        let store = temp_store();
        store
            .capture(
                &decision("sqlite postgres", "both", 0.5, None),
                "global",
                None,
            )
            .unwrap();
        store
            .capture(
                &decision("sqlite", "only one term", 0.5, None),
                "global",
                None,
            )
            .unwrap();
        let hits = store
            .search("sqlite postgres", &["global"], &[], 10)
            .unwrap();
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn fts5_lexical_orders_multi_term_match_first() {
        // The row matching both query terms must outrank the row matching only one. FTS5 bm25()
        // handles idf and length normalisation natively. SQLite owns this behavior.
        let store = temp_store();
        store
            .capture(
                &decision("worktree long", &("node_modules ".repeat(300)), 0.5, None),
                "global",
                None,
            )
            .unwrap();
        store
            .capture(
                &decision("worktree corruption", "corruption", 0.5, None),
                "global",
                None,
            )
            .unwrap();
        let hits = store
            .search("worktree corruption", &["global"], &[], 10)
            .unwrap();
        assert_eq!(hits[0].subject, "worktree corruption");
    }

    #[test]
    fn feedback_moves_effectiveness_and_is_clamped() {
        let store = temp_store();
        let id = store
            .capture(
                &decision("use sqlite", "single file local-first", 0.5, None),
                "global",
                None,
            )
            .unwrap();
        // Ungraded prior is 0.5; a helpful verdict raises it by 0.05.
        let eff = store.feedback(&id, true).unwrap().unwrap();
        assert!((eff - 0.55).abs() < 1e-9, "expected 0.55, got {eff}");
        // A not-helpful verdict lowers it by 0.03.
        let eff = store.feedback(&id, false).unwrap().unwrap();
        assert!((eff - 0.52).abs() < 1e-9, "expected 0.52, got {eff}");
        // Unknown id returns None and records nothing.
        assert!(store.feedback("no-such-id", true).unwrap().is_none());
    }

    #[test]
    fn historical_mode_surfaces_supersession_chain() {
        let store = temp_store();
        store
            .capture_external(
                &decision("database choice", "sqlite", 0.5, None),
                "global",
                "aaa",
                None,
                None,
                None,
            )
            .unwrap();
        store
            .capture_external(
                &decision("database choice v2", "postgres now", 0.5, None),
                "global",
                "bbb",
                None,
                None,
                Some("aaa"),
            )
            .unwrap();
        // Active search returns only the current (non-superseded) record.
        let hits = store
            .search("sqlite postgres", &["global"], &[], 10)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].subject, "database choice v2");
        // Historical search returns both.
        let hits = store
            .search_records_with("sqlite postgres", &["global"], &[], 10, true)
            .unwrap();
        assert_eq!(hits.len(), 2);
        // The chain walks aaa -> bbb.
        let chain = store.supersession_chain("aaa", 20).unwrap();
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].id, "aaa");
        assert_eq!(chain[1].id, "bbb");
    }

    #[test]
    fn exact_capture_replay_completes_every_requested_supersession_mode() {
        let store = temp_store();

        store
            .import_external(&[history_row(
                "generated-old",
                None,
                "global",
                "generated predecessor",
            )])
            .unwrap();
        let generated = decision("generated successor", "stable body", 0.5, None);
        let generated_id = store.capture(&generated, "global", None).unwrap();
        let generated_digest = Store::record_digest_row_by_id_on(&store.conn, &generated_id)
            .unwrap()
            .unwrap()
            .sealed_digest;
        assert_eq!(
            store
                .capture(&generated, "global", Some("generated-old"))
                .unwrap(),
            generated_id
        );

        store
            .import_external(&[history_row(
                "explicit-old",
                None,
                "global",
                "explicit predecessor",
            )])
            .unwrap();
        let explicit = decision("explicit successor", "stable body", 0.5, None);
        store
            .capture_external(
                &explicit,
                "global",
                "explicit-new",
                Some("2026-03-01T00:00:00Z"),
                None,
                None,
            )
            .unwrap();
        let explicit_digest = Store::record_digest_row_by_id_on(&store.conn, "explicit-new")
            .unwrap()
            .unwrap()
            .sealed_digest;
        store
            .capture_external(
                &explicit,
                "global",
                "explicit-new",
                Some("2026-03-01T00:00:00Z"),
                None,
                Some("explicit-old"),
            )
            .unwrap();

        let keyed = decision("keyed successor", "stable body", 0.5, None);
        store
            .capture_external(
                &keyed,
                "global",
                "keyed-new",
                Some("2026-03-01T00:00:00Z"),
                Some("shared-key"),
                None,
            )
            .unwrap();
        let keyed_digest = Store::record_digest_row_by_id_on(&store.conn, "keyed-new")
            .unwrap()
            .unwrap()
            .sealed_digest;
        let mut keyed_old = history_row("keyed-old", None, "global", "keyed predecessor");
        keyed_old.title = "different title".to_owned();
        keyed_old.fact_key = Some("shared-key".to_owned());
        store.import_external(&[keyed_old]).unwrap();
        store
            .capture_external(
                &keyed,
                "global",
                "keyed-new",
                Some("2026-03-01T00:00:00Z"),
                Some("shared-key"),
                None,
            )
            .unwrap();

        let titled = decision("shared title", "stable body", 0.5, None);
        store
            .capture_external(
                &titled,
                "global",
                "titled-new",
                Some("2026-03-01T00:00:00Z"),
                None,
                None,
            )
            .unwrap();
        let titled_digest = Store::record_digest_row_by_id_on(&store.conn, "titled-new")
            .unwrap()
            .unwrap()
            .sealed_digest;
        let mut titled_old = history_row("titled-old", None, "global", "title predecessor");
        titled_old.title = "shared title".to_owned();
        store.import_external(&[titled_old]).unwrap();
        store
            .capture_external(
                &titled,
                "global",
                "titled-new",
                Some("2026-03-01T00:00:00Z"),
                None,
                None,
            )
            .unwrap();

        for (old, successor) in [
            ("generated-old", generated_id.as_str()),
            ("explicit-old", "explicit-new"),
            ("keyed-old", "keyed-new"),
            ("titled-old", "titled-new"),
        ] {
            let actual: Option<String> = store
                .conn
                .query_row(
                    "SELECT superseded_by FROM decisions WHERE id=?1",
                    [old],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(actual.as_deref(), Some(successor), "replay mode {old}");
        }
        for (id, before) in [
            (generated_id.as_str(), generated_digest),
            ("explicit-new", explicit_digest),
            ("keyed-new", keyed_digest),
            ("titled-new", titled_digest),
        ] {
            assert_eq!(
                Store::record_digest_row_by_id_on(&store.conn, id)
                    .unwrap()
                    .unwrap()
                    .sealed_digest,
                before,
                "replay rotated the sealed digest for {id}"
            );
        }
    }

    #[test]
    fn conflicting_external_capture_fails_before_any_supersession_effect() {
        let store = temp_store();
        let original = decision("shared conflict title", "sealed body", 0.5, None);
        store
            .capture_external(
                &original,
                "global",
                "sealed-new",
                Some("2026-03-01T00:00:00Z"),
                Some("shared-conflict-key"),
                None,
            )
            .unwrap();
        let sealed_before = Store::record_digest_row_by_id_on(&store.conn, "sealed-new")
            .unwrap()
            .unwrap()
            .sealed_digest;

        let explicit_old = history_row("conflict-explicit-old", None, "global", "explicit");
        let mut keyed_old = history_row("conflict-keyed-old", None, "global", "keyed");
        keyed_old.title = "different conflict title".to_owned();
        keyed_old.fact_key = Some("shared-conflict-key".to_owned());
        let mut titled_old = history_row("conflict-titled-old", None, "global", "titled");
        titled_old.title = "shared conflict title".to_owned();
        store
            .import_external(&[explicit_old, keyed_old, titled_old])
            .unwrap();

        let mut conflict = original;
        conflict.body = "changed immutable body".to_owned();
        let error = store
            .capture_external(
                &conflict,
                "global",
                "sealed-new",
                Some("2026-03-01T00:00:00Z"),
                Some("shared-conflict-key"),
                Some("conflict-explicit-old"),
            )
            .unwrap_err();
        assert!(error.downcast_ref::<RecordIdentityConflict>().is_some());

        for id in [
            "conflict-explicit-old",
            "conflict-keyed-old",
            "conflict-titled-old",
        ] {
            let successor: Option<String> = store
                .conn
                .query_row(
                    "SELECT superseded_by FROM decisions WHERE id=?1",
                    [id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(successor, None, "conflict changed relation for {id}");
        }
        assert_eq!(
            Store::record_digest_row_by_id_on(&store.conn, "sealed-new")
                .unwrap()
                .unwrap()
                .sealed_digest,
            sealed_before
        );
    }

    #[test]
    fn same_tick_exact_replay_keeps_a_positive_resolvable_interval() {
        let store = temp_store();
        let (old_id, new_id, successor) = (0..32)
            .find_map(|attempt| {
                let old_id = format!("same-tick-old-{attempt}");
                let new_id = format!("same-tick-new-{attempt}");
                let old = decision(&format!("same tick old {attempt}"), "old body", 0.5, None);
                let successor =
                    decision(&format!("same tick new {attempt}"), "new body", 0.5, None);
                store
                    .capture_external(&old, "global", &old_id, None, None, None)
                    .unwrap();
                store
                    .capture_external(&successor, "global", &new_id, None, None, None)
                    .unwrap();
                let ticks: (Option<String>, Option<String>) = store
                    .conn
                    .query_row(
                        "SELECT
                           (SELECT valid_from FROM decisions WHERE id=?1),
                           (SELECT valid_from FROM decisions WHERE id=?2)",
                        params![old_id, new_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .unwrap();
                (ticks.0 == ticks.1).then_some((old_id, new_id, successor))
            })
            .expect("32 immediate capture pairs must include one production-clock tick pair");
        let digest_before = Store::record_digest_row_by_id_on(&store.conn, &new_id)
            .unwrap()
            .unwrap()
            .sealed_digest;

        store
            .capture_external(&successor, "global", &new_id, None, None, Some(&old_id))
            .unwrap();
        let first_relation: (Option<String>, Option<String>, Option<String>) = store
            .conn
            .query_row(
                "SELECT superseded_by,valid_from,valid_until FROM decisions WHERE id=?1",
                [&old_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        store
            .capture_external(&successor, "global", &new_id, None, None, Some(&old_id))
            .unwrap();
        let second_relation: (Option<String>, Option<String>, Option<String>) = store
            .conn
            .query_row(
                "SELECT superseded_by,valid_from,valid_until FROM decisions WHERE id=?1",
                [&old_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();

        assert_eq!(first_relation, second_relation);
        assert_eq!(first_relation.0.as_deref(), Some(new_id.as_str()));
        assert!(
            iso_to_epoch(first_relation.2.as_deref().unwrap()).unwrap()
                > iso_to_epoch(first_relation.1.as_deref().unwrap()).unwrap()
        );
        assert_eq!(
            Store::record_digest_row_by_id_on(&store.conn, &new_id)
                .unwrap()
                .unwrap()
                .sealed_digest,
            digest_before
        );
        assert!(matches!(
            store
                .get_current_evidence_in_scope(&old_id, "global")
                .unwrap(),
            ScopedCurrentRecordResolution::Ok {
                current_id,
                ref record,
                ..
            } if current_id == new_id && record.id == new_id
        ));
    }

    #[test]
    fn iso_to_epoch_accepts_only_canonical_utc_timestamps() {
        for valid in [
            "1970-01-01T00:00:00Z",
            "2000-02-29T23:59:59Z",
            "2026-09-03T12:34:56.1Z",
            "2026-09-03T12:34:56.123456789Z",
            "9999-12-31T23:59:59Z",
        ] {
            assert!(iso_to_epoch(valid).is_some(), "rejected canonical {valid}");
        }
        let boundary = format!("2026-09-03T12:34:56.{}Z", "1".repeat(107));
        let over_bound = format!("2026-09-03T12:34:56.{}Z", "1".repeat(108));
        assert_eq!(boundary.len(), MAX_TEMPORAL_VALUE_BYTES);
        assert!(iso_to_epoch(&boundary).is_some());
        assert_eq!(over_bound.len(), MAX_TEMPORAL_VALUE_BYTES + 1);
        assert!(iso_to_epoch(&over_bound).is_none());
        for invalid in [
            "2026X01Y01Q00R00S00Z",
            "2026-01-01 00:00:00Z",
            "2026-01-01T00:00:00z",
            "2026-01-01T00:00:00Ztrailing",
            "2026-01-01T00:00:00.Z",
            "2026-01-01T00:00:00.1Ztrailing",
            "2026-01-01T24:00:00Z",
            "2026-01-01T00:60:00Z",
            "2026-01-01T00:00:60Z",
            "2026-02-29T00:00:00Z",
            "2024-02-30T00:00:00Z",
            "2026-04-31T00:00:00Z",
            "1969-12-31T23:59:59Z",
            "10000-01-01T00:00:00Z",
        ] {
            assert!(iso_to_epoch(invalid).is_none(), "accepted {invalid}");
        }
    }

    #[test]
    fn completed_exact_relation_replay_skips_malformed_predecessor_time() {
        let store = temp_store();
        let successor = decision("completed successor", "successor body", 0.5, None);
        store
            .capture_external(
                &successor,
                "global",
                "completed-new",
                Some("2026-02-01T00:00:00Z"),
                None,
                None,
            )
            .unwrap();
        let mut predecessor = history_row(
            "completed-old",
            Some("completed-new"),
            "global",
            "predecessor body",
        );
        predecessor.title = "completed predecessor".to_owned();
        predecessor.valid_from = Some("legacy-not-a-time".to_owned());
        predecessor.git_refs.clear();
        store.import_external(&[predecessor]).unwrap();
        let snapshot = || {
            store
                .conn
                .query_row(
                    "SELECT superseded_by,valid_from,valid_until,record_digest_v1,
                            (SELECT record_digest_v1 FROM decisions WHERE id='completed-new'),
                            (SELECT count(*) FROM decisions),
                            (SELECT count(*) FROM decision_git_refs)
                     FROM decisions WHERE id='completed-old'",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, i64>(5)?,
                            row.get::<_, i64>(6)?,
                        ))
                    },
                )
                .unwrap()
        };
        let before = snapshot();

        store
            .capture_external(
                &successor,
                "global",
                "completed-new",
                Some("2026-02-01T00:00:00Z"),
                None,
                Some("completed-old"),
            )
            .unwrap();

        assert_eq!(snapshot(), before);
    }

    #[test]
    fn completed_different_relation_returns_typed_conflict_before_time_parsing() {
        let store = temp_store();
        let wanted = decision("wanted successor", "wanted body", 0.5, None);
        let other = decision("other successor", "other body", 0.5, None);
        store
            .capture_external(
                &other,
                "global",
                "other-new",
                Some("2026-02-01T00:00:00Z"),
                None,
                None,
            )
            .unwrap();
        let mut predecessor = history_row(
            "conflicting-old",
            Some("other-new"),
            "global",
            "predecessor body",
        );
        predecessor.title = "conflicting predecessor".to_owned();
        predecessor.valid_from = Some("legacy-not-a-time".to_owned());
        predecessor.git_refs.clear();
        store.import_external(&[predecessor]).unwrap();
        let snapshot = || {
            let decisions: Vec<(String, Option<String>, Option<String>, String)> = store
                .conn
                .prepare(
                    "SELECT id,superseded_by,valid_until,record_digest_v1
                     FROM decisions ORDER BY id",
                )
                .unwrap()
                .query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            let relation_count: i64 = store
                .conn
                .query_row("SELECT count(*) FROM decision_git_refs", [], |row| {
                    row.get(0)
                })
                .unwrap();
            (decisions, relation_count)
        };
        let before = snapshot();

        let error = store
            .capture_external(
                &wanted,
                "global",
                "wanted-new",
                Some("2026-03-01T00:00:00Z"),
                None,
                Some("conflicting-old"),
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
    }

    #[test]
    fn missing_supersession_target_fails_typed_before_either_capture_inserts() {
        let store = temp_store();
        let snapshot = || {
            let decision_count: i64 = store
                .conn
                .query_row("SELECT count(*) FROM decisions", [], |row| row.get(0))
                .unwrap();
            let relation_count: i64 = store
                .conn
                .query_row("SELECT count(*) FROM decision_git_refs", [], |row| {
                    row.get(0)
                })
                .unwrap();
            let fts_count: i64 = store
                .conn
                .query_row("SELECT count(*) FROM decisions_fts", [], |row| row.get(0))
                .unwrap();
            (decision_count, relation_count, fts_count)
        };
        let before = snapshot();
        let decision = decision("missing target", "must not persist", 0.5, None);

        for error in [
            store
                .capture_external(
                    &decision,
                    "global",
                    "missing-target-external",
                    Some("2026-03-01T00:00:00Z"),
                    None,
                    Some("absent-old"),
                )
                .unwrap_err(),
            store
                .capture(&decision, "global", Some("absent-old"))
                .unwrap_err(),
        ] {
            assert_eq!(
                error.downcast_ref::<SupersessionTargetNotFound>(),
                Some(&SupersessionTargetNotFound)
            );
            assert_eq!(
                error.to_string(),
                "supersession_target_not_found: predecessor was not found"
            );
            assert_eq!(snapshot(), before);
        }
    }

    #[test]
    fn changed_relation_during_apply_rolls_back_candidate_and_all_retirements() {
        let store = temp_store();
        let mut explicit = history_row("race-a-explicit", None, "global", "explicit body");
        explicit.title = "different title".to_owned();
        explicit.git_refs.clear();
        let mut automatic = history_row("race-z-automatic", None, "global", "automatic body");
        automatic.title = "race successor".to_owned();
        automatic.git_refs.clear();
        store.import_external(&[explicit, automatic]).unwrap();
        let snapshot = || {
            let decisions: Vec<(String, Option<String>, Option<String>, String)> = store
                .conn
                .prepare(
                    "SELECT id,superseded_by,valid_until,record_digest_v1
                     FROM decisions ORDER BY id",
                )
                .unwrap()
                .query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            let fts: Vec<String> = store
                .conn
                .prepare("SELECT title FROM decisions_fts ORDER BY title")
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            let relation_count: i64 = store
                .conn
                .query_row("SELECT count(*) FROM decision_git_refs", [], |row| {
                    row.get(0)
                })
                .unwrap();
            (decisions, fts, relation_count)
        };
        let before = snapshot();
        let successor = decision("race successor", "candidate body", 0.5, None);

        let error = store
            .capture_external_with_pre_retirement_hook(
                ExternalCaptureRequest {
                    decision: &successor,
                    scope: "global",
                    id: "race-candidate",
                    valid_from: Some("2026-03-01T00:00:00Z"),
                    fact_key: None,
                    supersedes: Some("race-a-explicit"),
                },
                |tx| {
                    tx.execute(
                        "UPDATE decisions
                         SET valid_until='2026-02-01T00:00:00Z'
                         WHERE id='race-z-automatic'",
                        [],
                    )?;
                    Ok(())
                },
            )
            .unwrap_err();

        assert_eq!(
            error.downcast_ref::<SupersessionConflict>(),
            Some(&SupersessionConflict)
        );
        assert_eq!(snapshot(), before);
    }

    #[test]
    fn automatic_retirement_preflights_all_predecessors_before_effects() {
        let store = temp_store();
        let successor = decision("automatic shared title", "successor body", 0.5, None);
        store
            .capture_external(
                &successor,
                "global",
                "automatic-new",
                Some("2026-02-01T00:00:00Z"),
                None,
                None,
            )
            .unwrap();
        let mut valid = history_row("automatic-a-valid", None, "global", "valid predecessor");
        valid.title = "automatic shared title".to_owned();
        valid.git_refs.clear();
        let mut invalid = history_row("automatic-z-invalid", None, "global", "invalid predecessor");
        invalid.title = "automatic shared title".to_owned();
        invalid.valid_from = Some("2026X01Y01Q00R00S00Z".to_owned());
        invalid.git_refs.clear();
        store.import_external(&[valid, invalid]).unwrap();
        let snapshot = || {
            let mut statement = store
                .conn
                .prepare(
                    "SELECT id,superseded_by,valid_from,valid_until,record_digest_v1
                     FROM decisions ORDER BY id",
                )
                .unwrap();
            let decisions = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            let relations: Vec<(String, String, String)> = store
                .conn
                .prepare(
                    "SELECT decision_id,commit_hash,commit_subject
                     FROM decision_git_refs ORDER BY decision_id,commit_hash",
                )
                .unwrap()
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            (decisions, relations)
        };
        let before = snapshot();

        let error = store
            .capture_external(
                &successor,
                "global",
                "automatic-new",
                Some("2026-02-01T00:00:00Z"),
                None,
                None,
            )
            .unwrap_err();

        assert_eq!(
            error.downcast_ref::<CurrentRecordErrorCode>(),
            Some(&CurrentRecordErrorCode::InvalidTemporalData)
        );
        assert_eq!(snapshot(), before);
    }

    #[test]
    fn retirement_rejects_out_of_domain_and_malformed_predecessor_time_before_effects() {
        let store = temp_store();
        let over_bound = format!("2026-01-01T00:00:00.{}Z", "1".repeat(108));
        for (label, valid_from, parses) in [
            ("maximum", "9999-12-31T23:59:59Z".to_owned(), true),
            ("malformed", "legacy-not-a-time".to_owned(), false),
            ("noncanonical", "2026X01Y01Q00R00S00Z".to_owned(), false),
            ("over-bound", over_bound, false),
        ] {
            assert_eq!(iso_to_epoch(&valid_from).is_some(), parses);
            let old_id = format!("domain-old-{label}");
            let new_id = format!("domain-new-{label}");
            let mut predecessor = history_row(&old_id, None, "global", "predecessor body");
            predecessor.title = format!("domain old {label}");
            predecessor.valid_from = Some(valid_from);
            predecessor.git_refs.clear();
            store.import_external(&[predecessor]).unwrap();
            let successor = decision(&format!("domain new {label}"), "successor body", 0.5, None);
            let snapshot = || {
                store
                    .conn
                    .query_row(
                        "SELECT
                           (SELECT count(*) FROM decisions),
                           superseded_by,valid_from,valid_until,record_digest_v1,
                           (SELECT record_digest_v1 FROM decisions WHERE id=?2),
                           (SELECT count(*) FROM decision_git_refs)
                         FROM decisions WHERE id=?1",
                        params![old_id, new_id],
                        |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                row.get::<_, Option<String>>(1)?,
                                row.get::<_, Option<String>>(2)?,
                                row.get::<_, Option<String>>(3)?,
                                row.get::<_, String>(4)?,
                                row.get::<_, Option<String>>(5)?,
                                row.get::<_, i64>(6)?,
                            ))
                        },
                    )
                    .unwrap()
            };
            let before = snapshot();
            let error = store
                .capture_external(
                    &successor,
                    "global",
                    &new_id,
                    Some("2026-01-01T00:00:00Z"),
                    None,
                    Some(&old_id),
                )
                .unwrap_err();
            assert_eq!(
                error.downcast_ref::<CurrentRecordErrorCode>(),
                Some(&CurrentRecordErrorCode::InvalidTemporalData)
            );
            assert_eq!(snapshot(), before, "{label} changed stored evidence");
        }
    }

    #[test]
    fn current_evidence_resolves_a_stale_link_and_returns_current_git_proof() {
        let store = temp_store();
        store
            .capture_external(
                &decision("database choice", "sqlite", 0.5, None),
                "global",
                "aaa",
                Some("2026-01-01T00:00:00Z"),
                None,
                None,
            )
            .unwrap();
        store
            .capture_external(
                &decision("database choice", "postgres now", 0.5, None),
                "global",
                "bbb",
                Some("2026-02-01T00:00:00Z"),
                None,
                Some("aaa"),
            )
            .unwrap();
        store.link_git("aaa", "old-commit", "Use SQLite").unwrap();
        store
            .link_git("bbb", "new-commit", "Move to Postgres")
            .unwrap();

        let as_of = iso_to_epoch("2026-03-01T00:00:00Z").unwrap();
        let evidence = store.get_current_evidence_at("aaa", as_of, 64).unwrap();
        let CurrentRecordResolution::Ok {
            requested_id,
            current_id,
            record,
            supersession_chain,
            git_refs,
            as_of: effective_as_of,
            ..
        } = evidence
        else {
            panic!("expected successful current resolution");
        };
        assert_eq!(requested_id, "aaa");
        assert_eq!(current_id, "bbb");
        assert_eq!(record.id, "bbb");
        assert_eq!(record.content, "postgres now");
        assert_eq!(supersession_chain, ["aaa", "bbb"]);
        assert_eq!(git_refs.len(), 1);
        assert_eq!(git_refs[0].commit_hash, "new-commit");
        assert_eq!(effective_as_of, "2026-03-01T00:00:00Z");
    }

    #[test]
    fn current_evidence_fails_closed_for_a_retired_record_without_a_successor() {
        let store = temp_store();
        store
            .capture_external(
                &decision("retired", "no longer current", 0.5, None),
                "global",
                "retired-id",
                Some("2026-01-01T00:00:00Z"),
                None,
                None,
            )
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE decisions SET valid_until='2026-02-01T00:00:00Z' WHERE id='retired-id'",
                [],
            )
            .unwrap();

        let as_of = iso_to_epoch("2026-03-01T00:00:00Z").unwrap();
        assert!(matches!(
            store
                .get_current_evidence_at("retired-id", as_of, 64)
                .unwrap(),
            CurrentRecordResolution::Error {
                code: CurrentRecordErrorCode::ExpiredWithoutSuccessor,
                ..
            }
        ));
        assert!(matches!(
            store.get_current_evidence_at("missing", as_of, 64).unwrap(),
            CurrentRecordResolution::Error {
                code: CurrentRecordErrorCode::NotFound,
                ..
            }
        ));
    }

    #[test]
    fn current_evidence_distinguishes_broken_cycle_and_traversal_limit() {
        let store = temp_store();
        let row = |id: &str, successor: Option<&str>| ExternalDecision {
            id: id.to_owned(),
            kind: "decision".to_owned(),
            title: format!("record {id}"),
            content: format!("complete body for {id}"),
            importance: 0.5,
            source: "synthetic".to_owned(),
            author: "tester".to_owned(),
            date: "2026-01-01".to_owned(),
            updated_at: None,
            accessed_count: None,
            times_injected: None,
            effectiveness: None,
            tags: None,
            scope: "scope-a".to_owned(),
            valid_from: Some("2026-01-01T00:00:00Z".to_owned()),
            valid_until: successor.map(|_| "2026-02-01T00:00:00Z".to_owned()),
            superseded_by: successor.map(str::to_owned),
            fact_key: None,
            git_refs: Vec::new(),
        };
        store
            .import_external(&[row("broken", Some("missing"))])
            .unwrap();
        store
            .import_external(&[
                row("cycle-a", Some("cycle-b")),
                row("cycle-b", Some("cycle-a")),
            ])
            .unwrap();
        store
            .import_external(&[row("long-a", Some("long-b")), row("long-b", None)])
            .unwrap();
        let as_of = iso_to_epoch("2026-03-01T00:00:00Z").unwrap();

        assert!(matches!(
            store.get_current_evidence_at("broken", as_of, 64).unwrap(),
            CurrentRecordResolution::Error {
                code: CurrentRecordErrorCode::BrokenChain,
                ..
            }
        ));
        assert!(matches!(
            store.get_current_evidence_at("cycle-a", as_of, 64).unwrap(),
            CurrentRecordResolution::Error {
                code: CurrentRecordErrorCode::Cycle,
                ..
            }
        ));
        assert!(matches!(
            store.get_current_evidence_at("long-a", as_of, 1).unwrap(),
            CurrentRecordResolution::Error {
                code: CurrentRecordErrorCode::TraversalLimit,
                ..
            }
        ));
    }

    #[test]
    fn current_evidence_obeys_validity_instants_and_rejects_bad_stored_time() {
        let store = temp_store();
        let insert = |id: &str, from: &str, until: Option<&str>| {
            let mut row = history_row(id, None, "scope-a", "full body");
            row.valid_from = Some(from.to_owned());
            row.valid_until = until.map(str::to_owned);
            store.import_external(&[row]).unwrap();
        };
        insert("future", "2026-04-01T00:00:00Z", None);
        insert(
            "bounded",
            "2026-01-01T00:00:00Z",
            Some("2026-04-01T00:00:00Z"),
        );
        insert("invalid", "not-a-time", None);
        insert(
            "inverted",
            "2026-05-01T00:00:00Z",
            Some("2026-04-01T00:00:00Z"),
        );
        let before = iso_to_epoch("2026-03-01T00:00:00Z").unwrap();
        let boundary = iso_to_epoch("2026-04-01T00:00:00Z").unwrap();

        assert!(matches!(
            store.get_current_evidence_at("future", before, 64).unwrap(),
            CurrentRecordResolution::Error {
                code: CurrentRecordErrorCode::NotYetValid,
                ..
            }
        ));
        assert!(matches!(
            store
                .get_current_evidence_at("bounded", before, 64)
                .unwrap(),
            CurrentRecordResolution::Ok { .. }
        ));
        assert!(matches!(
            store
                .get_current_evidence_at("bounded", boundary, 64)
                .unwrap(),
            CurrentRecordResolution::Error {
                code: CurrentRecordErrorCode::ExpiredWithoutSuccessor,
                ..
            }
        ));
        assert!(matches!(
            store
                .get_current_evidence_at("invalid", before, 64)
                .unwrap(),
            CurrentRecordResolution::Error {
                code: CurrentRecordErrorCode::InvalidTemporalData,
                ..
            }
        ));
        assert!(matches!(
            store
                .get_current_evidence_at("inverted", before, 64)
                .unwrap(),
            CurrentRecordResolution::Error {
                code: CurrentRecordErrorCode::InvalidTemporalData,
                ..
            }
        ));
    }

    #[test]
    fn scoped_current_hides_foreign_and_absent_nodes_without_mutating_state() {
        let store = temp_store();
        store
            .import_external(&[
                history_row("root", Some("middle"), "scope-a", "root body"),
                history_row("middle", Some("foreign"), "scope-a", "middle body"),
                history_row(
                    "foreign",
                    None,
                    "scope-b",
                    "foreign body sentinel 2099-01-01T00:00:00Z",
                ),
            ])
            .unwrap();
        store
            .link_git("foreign", "foreign-commit", "Foreign evidence sentinel")
            .unwrap();
        store
            .conn
            .execute_batch("DROP TRIGGER decisions_identity_update_guard;")
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE decisions
                 SET content=CAST(zeroblob(8388608) AS TEXT), importance=X'FF',
                     superseded_by=X'FF', valid_from=X'FF', valid_until=42
                 WHERE id='foreign'",
                [],
            )
            .unwrap();
        let as_of = iso_to_epoch("2026-03-01T00:00:00Z").unwrap();
        let changes_before_foreign = store.conn.total_changes();
        let foreign = store
            .get_current_evidence_in_scope_at("root", "scope-a", as_of, 64)
            .unwrap();
        assert_eq!(store.conn.total_changes(), changes_before_foreign);

        store
            .conn
            .execute(
                "UPDATE decisions SET superseded_by='absent' WHERE id='middle'",
                [],
            )
            .unwrap();
        let changes_before_absent = store.conn.total_changes();
        let absent = store
            .get_current_evidence_in_scope_at("root", "scope-a", as_of + 60, 64)
            .unwrap();
        assert_eq!(store.conn.total_changes(), changes_before_absent);

        let normalize = |resolution: CurrentRecordResolution| {
            let mut value = serde_json::to_value(resolution).unwrap();
            value["as_of"] = serde_json::Value::String("normalized".to_owned());
            value
        };
        let foreign = normalize(foreign);
        let absent = normalize(absent);
        assert_eq!(foreign, absent);
        assert_eq!(foreign["contract"], CURRENT_RATIONALE_CONTRACT);
        assert_eq!(foreign["code"], "broken_chain");
        assert_eq!(
            foreign["message"],
            "supersession chain is unavailable in the requested scope"
        );
        let wire = serde_json::to_string(&foreign).unwrap();
        for secret in [
            "foreign",
            "2099-01-01T00:00:00Z",
            "foreign body sentinel",
            "foreign-commit",
            "Foreign evidence sentinel",
        ] {
            assert!(!wire.contains(secret), "scoped error leaked {secret}");
        }
    }

    #[test]
    fn scoped_current_makes_wrong_scope_root_indistinguishable_from_absence() {
        let wrong_scope_store = temp_store();
        wrong_scope_store
            .import_external(&[history_row("same-id", None, "scope-b", "foreign root body")])
            .unwrap();
        wrong_scope_store
            .conn
            .execute_batch("DROP TRIGGER decisions_identity_update_guard;")
            .unwrap();
        wrong_scope_store
            .conn
            .execute(
                "UPDATE decisions
                 SET content=CAST(zeroblob(8388608) AS TEXT), importance=X'FF',
                     superseded_by=X'FF', valid_from=X'FF', valid_until=42
                 WHERE id='same-id'",
                [],
            )
            .unwrap();
        let absent_store = temp_store();
        let as_of = iso_to_epoch("2026-03-01T00:00:00Z").unwrap();

        let changes_before_wrong_scope = wrong_scope_store.conn.total_changes();
        let wrong_scope = wrong_scope_store
            .get_current_evidence_in_scope_at("same-id", "scope-a", as_of, 64)
            .unwrap();
        assert_eq!(
            wrong_scope_store.conn.total_changes(),
            changes_before_wrong_scope
        );
        let changes_before_absent = absent_store.conn.total_changes();
        let absent = absent_store
            .get_current_evidence_in_scope_at("same-id", "scope-a", as_of, 64)
            .unwrap();
        assert_eq!(absent_store.conn.total_changes(), changes_before_absent);
        let wrong_scope = serde_json::to_value(wrong_scope).unwrap();
        assert_eq!(wrong_scope["contract"], CURRENT_RATIONALE_CONTRACT);
        assert_eq!(wrong_scope, serde_json::to_value(absent).unwrap());
    }

    #[test]
    fn current_evidence_uses_one_snapshot_then_observes_the_next_complete_snapshot() {
        let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "open-why-current-snapshot-{}-{n}",
                std::process::id()
            ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("current.db");
        let store = Store::open_with_embedder_and_store_instance_id(
            &path,
            None,
            &format!("provider:current:{n}"),
        )
        .unwrap();
        store
            .conn
            .execute_batch("PRAGMA journal_mode=WAL;")
            .unwrap();
        store
            .import_external(&[
                history_row("snapshot-root", Some("snapshot-old"), "scope-a", "root"),
                history_row("snapshot-old", None, "scope-a", "old current body"),
                history_row("snapshot-new", None, "scope-a", "pending new body"),
            ])
            .unwrap();
        store
            .link_git("snapshot-old", "old-commit", "Old evidence")
            .unwrap();
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        let as_of = iso_to_epoch("2026-03-01T00:00:00Z").unwrap();

        let first = store
            .get_current_evidence_at_with_scope_and_hook(
                "snapshot-root",
                Some("scope-a"),
                as_of,
                64,
                true,
                || {
                    writer.execute_batch(
                        "BEGIN IMMEDIATE;
                         UPDATE decisions SET superseded_by='snapshot-new'
                         WHERE id='snapshot-root';
                         INSERT INTO decision_git_refs
                         (decision_id,commit_hash,commit_subject)
                         VALUES ('snapshot-new','new-commit','New evidence');
                         COMMIT;",
                    )?;
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(
            first
                .identity
                .as_ref()
                .map(|identity| identity.record_id.as_str()),
            Some("snapshot-old")
        );
        let CurrentRecordResolution::Ok {
            current_id,
            record,
            git_refs,
            supersession_chain,
            ..
        } = first.resolution
        else {
            panic!("expected old coherent snapshot");
        };
        assert_eq!(current_id, "snapshot-old");
        assert_eq!(record.content, "old current body");
        assert_eq!(supersession_chain, ["snapshot-root", "snapshot-old"]);
        assert!(git_refs.iter().any(|item| item.commit_hash == "old-commit"));
        assert!(!git_refs.iter().any(|item| item.commit_hash == "new-commit"));

        let second = store
            .get_current_evidence_at_with_scope_and_hook(
                "snapshot-root",
                Some("scope-a"),
                as_of,
                64,
                true,
                || Ok(()),
            )
            .unwrap();
        assert_eq!(
            second
                .identity
                .as_ref()
                .map(|identity| identity.record_id.as_str()),
            Some("snapshot-new")
        );
        let CurrentRecordResolution::Ok {
            current_id,
            record,
            git_refs,
            supersession_chain,
            ..
        } = second.resolution
        else {
            panic!("expected new coherent snapshot");
        };
        assert_eq!(current_id, "snapshot-new");
        assert_eq!(record.content, "pending new body");
        assert_eq!(supersession_chain, ["snapshot-root", "snapshot-new"]);
        assert!(git_refs.iter().any(|item| item.commit_hash == "new-commit"));
        assert!(!git_refs.iter().any(|item| item.commit_hash == "old-commit"));

        drop(writer);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rationale_history_pages_one_two_and_long_chains_without_gaps() {
        let store = temp_store();
        store
            .import_external(&[history_row("one", None, "scope-a", "one body")])
            .unwrap();
        store
            .import_external(&[
                history_row("two-a", Some("two-b"), "scope-a", "old body"),
                history_row("two-b", None, "scope-a", "new body"),
            ])
            .unwrap();
        store
            .import_external(&[
                history_row("long-a", Some("long-b"), "scope-a", "body α"),
                history_row("long-b", Some("long-c"), "scope-a", "body β"),
                history_row("long-c", Some("long-d"), "scope-a", "body γ"),
                history_row("long-d", Some("long-e"), "scope-a", "body δ"),
                history_row("long-e", None, "scope-a", "body 🚀"),
            ])
            .unwrap();
        let as_of = iso_to_epoch("2026-03-01T00:00:00Z").unwrap();

        let RationaleHistoryResolution::Ok {
            records,
            next_cursor,
            complete,
            page_start_id,
            ..
        } = store
            .get_rationale_history_at("one", "scope-a", None, 3, as_of, 64)
            .unwrap()
        else {
            panic!("expected one-record history");
        };
        assert_eq!(page_start_id, "one");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.id, "one");
        assert_eq!(records[0].git_refs[0].commit_hash, "commit-one");
        assert!(complete);
        assert_eq!(next_cursor, None);

        let RationaleHistoryResolution::Ok { records, .. } = store
            .get_rationale_history_at("two-a", "scope-a", None, 3, as_of, 64)
            .unwrap()
        else {
            panic!("expected two-record history");
        };
        assert_eq!(
            records
                .iter()
                .map(|item| item.record.id.as_str())
                .collect::<Vec<_>>(),
            ["two-a", "two-b"]
        );

        let first = store
            .get_rationale_history_at("long-a", "scope-a", None, 3, as_of, 64)
            .unwrap();
        let RationaleHistoryResolution::Ok {
            records,
            next_cursor,
            complete,
            ..
        } = &first
        else {
            panic!("expected first history page");
        };
        assert_eq!(
            records
                .iter()
                .map(|item| item.record.id.as_str())
                .collect::<Vec<_>>(),
            ["long-a", "long-b", "long-c"]
        );
        assert_eq!(records[2].record.content, "body γ");
        assert_eq!(next_cursor.as_deref(), Some("long-d"));
        assert!(!complete);

        let second = store
            .get_rationale_history_at("long-a", "scope-a", next_cursor.as_deref(), 3, as_of, 64)
            .unwrap();
        let RationaleHistoryResolution::Ok {
            records,
            next_cursor,
            complete,
            page_start_id,
            ..
        } = &second
        else {
            panic!("expected final history page");
        };
        assert_eq!(page_start_id, "long-d");
        assert_eq!(
            records
                .iter()
                .map(|item| item.record.id.as_str())
                .collect::<Vec<_>>(),
            ["long-d", "long-e"]
        );
        assert_eq!(records[1].record.content, "body 🚀");
        assert_eq!(next_cursor, &None);
        assert!(*complete);

        let repeat = store
            .get_rationale_history_at("long-a", "scope-a", None, 3, as_of, 64)
            .unwrap();
        assert_eq!(
            serde_json::to_value(first).unwrap(),
            serde_json::to_value(repeat).unwrap()
        );
    }

    #[test]
    fn rationale_history_rejects_off_chain_scope_and_structural_failures() {
        let store = temp_store();
        store
            .import_external(&[
                history_row("root", Some("next"), "scope-a", "root"),
                history_row("next", None, "scope-a", "next"),
                history_row("unrelated", None, "scope-a", "unrelated"),
                history_row("foreign", None, "scope-b", "foreign"),
                history_row("broken", Some("missing-successor"), "scope-a", "broken"),
                history_row("cycle-a", Some("cycle-b"), "scope-a", "cycle a"),
                history_row("cycle-b", Some("cycle-a"), "scope-a", "cycle b"),
                history_row("cap-a", Some("cap-b"), "scope-a", "cap a"),
                history_row("cap-b", None, "scope-a", "cap b"),
                history_row(
                    "cross-root",
                    Some("cross-successor"),
                    "scope-a",
                    "cross root",
                ),
                history_row("cross-successor", None, "scope-b", "foreign body"),
            ])
            .unwrap();
        let mut bad_time = history_row("bad-time", None, "scope-a", "body");
        bad_time.valid_from = Some("not-a-time".to_owned());
        store.import_external(&[bad_time]).unwrap();
        let as_of = iso_to_epoch("2026-03-01T00:00:00Z").unwrap();
        let code = |resolution| match resolution {
            RationaleHistoryResolution::Error { code, .. } => code,
            RationaleHistoryResolution::Ok { .. } => panic!("expected typed history error"),
        };

        let RationaleHistoryResolution::Ok { records, .. } = store
            .get_rationale_history_at("root", "scope-a", Some("next"), 3, as_of, 64)
            .unwrap()
        else {
            unreachable!();
        };
        assert_eq!(records[0].record.id, "next");
        for cursor in ["unrelated", "foreign", "missing-cursor"] {
            assert_eq!(
                code(
                    store
                        .get_rationale_history_at("root", "scope-a", Some(cursor), 3, as_of, 64,)
                        .unwrap()
                ),
                RationaleHistoryErrorCode::InvalidCursor
            );
        }
        let wrong_scope = store
            .get_rationale_history_at("foreign", "scope-a", None, 3, as_of, 64)
            .unwrap();
        let empty_store = temp_store();
        let same_id_missing = empty_store
            .get_rationale_history_at("foreign", "scope-a", None, 3, as_of, 64)
            .unwrap();
        assert_eq!(
            serde_json::to_value(wrong_scope).unwrap(),
            serde_json::to_value(same_id_missing).unwrap(),
            "wrong-scope and missing records must be indistinguishable"
        );
        assert_eq!(
            code(
                store
                    .get_rationale_history_at("absent", "scope-a", None, 3, as_of, 64)
                    .unwrap()
            ),
            RationaleHistoryErrorCode::NotFound
        );
        assert_eq!(
            code(
                store
                    .get_rationale_history_at("broken", "scope-a", None, 3, as_of, 64)
                    .unwrap()
            ),
            RationaleHistoryErrorCode::BrokenChain
        );
        let foreign_successor = store
            .get_rationale_history_at("cross-root", "scope-a", None, 3, as_of, 64)
            .unwrap();
        let unavailable_successor = store
            .get_rationale_history_at("broken", "scope-a", None, 3, as_of, 64)
            .unwrap();
        let error_shape = |resolution| match resolution {
            RationaleHistoryResolution::Error { code, message, .. } => (code, message),
            RationaleHistoryResolution::Ok { .. } => panic!("expected broken chain"),
        };
        let (foreign_code, foreign_message) = error_shape(foreign_successor);
        let (missing_code, missing_message) = error_shape(unavailable_successor);
        assert_eq!(foreign_code, RationaleHistoryErrorCode::BrokenChain);
        assert_eq!(foreign_code, missing_code);
        assert_eq!(foreign_message, missing_message);
        assert!(!foreign_message.contains("cross-successor"));
        assert_eq!(
            code(
                store
                    .get_rationale_history_at("cycle-a", "scope-a", None, 3, as_of, 64)
                    .unwrap()
            ),
            RationaleHistoryErrorCode::Cycle
        );
        assert_eq!(
            code(
                store
                    .get_rationale_history_at("cap-a", "scope-a", None, 3, as_of, 1)
                    .unwrap()
            ),
            RationaleHistoryErrorCode::TraversalLimit
        );
        assert_eq!(
            code(
                store
                    .get_rationale_history_at("bad-time", "scope-a", None, 3, as_of, 64)
                    .unwrap()
            ),
            RationaleHistoryErrorCode::InvalidTemporalData
        );
    }

    #[test]
    fn rationale_history_uses_one_snapshot_and_bounds_selected_hydration() {
        let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "open-why-history-snapshot-{}-{n}",
                std::process::id()
            ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.db");
        let store = Store::open_with_embedder_and_store_instance_id(
            &path,
            None,
            &format!("provider:history:{n}"),
        )
        .unwrap();
        store
            .conn
            .execute_batch("PRAGMA journal_mode=WAL;")
            .unwrap();
        store
            .import_external(&[
                history_row("snapshot-a", Some("snapshot-b"), "scope-a", "old root"),
                history_row("snapshot-b", None, "scope-a", "old successor"),
                history_row("snapshot-alt", None, "scope-a", "alternate successor"),
            ])
            .unwrap();
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        let as_of = iso_to_epoch("2026-03-01T00:00:00Z").unwrap();
        let resolution = store
            .get_rationale_history_at_with_hook(
                HistoryPageRequest {
                    id: "snapshot-a",
                    scope: "scope-a",
                    page_cursor: None,
                    limit: 3,
                    as_of,
                    chain_cap: 64,
                },
                || {
                    writer.execute(
                        "UPDATE decisions SET superseded_by='snapshot-alt'
                         WHERE id='snapshot-a'",
                        [],
                    )?;
                    writer.execute(
                        "INSERT INTO decision_git_refs
                         (decision_id,commit_hash,commit_subject)
                         VALUES ('snapshot-b','concurrent-commit','Concurrent evidence')",
                        [],
                    )?;
                    Ok(())
                },
            )
            .unwrap();
        let RationaleHistoryResolution::Ok { records, .. } = resolution else {
            panic!("expected snapshot history");
        };
        assert_eq!(records.len(), 2);
        assert_eq!(
            records[0].record.superseded_by.as_deref(),
            Some("snapshot-b")
        );
        assert_eq!(records[1].record.content, "old successor");
        assert_eq!(records[1].git_refs.len(), 1);
        assert!(!records[1]
            .git_refs
            .iter()
            .any(|git_ref| git_ref.commit_hash == "concurrent-commit"));
        assert_eq!(
            store
                .get_record_any("snapshot-a", true)
                .unwrap()
                .unwrap()
                .superseded_by
                .as_deref(),
            Some("snapshot-alt")
        );
        assert_eq!(
            store
                .get_record_any("snapshot-b", true)
                .unwrap()
                .unwrap()
                .content,
            "old successor"
        );
        assert_eq!(store.linked_commits("snapshot-b").unwrap().len(), 2);

        let huge_tail = "\0".repeat(MAX_HISTORY_PAGE_SOURCE_BYTES + 1);
        store
            .import_external(&[
                history_row("bounded-a", Some("bounded-b"), "scope-a", "a"),
                history_row("bounded-b", Some("bounded-c"), "scope-a", "b"),
                history_row("bounded-c", Some("bounded-d"), "scope-a", "c"),
                history_row("bounded-d", None, "scope-a", &huge_tail),
            ])
            .unwrap();
        assert!(matches!(
            store
                .get_rationale_history_at("bounded-a", "scope-a", None, 3, as_of, 64)
                .unwrap(),
            RationaleHistoryResolution::Ok {
                complete: false,
                ..
            }
        ));
        assert!(matches!(
            store
                .get_rationale_history_at("bounded-a", "scope-a", Some("bounded-d"), 3, as_of, 64,)
                .unwrap(),
            RationaleHistoryResolution::Error {
                code: RationaleHistoryErrorCode::ResponseTooLarge,
                ..
            }
        ));

        store
            .import_external(&[history_row(
                "reference-budget",
                None,
                "scope-a",
                "bounded body",
            )])
            .unwrap();
        store
            .conn
            .execute(
                "WITH RECURSIVE numbers(n) AS (
                     SELECT 1
                     UNION ALL
                     SELECT n + 1 FROM numbers WHERE n < ?2
                 )
                 INSERT INTO decision_git_refs
                     (decision_id,commit_hash,commit_subject)
                 SELECT ?1, printf('budget-commit-%03d',n), 'bounded evidence'
                 FROM numbers",
                params!["reference-budget", MAX_HISTORY_PAGE_GIT_REFS + 1],
            )
            .unwrap();
        assert!(matches!(
            store
                .get_rationale_history_at("reference-budget", "scope-a", None, 3, as_of, 64,)
                .unwrap(),
            RationaleHistoryResolution::Error {
                code: RationaleHistoryErrorCode::ResponseTooLarge,
                ..
            }
        ));
        drop(writer);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rationale_history_v1_validates_records_but_not_cross_record_continuity() {
        let store = temp_store();
        let mut overlap_a = history_row("overlap-a", Some("overlap-b"), "scope-a", "a");
        overlap_a.valid_until = Some("2026-03-01T00:00:00Z".to_owned());
        let mut overlap_b = history_row("overlap-b", None, "scope-a", "b");
        overlap_b.valid_from = Some("2026-02-01T00:00:00Z".to_owned());
        let mut gap_a = history_row("gap-a", Some("gap-b"), "scope-a", "a");
        gap_a.valid_until = Some("2026-02-01T00:00:00Z".to_owned());
        let mut gap_b = history_row("gap-b", None, "scope-a", "b");
        gap_b.valid_from = Some("2026-03-01T00:00:00Z".to_owned());
        store
            .import_external(&[overlap_a, overlap_b, gap_a, gap_b])
            .unwrap();
        let as_of = iso_to_epoch("2026-04-01T00:00:00Z").unwrap();

        for root in ["overlap-a", "gap-a"] {
            assert!(matches!(
                store
                    .get_rationale_history_at(root, "scope-a", None, 3, as_of, 64)
                    .unwrap(),
                RationaleHistoryResolution::Ok { .. }
            ));
        }
    }

    #[test]
    fn scoped_commit_link_is_authoritative_idempotent_and_conflict_safe() {
        let store = temp_store();
        store
            .import_external(&[history_row("sealed-link", None, "scope-a", "body")])
            .unwrap();
        let identity = evidence_identity(&store, "sealed-link", "scope-a");
        let before_identity = identity.clone();

        let created = store.link_git_in_scope(&identity, "abc123", "Create link");
        assert_eq!(
            created,
            ScopedCommitLinkResolution::Ok {
                contract: SCOPED_COMMIT_LINK_WRITE_CONTRACT,
                outcome: ScopedCommitLinkOutcome::Created,
                evidence_identity: identity.clone(),
                git_ref: GitRef {
                    commit_hash: "abc123".to_owned(),
                    commit_subject: "Create link".to_owned(),
                },
            }
        );
        assert_eq!(
            evidence_identity(&store, "sealed-link", "scope-a"),
            before_identity
        );
        let CommitLinksResolution::Ok { items, .. } = store
            .get_commit_links("scope-a", "abc123", None, 20)
            .unwrap()
        else {
            panic!("created link must be readable through commit-links v1");
        };
        assert_eq!(items[0].record_id, "sealed-link");
        assert_eq!(items[0].commit_subject, "Create link");

        let observer = Connection::open(store.conn.path().unwrap()).unwrap();
        let data_version = || {
            observer
                .pragma_query_value(None, "data_version", |row| row.get::<_, i64>(0))
                .unwrap()
        };
        let version_before = data_version();
        let changes_before = store.conn.total_changes();
        let replay = store.link_git_in_scope(&identity, "abc123", "Create link");
        assert!(matches!(
            replay,
            ScopedCommitLinkResolution::Ok {
                outcome: ScopedCommitLinkOutcome::ExactReplay,
                ..
            }
        ));
        assert_eq!(store.conn.total_changes(), changes_before);
        assert_eq!(data_version(), version_before);

        let conflict = store.link_git_in_scope(&identity, "abc123", "Changed subject");
        assert!(matches!(
            conflict,
            ScopedCommitLinkResolution::Error {
                code: ScopedCommitLinkErrorCode::LinkConflict,
                retryable: false,
                ..
            }
        ));
        assert_eq!(store.conn.total_changes(), changes_before);
        assert_eq!(data_version(), version_before);
        assert_eq!(
            store.linked_commits("sealed-link").unwrap()[0].1,
            "Create link"
        );
    }

    #[test]
    fn scoped_commit_link_collapses_identity_failures_before_effects() {
        let store = temp_store();
        store
            .import_external(&[
                history_row("sealed-link", None, "scope-a", "body"),
                history_row("foreign-link", None, "scope-b", "foreign secret"),
            ])
            .unwrap();
        let valid = evidence_identity(&store, "sealed-link", "scope-a");
        let foreign = evidence_identity(&store, "foreign-link", "scope-b");
        let mut cases = Vec::new();

        let mut missing = valid.clone();
        missing.record_id = "missing-link".to_owned();
        cases.push(missing);
        let mut wrong_scope = foreign;
        wrong_scope.scope = "scope-a".to_owned();
        cases.push(wrong_scope);
        let mut wrong_store = valid.clone();
        wrong_store.store_instance_id = "provider:another-store".to_owned();
        cases.push(wrong_store);
        let mut wrong_digest = valid.clone();
        wrong_digest.record_digest = "0".repeat(64);
        cases.push(wrong_digest);
        let mut wrong_contract = valid.clone();
        wrong_contract.contract = "open-why.evidence-identity/v0";
        cases.push(wrong_contract);
        let mut wrong_digest_contract = valid.clone();
        wrong_digest_contract.record_digest_contract = "open-why.record-digest/v0";
        cases.push(wrong_digest_contract);
        let mut malformed_digest = valid.clone();
        malformed_digest.record_digest = "A".repeat(64);
        cases.push(malformed_digest);
        let mut oversized_store = valid.clone();
        oversized_store.store_instance_id = "a".repeat(MAX_STORE_INSTANCE_ID_BYTES + 1);
        cases.push(oversized_store);
        let mut empty_scope = valid.clone();
        empty_scope.scope.clear();
        cases.push(empty_scope);
        let mut oversized_scope = valid.clone();
        oversized_scope.scope = "s".repeat(MAX_COMMIT_LINK_SCOPE_BYTES + 1);
        cases.push(oversized_scope);
        let mut empty_record = valid.clone();
        empty_record.record_id.clear();
        cases.push(empty_record);
        let mut oversized_record = valid.clone();
        oversized_record.record_id = "r".repeat(MAX_COMMIT_LINK_RECORD_ID_BYTES + 1);
        cases.push(oversized_record);

        let observer = Connection::open(store.conn.path().unwrap()).unwrap();
        let version_before: i64 = observer
            .pragma_query_value(None, "data_version", |row| row.get(0))
            .unwrap();
        let changes_before = store.conn.total_changes();
        let expected = serde_json::to_vec(&scoped_commit_link_error(
            ScopedCommitLinkErrorCode::EvidenceUnavailable,
            false,
        ))
        .unwrap();
        for identity in cases {
            let resolution = store.link_git_in_scope(&identity, "abc123", "No link");
            assert_eq!(serde_json::to_vec(&resolution).unwrap(), expected);
            assert_eq!(store.conn.total_changes(), changes_before);
            assert_eq!(
                observer
                    .pragma_query_value(None, "data_version", |row| row.get::<_, i64>(0))
                    .unwrap(),
                version_before
            );
        }
        assert!(!store
            .linked_commits("sealed-link")
            .unwrap()
            .iter()
            .any(|git_ref| git_ref.0 == "abc123"));
        assert!(!store
            .linked_commits("foreign-link")
            .unwrap()
            .iter()
            .any(|git_ref| git_ref.0 == "abc123"));

        let invalid = store.link_git_in_scope(&valid, "", "No link");
        assert!(matches!(
            invalid,
            ScopedCommitLinkResolution::Error {
                code: ScopedCommitLinkErrorCode::InvalidRequest,
                ..
            }
        ));
        let oversized = "é".repeat(MAX_COMMIT_LINK_HASH_BYTES / 2 + 1);
        assert!(matches!(
            store.link_git_in_scope(&valid, &oversized, "No link"),
            ScopedCommitLinkResolution::Error {
                code: ScopedCommitLinkErrorCode::InvalidRequest,
                ..
            }
        ));
        assert!(matches!(
            store.link_git_in_scope(
                &valid,
                "subject-too-long",
                &"s".repeat(MAX_COMMIT_LINK_SUBJECT_BYTES + 1)
            ),
            ScopedCommitLinkResolution::Error {
                code: ScopedCommitLinkErrorCode::InvalidRequest,
                ..
            }
        ));

        let exact_commit = format!(" {} ", "c".repeat(MAX_COMMIT_LINK_HASH_BYTES - 2));
        let exact_subject = "s".repeat(MAX_COMMIT_LINK_SUBJECT_BYTES);
        let exact = store.link_git_in_scope(&valid, &exact_commit, &exact_subject);
        let ScopedCommitLinkResolution::Ok { git_ref, .. } = exact else {
            panic!("exact input bounds must be accepted");
        };
        assert_eq!(git_ref.commit_hash, exact_commit);
        assert_eq!(git_ref.commit_subject, exact_subject);
    }

    #[test]
    fn scoped_commit_link_recomputes_the_sealed_envelope_before_effects() {
        let store = temp_store();
        store
            .import_external(&[history_row("sealed-link", None, "scope-a", "body")])
            .unwrap();
        let identity = evidence_identity(&store, "sealed-link", "scope-a");
        let trigger: String = store
            .conn
            .query_row(
                "SELECT sql FROM sqlite_schema
                 WHERE type='trigger' AND name='decisions_identity_update_guard'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        store
            .conn
            .execute_batch("DROP TRIGGER decisions_identity_update_guard")
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE decisions SET content='tampered secret' WHERE id='sealed-link'",
                [],
            )
            .unwrap();
        store.conn.execute_batch(&trigger).unwrap();
        let observer = Connection::open(store.conn.path().unwrap()).unwrap();
        let version_before: i64 = observer
            .pragma_query_value(None, "data_version", |row| row.get(0))
            .unwrap();
        let changes_before = store.conn.total_changes();

        assert_eq!(
            store.link_git_in_scope(&identity, "abc123", "No link"),
            scoped_commit_link_error(ScopedCommitLinkErrorCode::EvidenceUnavailable, false)
        );
        assert_eq!(store.conn.total_changes(), changes_before);
        assert_eq!(
            observer
                .pragma_query_value(None, "data_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            version_before
        );
        assert!(!store
            .linked_commits("sealed-link")
            .unwrap()
            .iter()
            .any(|git_ref| git_ref.0 == "abc123"));
    }

    #[test]
    fn scoped_commit_link_reports_held_writer_without_leaking_details() {
        let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "open-why-scoped-link-lock-{}-{n}",
                std::process::id()
            ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("link.db");
        let store =
            Store::open_with_store_instance_id(&path, &format!("provider:link:{n}")).unwrap();
        store
            .import_external(&[history_row("sealed-link", None, "scope-a", "body")])
            .unwrap();
        let identity = evidence_identity(&store, "sealed-link", "scope-a");
        store
            .conn
            .busy_timeout(std::time::Duration::from_millis(0))
            .unwrap();
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        let changes_before = store.conn.total_changes();

        assert_eq!(
            store.link_git_in_scope(&identity, "abc123", "No link"),
            scoped_commit_link_error(ScopedCommitLinkErrorCode::StoreUnavailable, true)
        );
        assert_eq!(store.conn.total_changes(), changes_before);
        writer.execute_batch("ROLLBACK").unwrap();
        assert!(!store
            .linked_commits("sealed-link")
            .unwrap()
            .iter()
            .any(|git_ref| git_ref.0 == "abc123"));

        store
            .conn
            .execute_batch(
                "CREATE TRIGGER reject_scoped_link
                 BEFORE INSERT ON decision_git_refs
                 WHEN NEW.commit_hash='reject-insert'
                 BEGIN SELECT RAISE(ABORT, 'private sqlite detail'); END",
            )
            .unwrap();
        let changes_before = store.conn.total_changes();
        let rejected = store.link_git_in_scope(&identity, "reject-insert", "No link");
        assert_eq!(
            rejected,
            scoped_commit_link_error(ScopedCommitLinkErrorCode::StoreUnavailable, false)
        );
        assert!(!serde_json::to_string(&rejected)
            .unwrap()
            .contains("private sqlite detail"));
        assert_eq!(store.conn.total_changes(), changes_before);
        assert!(!store
            .linked_commits("sealed-link")
            .unwrap()
            .iter()
            .any(|git_ref| git_ref.0 == "reject-insert"));
        drop(writer);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_scoped_commit_links_never_overwrite() {
        let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "open-why-scoped-link-race-{}-{n}",
                std::process::id()
            ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("link.db");
        let provider = format!("provider:link-race:{n}");
        let store = Store::open_with_store_instance_id(&path, &provider).unwrap();
        store.conn.execute_batch("PRAGMA journal_mode=WAL").unwrap();
        store
            .import_external(&[history_row("sealed-link", None, "scope-a", "body")])
            .unwrap();
        let identity = evidence_identity(&store, "sealed-link", "scope-a");

        let race = |commit: &'static str, subjects: [&'static str; 2]| {
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
            let mut handles = Vec::new();
            for subject in subjects {
                let path = path.clone();
                let provider = provider.clone();
                let identity = identity.clone();
                let barrier = barrier.clone();
                handles.push(std::thread::spawn(move || {
                    let thread_store =
                        Store::open_with_store_instance_id(&path, &provider).unwrap();
                    barrier.wait();
                    thread_store.link_git_in_scope(&identity, commit, subject)
                }));
            }
            barrier.wait();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        };

        let same = race("same-commit", ["same subject", "same subject"]);
        assert_eq!(
            same.iter()
                .filter(|result| matches!(
                    result,
                    ScopedCommitLinkResolution::Ok {
                        outcome: ScopedCommitLinkOutcome::Created,
                        ..
                    }
                ))
                .count(),
            1
        );
        assert!(same.iter().any(|result| matches!(
            result,
            ScopedCommitLinkResolution::Ok {
                outcome: ScopedCommitLinkOutcome::ExactReplay,
                ..
            } | ScopedCommitLinkResolution::Error {
                code: ScopedCommitLinkErrorCode::StoreUnavailable,
                retryable: true,
                ..
            }
        )));

        let different = race("different-commit", ["first subject", "second subject"]);
        assert_eq!(
            different
                .iter()
                .filter(|result| matches!(
                    result,
                    ScopedCommitLinkResolution::Ok {
                        outcome: ScopedCommitLinkOutcome::Created,
                        ..
                    }
                ))
                .count(),
            1
        );
        assert!(different.iter().any(|result| matches!(
            result,
            ScopedCommitLinkResolution::Error {
                code: ScopedCommitLinkErrorCode::LinkConflict,
                ..
            } | ScopedCommitLinkResolution::Error {
                code: ScopedCommitLinkErrorCode::StoreUnavailable,
                retryable: true,
                ..
            }
        )));
        let stored = store.linked_commits("sealed-link").unwrap();
        assert_eq!(stored.len(), 3);
        assert_eq!(
            stored
                .iter()
                .filter(|git_ref| git_ref.0 == "same-commit")
                .count(),
            1
        );
        assert_eq!(
            stored
                .iter()
                .filter(|git_ref| git_ref.0 == "different-commit")
                .count(),
            1
        );

        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn commit_links_page_exact_hashes_and_fail_closed_authority() {
        let store = temp_store();
        store
            .import_external(&[
                history_row("link-a", None, "scope-a", "a"),
                history_row("link-b", None, "scope-a", "b"),
                history_row("link-c", None, "scope-a", "c"),
                history_row("link-case", None, "scope-a", "case"),
                history_row("link-prefix", None, "scope-a", "prefix"),
                history_row("link-suffix", None, "scope-a", "suffix"),
                history_row("foreign-link", None, "scope-b", "foreign"),
                history_row("retired-link", Some("current-link"), "scope-a", "retired"),
                history_row("current-link", None, "scope-a", "current"),
            ])
            .unwrap();
        for (id, commit, subject) in [
            ("link-c", "ExactHash", "subject c"),
            ("link-a", "ExactHash", "subject a"),
            ("link-b", "ExactHash", "subject b"),
            ("link-case", "exacthash", "case variant"),
            ("link-prefix", "xExactHash", "prefix variant"),
            ("link-suffix", "ExactHashx", "suffix variant"),
            ("foreign-link", "ExactHash", "foreign mixed scope"),
            ("foreign-link", "foreign-only", "foreign only"),
            ("retired-link", "retired-commit", "historical evidence"),
        ] {
            store.link_git(id, commit, subject).unwrap();
        }
        store
            .conn
            .execute(
                "INSERT INTO decision_git_refs
                 (decision_id,commit_hash,commit_subject)
                 VALUES ('orphan-id','orphan-only','orphan')",
                [],
            )
            .unwrap();

        let first = store
            .get_commit_links("scope-a", "ExactHash", None, 2)
            .unwrap();
        let CommitLinksResolution::Ok {
            items, next_cursor, ..
        } = first
        else {
            panic!("expected first commit-link page");
        };
        assert_eq!(
            items
                .iter()
                .map(|item| item.record_id.as_str())
                .collect::<Vec<_>>(),
            ["link-a", "link-b"]
        );
        assert_eq!(items[0].commit_subject, "subject a");
        assert_eq!(next_cursor.as_deref(), Some("link-c"));

        let second = store
            .get_commit_links("scope-a", "ExactHash", Some("link-c"), 2)
            .unwrap();
        let CommitLinksResolution::Ok {
            items, next_cursor, ..
        } = second
        else {
            panic!("expected second commit-link page");
        };
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].record_id, "link-c");
        assert_eq!(next_cursor, None);

        for exact_variant in ["exacthash", "xExactHash", "ExactHashx"] {
            let CommitLinksResolution::Ok { items, .. } = store
                .get_commit_links("scope-a", exact_variant, None, 20)
                .unwrap()
            else {
                panic!("expected isolated exact-hash result");
            };
            assert_eq!(items.len(), 1);
        }

        let error_shape = |resolution| match resolution {
            CommitLinksResolution::Error { code, message, .. } => (code, message),
            CommitLinksResolution::Ok { .. } => panic!("expected commit-link error"),
        };
        let absent = error_shape(
            store
                .get_commit_links("scope-a", "absent", None, 20)
                .unwrap(),
        );
        for (scope, commit) in [
            ("scope-missing", "ExactHash"),
            ("scope-a", "foreign-only"),
            ("scope-a", "orphan-only"),
        ] {
            assert_eq!(
                error_shape(store.get_commit_links(scope, commit, None, 20).unwrap()),
                absent
            );
        }
        assert!(!absent.1.contains("foreign-only"));
        assert!(!absent.1.contains("orphan-id"));

        assert!(matches!(
            store
                .get_commit_links("scope-a", "ExactHash", Some("link-case"), 2)
                .unwrap(),
            CommitLinksResolution::Error {
                code: CommitLinksErrorCode::InvalidCursor,
                ..
            }
        ));
        store.link_git("link-a", "removed-cursor", "a").unwrap();
        store.link_git("link-b", "removed-cursor", "b").unwrap();
        let CommitLinksResolution::Ok { next_cursor, .. } = store
            .get_commit_links("scope-a", "removed-cursor", None, 1)
            .unwrap()
        else {
            panic!("expected cursor fixture page");
        };
        let removed = next_cursor.unwrap();
        store
            .conn
            .execute(
                "DELETE FROM decision_git_refs
                 WHERE decision_id=?1 AND commit_hash='removed-cursor'",
                params![removed],
            )
            .unwrap();
        assert!(matches!(
            store
                .get_commit_links("scope-a", "removed-cursor", Some(&removed), 1)
                .unwrap(),
            CommitLinksResolution::Error {
                code: CommitLinksErrorCode::InvalidCursor,
                ..
            }
        ));

        let CommitLinksResolution::Ok { items, .. } = store
            .get_commit_links("scope-a", "retired-commit", None, 20)
            .unwrap()
        else {
            panic!("expected historical direct link");
        };
        assert_eq!(items[0].record_id, "retired-link");
        let CurrentRecordResolution::Ok { current_id, .. } = store
            .get_current_evidence_at("retired-link", now_epoch(), 64)
            .unwrap()
        else {
            panic!("expected current resolution");
        };
        assert_eq!(current_id, "current-link");
    }

    #[test]
    fn commit_links_reject_oversized_subject_and_aggregate() {
        let store = temp_store();
        let mut rows = Vec::new();
        for index in 0..20 {
            rows.push(history_row(
                &format!("budget-{index:02}"),
                None,
                "scope-a",
                "body",
            ));
        }
        rows.push(history_row("oversized-subject", None, "scope-a", "body"));
        store.import_external(&rows).unwrap();
        store
            .conn
            .execute(
                "INSERT INTO decision_git_refs
                 (decision_id,commit_hash,commit_subject) VALUES (?1,?2,?3)",
                params![
                    "oversized-subject",
                    "oversized-subject-commit",
                    "s".repeat(MAX_COMMIT_LINK_SUBJECT_BYTES + 1)
                ],
            )
            .unwrap();
        assert!(matches!(
            store
                .get_commit_links("scope-a", "oversized-subject-commit", None, 20)
                .unwrap(),
            CommitLinksResolution::Error {
                code: CommitLinksErrorCode::ResponseTooLarge,
                ..
            }
        ));

        let bounded_subject = "e".repeat(MAX_COMMIT_LINK_SUBJECT_BYTES);
        for index in 0..20 {
            store
                .link_git(
                    &format!("budget-{index:02}"),
                    "aggregate-budget",
                    &bounded_subject,
                )
                .unwrap();
        }
        assert!(matches!(
            store
                .get_commit_links("scope-a", "aggregate-budget", None, 20)
                .unwrap(),
            CommitLinksResolution::Error {
                code: CommitLinksErrorCode::ResponseTooLarge,
                ..
            }
        ));
    }

    #[test]
    fn commit_links_use_one_snapshot_despite_concurrent_matching_insert() {
        let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "open-why-commit-links-snapshot-{}-{n}",
                std::process::id()
            ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("links.db");
        let store = Store::open_with_embedder_and_store_instance_id(
            &path,
            None,
            &format!("provider:links:{n}"),
        )
        .unwrap();
        store
            .conn
            .execute_batch("PRAGMA journal_mode=WAL;")
            .unwrap();
        store
            .import_external(&[
                history_row("snapshot-a", None, "scope-a", "a"),
                history_row("snapshot-b", None, "scope-a", "b"),
                history_row("snapshot-c", None, "scope-a", "c"),
            ])
            .unwrap();
        store.link_git("snapshot-a", "snapshot-hash", "a").unwrap();
        store.link_git("snapshot-c", "snapshot-hash", "c").unwrap();
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch("PRAGMA journal_mode=WAL;").unwrap();

        let resolution = store
            .get_commit_links_with_hook("scope-a", "snapshot-hash", None, 20, || {
                writer.execute(
                    "INSERT INTO decision_git_refs
                     (decision_id,commit_hash,commit_subject)
                     VALUES ('snapshot-b','snapshot-hash','b')",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        let CommitLinksResolution::Ok { items, .. } = resolution else {
            panic!("expected coherent snapshot");
        };
        assert_eq!(
            items
                .iter()
                .map(|item| item.record_id.as_str())
                .collect::<Vec<_>>(),
            ["snapshot-a", "snapshot-c"]
        );
        let CommitLinksResolution::Ok { items, .. } = store
            .get_commit_links("scope-a", "snapshot-hash", None, 20)
            .unwrap()
        else {
            panic!("expected live post-commit snapshot");
        };
        assert_eq!(
            items
                .iter()
                .map(|item| item.record_id.as_str())
                .collect::<Vec<_>>(),
            ["snapshot-a", "snapshot-b", "snapshot-c"]
        );
        drop(writer);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn active_search_excludes_future_and_expired_temporal_records() {
        let store = temp_store();
        let insert = |id: &str, from: &str, until: Option<&str>| {
            let mut row = history_row(id, None, "scope-a", "temporal sentinel");
            row.title = "temporal sentinel".to_owned();
            row.valid_from = Some(from.to_owned());
            row.valid_until = until.map(str::to_owned);
            store.import_external(&[row]).unwrap();
        };
        insert("current", "2000-01-01T00:00:00Z", None);
        insert("future", "2999-01-01T00:00:00Z", None);
        insert(
            "expired",
            "2000-01-01T00:00:00Z",
            Some("2001-01-01T00:00:00Z"),
        );

        let active = store
            .search_records("temporal sentinel", &["scope-a"], &[], 10)
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, "current");

        let historical = store
            .search_records_with("temporal sentinel", &["scope-a"], &[], 10, true)
            .unwrap();
        assert_eq!(historical.len(), 3);
    }

    #[test]
    fn explain_reports_components_and_drops() {
        let store = temp_store();
        for i in 0..3 {
            store
                .capture(
                    &decision(&format!("sqlite postgres {i}"), "both terms", 0.5, None),
                    "global",
                    None,
                )
                .unwrap();
        }
        let explained = store
            .search_records_explain("sqlite postgres", &["global"], &[], 3, false)
            .unwrap();
        assert_eq!(explained.len(), 3);
        assert!(explained.iter().all(|(_, e)| e.lexical_rank.is_some()));
        assert!(explained.iter().all(|(_, e)| e.semantic_rank.is_none()));
        assert!(explained.iter().all(|(_, e)| e.rrf_score > 0.0));
        let (results, drops) = store
            .search_records_drops("sqlite postgres", &["global"], &[], 1, false, 5)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(drops.len(), 2);
    }

    #[test]
    fn record_digest_v1_has_stable_null_unicode_and_float_vectors() {
        let base = RecordDigestRow {
            id: "record\0id".to_owned(),
            scope: "/repo/alpha".to_owned(),
            kind: "decision".to_owned(),
            title: "Use SQLite".to_owned(),
            content: "local-first\nreason".to_owned(),
            importance: -0.0,
            source: "capture".to_owned(),
            author: "agent".to_owned(),
            commit_sha: "abc123".to_owned(),
            date: "2026-09-03T12:34:56Z".to_owned(),
            tags: None,
            fact_key: None,
            valid_from: None,
            declared_valid_until: None,
            sealed_digest: None,
        };
        let mut unicode = base.clone();
        unicode.id = "记录-β".to_owned();
        unicode.title = "为什么 SQLite?".to_owned();
        unicode.importance = 0.125;
        unicode.tags = Some("[\"β\",\"alpha\",\"\"]".to_owned());
        unicode.fact_key = Some(String::new());
        unicode.valid_from = Some("2026-09-03T12:34:56.123Z".to_owned());
        unicode.declared_valid_until = Some(String::new());

        assert_eq!(
            (
                record_digest_v1(&base).unwrap(),
                record_digest_v1(&unicode).unwrap()
            ),
            (
                "a68e21b2b9e9ed1d5f2ebb0c47390b1c06a927589b8d7ac8e6a3dbafa9412bd7".to_owned(),
                "e7393a1403bffa15cfb05e72f40e8ba177984adbd39751f7ff9dbf8f50e8efaa".to_owned(),
            )
        );

        let mut left = base.clone();
        left.title = "a\0b".to_owned();
        left.content = "c".to_owned();
        let mut right = base;
        right.title = "a".to_owned();
        right.content = "b\0c".to_owned();
        assert_ne!(
            record_digest_v1(&left).unwrap(),
            record_digest_v1(&right).unwrap()
        );
    }

    #[test]
    fn migration_failure_rolls_back_every_foundation_effect_and_retries() {
        let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "open-why-migration-rollback-{}-{n}",
                std::process::id()
            ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("legacy.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(LEGACY_SCHEMA_V0_SQL).unwrap();
        conn.execute_batch(
            "INSERT INTO decisions
               (id,kind,title,content,importance,source,author,commit_sha,date,scope,
                valid_until,content_digest,source_identity,created_epoch)
             VALUES ('legacy','decision','Legacy','body',0.5,'import','author','',
                     '2025-01-01','repo-a','2027-01-01','old','legacy',1);",
        )
        .unwrap();
        let schema_snapshot = |conn: &Connection| {
            let mut statement = conn
                .prepare(
                    "SELECT type,name,tbl_name,COALESCE(sql,'') FROM sqlite_schema
                     ORDER BY type,name,tbl_name",
                )
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        let decisions_snapshot = |conn: &Connection| {
            conn.query_row(
                "SELECT hex(CAST(id AS BLOB)),hex(CAST(title AS BLOB)),
                        hex(CAST(content AS BLOB)),hex(CAST(valid_until AS BLOB))
                 FROM decisions WHERE id='legacy'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .unwrap()
        };
        let fts_snapshot = |conn: &Connection| {
            conn.query_row(
                "SELECT count(*),COALESCE(group_concat(rowid || ':' || title,','),'')
                 FROM decisions_fts",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .unwrap()
        };
        let schema_before = schema_snapshot(&conn);
        let decisions_before = decisions_snapshot(&conn);
        let fts_before = fts_snapshot(&conn);
        let bytes_before = std::fs::read(&path).unwrap();
        let sidecars = [
            PathBuf::from(format!("{}-journal", path.display())),
            PathBuf::from(format!("{}-wal", path.display())),
            PathBuf::from(format!("{}-shm", path.display())),
        ];
        assert!(sidecars.iter().all(|sidecar| !sidecar.exists()));
        let store = Store {
            conn,
            embedder: None,
            _store_parent: None,
        };
        let error = store
            .migrate_with_hook(Some("provider:rollback"), |_| {
                anyhow::bail!("injected migration crash")
            })
            .unwrap_err();
        assert!(error.to_string().contains("injected migration crash"));
        assert_eq!(schema_snapshot(&store.conn), schema_before);
        assert_eq!(decisions_snapshot(&store.conn), decisions_before);
        assert_eq!(fts_snapshot(&store.conn), fts_before);
        assert_eq!(std::fs::read(&path).unwrap(), bytes_before);
        assert!(sidecars.iter().all(|sidecar| !sidecar.exists()));
        let version: u32 = store
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 0);
        assert!(!object_exists(&store.conn, "table", "open_why_metadata").unwrap());
        assert!(!object_exists(&store.conn, "table", "open_why_migrations").unwrap());
        assert!(matches!(
            inspect_connection(&store.conn),
            StoreCompatibility::MigrationRequired { .. }
        ));
        store
            .migrate_with_provider_identity(Some("provider:rollback"))
            .unwrap();
        let first = store.store_identity().unwrap();
        store
            .migrate_with_provider_identity(Some("provider:rollback"))
            .unwrap();
        assert_eq!(store.store_identity().unwrap(), first);
        assert!(matches!(
            inspect_store(&path).unwrap(),
            StoreCompatibility::Compatible { identity } if identity == first
        ));
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
