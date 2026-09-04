use crate::embed::Embedder;
use crate::store::{
    CommitLinkItem, CommitLinksErrorCode, CommitLinksResolution, CurrentRecordErrorCode,
    CurrentRecordResolution, Decision, ExternalDecision, GitRef, RationaleHistoryErrorCode,
    RationaleHistoryRecord, RationaleHistoryResolution, Record, COMMIT_LINKS_CONTRACT,
    CURRENT_RATIONALE_CONTRACT, MAX_COMMIT_LINKS_PAGE_RECORDS, MAX_COMMIT_LINKS_PAGE_SOURCE_BYTES,
    MAX_COMMIT_LINK_RECORD_ID_BYTES, MAX_COMMIT_LINK_SUBJECT_BYTES, MAX_HISTORY_PAGE_GIT_REFS,
    MAX_HISTORY_PAGE_RECORDS, MAX_HISTORY_PAGE_SOURCE_BYTES, MAX_SUPERSESSION_CHAIN,
    RATIONALE_HISTORY_CONTRACT,
};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub fn default_path() -> PathBuf {
    std::env::var("OPEN_WHY_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| crate::store::cache_dir().join("open-why.db"))
}

/// The "why" core store. Owns the decision record (temporal + provenance + git linkage).
pub struct Store {
    conn: Connection,
    embedder: Option<Box<dyn Embedder>>,
}

struct HistoryNode {
    id: String,
    scope: String,
    superseded_by: Option<String>,
    valid_from: Option<String>,
    valid_until: Option<String>,
}

struct HistoryPageRequest<'a> {
    id: &'a str,
    scope: &'a str,
    page_cursor: Option<&'a str>,
    limit: usize,
    as_of: i64,
    chain_cap: usize,
}

impl Store {
    /// Open a store without an embedder (lexical-first). Kept as the explicit no-embedder entry
    /// point; every command path uses `open_default` so the semantic arm is active uniformly.
    #[allow(dead_code)]
    pub fn open(path: &Path) -> Result<Store> {
        Self::open_with_embedder(path, None)
    }

    /// Open the default store, wiring an embedder from the environment when one is configured
    /// (`OPEN_WHY_EMBED_MODEL_PATH` or `OPEN_WHY_EMBED_URL`). This is the entry point every CLI
    /// command and the MCP server use, so the semantic arm is active uniformly.
    pub fn open_default() -> Result<Store> {
        Self::open_with_embedder(&default_path(), crate::embed::from_env()?)
    }

    pub fn open_with_embedder(path: &Path, embedder: Option<Box<dyn Embedder>>) -> Result<Store> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        let store = Store { conn, embedder };
        store.migrate()?;
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

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS decisions (
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
                created_epoch INTEGER NOT NULL DEFAULT 0
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
               ON decision_git_refs(commit_hash, decision_id);",
        )?;
        self.ensure_column("valid_from", "TEXT")?;
        self.ensure_column("fact_key", "TEXT")?;
        self.ensure_column("embedding", "TEXT")?;
        self.ensure_column("updated_at", "TEXT")?;
        self.ensure_column("accessed_count", "INTEGER NOT NULL DEFAULT 0")?;
        self.ensure_column("times_injected", "INTEGER NOT NULL DEFAULT 0")?;
        self.ensure_column("effectiveness", "REAL NOT NULL DEFAULT 0.5")?;
        self.ensure_column("tags", "TEXT")?;
        self.ensure_column("times_helpful", "INTEGER NOT NULL DEFAULT 0")?;
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS feedback_log (
                id TEXT PRIMARY KEY,
                memory_id TEXT NOT NULL,
                helpful INTEGER NOT NULL,
                delta REAL NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
             );
             CREATE INDEX IF NOT EXISTS idx_feedback_log_memory ON feedback_log(memory_id);",
        )?;
        self.ensure_fts()?;
        Ok(())
    }

    /// Native FTS5 external-content lexical index with `scope`, `title`, `content`, and
    /// `tags` columns, synchronized by triggers,
    /// ranked by `bm25(decisions_fts, 0, 10, 5, 1)`: scope weight 0, title 10, content 5,
    /// tags 1. This makes the lexical arm byte-for-byte the same engine the TS side calls.
    fn ensure_fts(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS decisions_fts USING fts5(
               scope, title, content, tags,
               content=decisions, content_rowid=rowid
             );",
        )?;
        self.ensure_fts_triggers()?;
        // Backfill stores created before the FTS index existed. Detect it by the inverted
        // index being empty while the content table has rows. The FTS5 external-content
        // `'rebuild'` command is unreliable against a TEXT-primary-key content table, so
        // backfill with the same explicit insert shape the triggers use.
        let idx_count: i64 =
            self.conn
                .query_row("SELECT count(*) FROM decisions_fts_idx", [], |r| r.get(0))?;
        let content_count: i64 =
            self.conn
                .query_row("SELECT count(*) FROM decisions", [], |r| r.get(0))?;
        if idx_count == 0 && content_count > 0 {
            self.conn.execute_batch(
                "DROP TABLE IF EXISTS decisions_fts;
                 CREATE VIRTUAL TABLE decisions_fts USING fts5(
                   scope, title, content, tags,
                   content=decisions, content_rowid=rowid
                 );",
            )?;
            self.ensure_fts_triggers()?;
            self.conn.execute_batch(
                "INSERT INTO decisions_fts(rowid, scope, title, content, tags)
                 SELECT rowid, scope, title, content, tags FROM decisions;",
            )?;
        }
        Ok(())
    }

    fn ensure_fts_triggers(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TRIGGER IF NOT EXISTS decisions_fts_ai AFTER INSERT ON decisions BEGIN
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
             END;",
        )?;
        Ok(())
    }

    /// Backward-compatible column add for stores created before `column` existed.
    fn ensure_column(&self, column: &str, ty: &str) -> Result<()> {
        let has: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('decisions') WHERE name=?1",
            params![column],
            |r| r.get(0),
        )?;
        if has == 0 {
            self.conn
                .execute_batch(&format!("ALTER TABLE decisions ADD COLUMN {column} {ty};"))?;
        }
        Ok(())
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
        self.conn.execute(
            "INSERT OR IGNORE INTO decisions
               (id, kind, title, content, importance, source, author, commit_sha, date, scope,
                content_digest, source_identity, created_epoch)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                id,
                d.kind,
                d.subject,
                d.body,
                importance,
                d.source,
                d.author,
                commit,
                now_str,
                scope,
                content_digest,
                identity,
                now
            ],
        )?;
        if let Some(sid) = supersedes {
            if !sid.is_empty() {
                self.conn.execute(
                    "UPDATE decisions SET superseded_by=?1, valid_until=?2
                     WHERE id=?3 AND superseded_by IS NULL",
                    params![id, now_str, sid],
                )?;
            }
        }
        let existing: String = self.conn.query_row(
            "SELECT id FROM decisions WHERE source_identity=?1 AND content_digest=?2",
            params![identity, content_digest],
            |r| r.get(0),
        )?;
        Ok(existing)
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
        let embedding = self.embed_text(&d.subject, &d.body, None);
        self.conn.execute(
            "INSERT OR IGNORE INTO decisions
               (id, kind, title, content, importance, source, author, commit_sha, date, scope,
                valid_from, fact_key, embedding, updated_at, content_digest, source_identity, created_epoch)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
            params![
                id, d.kind, d.subject, d.body, importance, d.source, d.author, commit,
                now_str, scope, vfrom, fact_key, embedding, now_str, content_digest, identity, now
            ],
        )?;
        // Retire predecessors: the explicit supersedes id, then any current record that
        // shares the fact_key or the (kind, title).
        let mut predecessors: Vec<String> = supersedes
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .into_iter()
            .collect();
        let keyed: Vec<String> = match fact_key.as_deref() {
            Some(key) => self
                .conn
                .prepare(
                    "SELECT id FROM decisions WHERE scope=?1 AND kind=?2 AND fact_key=?3
                   AND id != ?4 AND superseded_by IS NULL AND valid_until IS NULL",
                )?
                .query_map(params![scope, d.kind, key, id], |r| r.get(0))?
                .filter_map(|r| r.ok())
                .collect(),
            None => Vec::new(),
        };
        let titled: Vec<String> = self
            .conn
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
        for old in predecessors {
            self.conn.execute(
                "UPDATE decisions SET superseded_by=?1, valid_until=?2
                 WHERE id=?3 AND superseded_by IS NULL",
                params![id, now_str, old],
            )?;
        }
        Ok(id.to_string())
    }

    /// Bulk-import externally-minted decisions, preserving ids, temporal windows,
    /// supersession, and git linkage. Idempotent: re-importing the same id replaces it.
    pub fn import_external(&self, rows: &[ExternalDecision]) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO decisions
                   (id, kind, title, content, importance, source, author, commit_sha, date, scope,
                    superseded_by, valid_from, valid_until, fact_key, embedding, updated_at,
                    accessed_count, times_injected, effectiveness, tags, content_digest, source_identity, created_epoch)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,'',?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22)",
            )?;
            for r in rows {
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
                    epoch
                ])?;
            }
        }
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO decision_git_refs (decision_id, commit_hash, commit_subject)
                 VALUES (?1,?2,?3)",
            )?;
            for r in rows {
                for g in &r.git_refs {
                    stmt.execute(params![r.id, g.commit_hash, g.commit_subject])?;
                }
            }
        }
        tx.commit()?;
        Ok(rows.len())
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

    /// Clock-injected implementation used by the MCP server and deterministic tests.
    /// MCP callers never supply `as_of`; the server owns that clock authority.
    pub(crate) fn get_current_evidence_at(
        &self,
        id: &str,
        as_of: i64,
        chain_cap: usize,
    ) -> Result<CurrentRecordResolution> {
        let as_of_iso = epoch_to_iso(as_of);
        let fail = |code, message: String| CurrentRecordResolution::Error {
            contract: CURRENT_RATIONALE_CONTRACT,
            as_of: as_of_iso.clone(),
            requested_id: id.to_string(),
            code,
            message,
            retryable: false,
        };

        let mut chain = Vec::new();
        let mut cursor = id.to_string();
        let mut seen = std::collections::HashSet::new();
        loop {
            if !seen.insert(cursor.clone()) {
                return Ok(fail(
                    CurrentRecordErrorCode::Cycle,
                    format!("supersession cycle reaches `{cursor}`"),
                ));
            }
            let Some(record) = self.get_record_any(&cursor, true)? else {
                let (code, message) = if chain.is_empty() {
                    (
                        CurrentRecordErrorCode::NotFound,
                        format!("record `{id}` was not found"),
                    )
                } else {
                    (
                        CurrentRecordErrorCode::BrokenChain,
                        format!("supersession successor `{cursor}` was not found"),
                    )
                };
                return Ok(fail(code, message));
            };

            for (field, raw) in [
                ("valid_from", record.valid_from.as_deref()),
                ("valid_until", record.valid_until.as_deref()),
            ] {
                if let Some(raw) = raw.filter(|value| !value.is_empty()) {
                    if self.temporal_epoch(raw)?.is_none() {
                        return Ok(fail(
                            CurrentRecordErrorCode::InvalidTemporalData,
                            format!("record `{}` has invalid {field} `{raw}`", record.id),
                        ));
                    }
                }
            }
            if let (Some(valid_from), Some(valid_until)) = (
                record
                    .valid_from
                    .as_deref()
                    .filter(|value| !value.is_empty()),
                record
                    .valid_until
                    .as_deref()
                    .filter(|value| !value.is_empty()),
            ) {
                let from = self.temporal_epoch(valid_from)?.expect("validated above");
                let until = self.temporal_epoch(valid_until)?.expect("validated above");
                if from >= until {
                    return Ok(fail(
                        CurrentRecordErrorCode::InvalidTemporalData,
                        format!(
                            "record `{}` has a non-positive validity interval",
                            record.id
                        ),
                    ));
                }
            }

            let next = record
                .superseded_by
                .as_deref()
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
            chain.push(record);
            if let Some(next) = next {
                if chain.len() >= chain_cap {
                    return Ok(fail(
                        CurrentRecordErrorCode::TraversalLimit,
                        format!("supersession chain exceeds {chain_cap} records"),
                    ));
                }
                cursor = next;
                continue;
            }
            break;
        }

        let current = chain.last().expect("a fetched chain is non-empty").clone();
        if let Some(valid_from) = current.valid_from.as_deref().filter(|v| !v.is_empty()) {
            let epoch = self.temporal_epoch(valid_from)?.expect("validated above");
            if as_of < epoch {
                return Ok(fail(
                    CurrentRecordErrorCode::NotYetValid,
                    format!("record `{}` is not current at {as_of_iso}", current.id),
                ));
            }
        }
        if let Some(valid_until) = current.valid_until.as_deref().filter(|v| !v.is_empty()) {
            let epoch = self.temporal_epoch(valid_until)?.expect("validated above");
            if as_of >= epoch {
                return Ok(fail(
                    CurrentRecordErrorCode::ExpiredWithoutSuccessor,
                    format!(
                        "record `{}` expired without a successor at `{valid_until}`",
                        current.id
                    ),
                ));
            }
        }

        let git_refs = self
            .linked_commits(&current.id)?
            .into_iter()
            .map(|(commit_hash, commit_subject)| GitRef {
                commit_hash,
                commit_subject,
            })
            .collect();
        Ok(CurrentRecordResolution::Ok {
            contract: CURRENT_RATIONALE_CONTRACT,
            as_of: as_of_iso,
            requested_id: id.to_string(),
            current_id: current.id.clone(),
            record: Box::new(current),
            git_refs,
            supersession_chain: chain.into_iter().map(|record| record.id).collect(),
        })
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

    /// Bulk-import mined decisions (commits + ADRs) into a scope. Idempotent.
    pub fn import_decisions(&self, scope: &str, decisions: &[Decision]) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO decisions
                   (id, kind, title, content, importance, source, author, commit_sha, date, scope,
                    content_digest, source_identity, created_epoch)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            )?;
            for d in decisions {
                let identity = if d.kind == "commit" {
                    format!("git:{scope}:commit:{}", d.sha)
                } else {
                    format!("git:{scope}:file:{}", d.source)
                };
                let content_digest = digest(&format!("{}\n{}", d.subject, d.body));
                let id = if d.kind == "commit" && !d.sha.is_empty() {
                    d.sha.clone()
                } else {
                    digest(&format!("{identity}\n{content_digest}"))
                };
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
                    epoch
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
        let updated = self.conn.execute(
            "UPDATE decisions SET
               times_helpful = COALESCE(times_helpful, 0) + ?1,
               effectiveness = MIN(1.0, MAX(0.01, effectiveness + ?2)),
               updated_at = datetime('now')
             WHERE id = ?3 AND superseded_by IS NULL
               AND (valid_from IS NULL OR unixepoch(valid_from) <= unixepoch('now'))
               AND (valid_until IS NULL OR unixepoch(valid_until) > unixepoch('now'))",
            params![if helpful { 1 } else { 0 }, delta, id],
        )?;
        if updated == 0 {
            return Ok(None);
        }
        let log_id = digest(&format!("{id}:{helpful}:{}", now_epoch()));
        self.conn.execute(
            "INSERT INTO feedback_log (id, memory_id, helpful, delta) VALUES (?1,?2,?3,?4)",
            params![log_id, id, if helpful { 1 } else { 0 }, delta],
        )?;
        let eff: f64 = self.conn.query_row(
            "SELECT effectiveness FROM decisions WHERE id=?1",
            params![id],
            |r| r.get(0),
        )?;
        Ok(Some(eff))
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
    if b.len() < 19 {
        return None;
    }
    let n = |i: usize| (b[i] as i64) - 48;
    let y = n(0) * 1000 + n(1) * 100 + n(2) * 10 + n(3);
    let mo = n(5) * 10 + n(6);
    let d = n(8) * 10 + n(9);
    let h = n(11) * 10 + n(12);
    let mi = n(14) * 10 + n(15);
    let se = n(17) * 10 + n(18);
    if !(1970..=9999).contains(&y) || !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
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
        let dir = std::env::temp_dir().join(format!("open-why-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Store::open_with_embedder(&dir.join("t.db"), None).unwrap()
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
            store
                .conn
                .execute(
                    "INSERT INTO decisions
                     (id,kind,title,content,importance,source,author,commit_sha,date,scope,
                      valid_from,valid_until,content_digest,source_identity,created_epoch)
                     VALUES (?1,'decision',?1,'full body',0.5,'synthetic','tester','',
                             '2026-01-01','scope-a',?2,?3,?1,?1,0)",
                    params![id, from, until],
                )
                .unwrap();
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
        store
            .conn
            .execute(
                "INSERT INTO decisions
                 (id,kind,title,content,importance,source,author,commit_sha,date,scope,
                  valid_from,content_digest,source_identity,created_epoch)
                 VALUES ('bad-time','decision','bad time','body',0.5,'synthetic','tester','',
                         '2026-01-01','scope-a','not-a-time','bad-time','bad-time',0)",
                [],
            )
            .unwrap();
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
        store
            .conn
            .execute("DELETE FROM decisions WHERE id='foreign'", [])
            .unwrap();
        let same_id_missing = store
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
        let dir = std::env::temp_dir().join(format!(
            "open-why-history-snapshot-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.db");
        let store = Store::open_with_embedder(&path, None).unwrap();
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
                        "UPDATE decisions SET content='new successor'
                         WHERE id='snapshot-b'",
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
            "new successor"
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
        let dir = std::env::temp_dir().join(format!(
            "open-why-commit-links-snapshot-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("links.db");
        let store = Store::open_with_embedder(&path, None).unwrap();
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
            store
                .conn
                .execute(
                    "INSERT INTO decisions
                     (id,kind,title,content,importance,source,author,commit_sha,date,scope,
                      valid_from,valid_until,content_digest,source_identity,created_epoch)
                     VALUES (?1,'decision','temporal sentinel','temporal sentinel',0.5,
                             'synthetic','tester','','2026-01-01','scope-a',?2,?3,?1,?1,0)",
                    params![id, from, until],
                )
                .unwrap();
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
}
