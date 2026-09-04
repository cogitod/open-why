use super::*;

pub(super) const CORE_SCHEMA_V1_SQL: &str = "CREATE TABLE IF NOT EXISTS decisions (
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

pub(super) const FEEDBACK_SCHEMA_V1_SQL: &str = "CREATE TABLE IF NOT EXISTS feedback_log (
    id TEXT PRIMARY KEY,
    memory_id TEXT NOT NULL,
    helpful INTEGER NOT NULL,
    delta REAL NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
 );
 CREATE INDEX IF NOT EXISTS idx_feedback_log_memory ON feedback_log(memory_id);";

pub(super) const FTS_SCHEMA_V1_SQL: &str =
    "CREATE VIRTUAL TABLE IF NOT EXISTS decisions_fts USING fts5(
    scope, title, content, tags,
    content=decisions, content_rowid=rowid
 );";

pub(super) const FTS_TRIGGERS_V1_SQL: &str = "CREATE TRIGGER IF NOT EXISTS decisions_fts_ai
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

pub(super) const IDENTITY_SCHEMA_V1_SQL: &str = "CREATE TABLE IF NOT EXISTS open_why_migrations (
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

pub(super) const IDENTITY_TRIGGERS_V1_SQL: &str =
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

pub(super) const LEGACY_SCHEMA_V0_SQL: &str = "CREATE TABLE decisions (
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

pub(super) const MIGRATION_STEPS: &[(&str, &str)] = &[
    ("0001-core-store", CORE_SCHEMA_V1_SQL),
    ("0002-feedback", FEEDBACK_SCHEMA_V1_SQL),
    ("0003-search", FTS_SCHEMA_V1_SQL),
    ("0004-search-triggers", FTS_TRIGGERS_V1_SQL),
    ("0005-identity-foundation", IDENTITY_SCHEMA_V1_SQL),
    ("0006-identity-guards", IDENTITY_TRIGGERS_V1_SQL),
];

pub(super) const REQUIRED_DECISION_COLUMNS: &[&str] = &[
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

pub(super) const REQUIRED_OBJECTS: &[(&str, &str)] = &[
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

pub(super) fn schema_sha256_on(conn: &Connection) -> Result<String> {
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

pub(super) fn expected_schema_sha256_v1() -> Result<String> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(CORE_SCHEMA_V1_SQL)?;
    conn.execute_batch(FEEDBACK_SCHEMA_V1_SQL)?;
    conn.execute_batch(FTS_SCHEMA_V1_SQL)?;
    conn.execute_batch(FTS_TRIGGERS_V1_SQL)?;
    conn.execute_batch(IDENTITY_SCHEMA_V1_SQL)?;
    conn.execute_batch(IDENTITY_TRIGGERS_V1_SQL)?;
    schema_sha256_on(&conn)
}

pub(super) fn expected_legacy_schema_sha256_v0() -> Result<String> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(LEGACY_SCHEMA_V0_SQL)?;
    schema_sha256_on(&conn)
}

fn normalize_schema_sql(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn migration_plan_digest() -> String {
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

pub(super) fn append_required(canonical: &mut Vec<u8>, name: &str, value: &[u8]) {
    canonical.extend_from_slice(&(name.len() as u64).to_be_bytes());
    canonical.extend_from_slice(name.as_bytes());
    canonical.push(1);
    canonical.extend_from_slice(&(value.len() as u64).to_be_bytes());
    canonical.extend_from_slice(value);
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
