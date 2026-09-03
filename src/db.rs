use crate::store::{Decision, ExternalDecision, Record};
use crate::embed::Embedder;
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

    /// Best-effort embedding of the searchable text. Mirrors cogitod's `embeddingText`:
    /// `title\ncontent`, then the space-joined tag array when present. Returns the JSON vector
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
             );",
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

    /// Native FTS5 lexical index, mirroring cogitod's `memories_fts` external-content table
    /// (migrations 001/044/049): columns `scope, title, content, tags`, synced by triggers,
    /// ranked by `bm25(decisions_fts, 0, 10, 5, 1)` — scope weight 0, title 10, content 5,
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
        // index being empty while the content table has rows — the FTS5 external-content
        // `'rebuild'` command is unreliable against a TEXT-primary-key content table, so
        // backfill with the same explicit insert shape the triggers use.
        let idx_count: i64 = self.conn.query_row("SELECT count(*) FROM decisions_fts_idx", [], |r| r.get(0))?;
        let content_count: i64 = self.conn.query_row("SELECT count(*) FROM decisions", [], |r| r.get(0))?;
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
            self.conn.execute_batch(&format!("ALTER TABLE decisions ADD COLUMN {column} {ty};"))?;
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
        let commit = if d.kind == "commit" { d.sha.clone() } else { String::new() };
        let now = now_epoch();
        let now_str = epoch_to_iso(now);
        self.conn.execute(
            "INSERT OR IGNORE INTO decisions
               (id, kind, title, content, importance, source, author, commit_sha, date, scope,
                content_digest, source_identity, created_epoch)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![id, d.kind, d.subject, d.body, importance, d.source, d.author, commit, now_str, scope, content_digest, identity, now],
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

    /// Capture a decision with an externally-minted id (a cogitod memory UUID) and an
    /// explicit validity start. Idempotent by the external id: re-capturing the same id
    /// returns it without a duplicate. `supersedes` retires an older decision.
    /// `fact_key` and title matches retire the current same-key / same-title record
    /// (point-in-time supersession, mirroring cogitod's keyed + title predecessor rule).
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
        let commit = if d.kind == "commit" { d.sha.clone() } else { String::new() };
        let now = now_epoch();
        let now_str = epoch_to_iso(now);
        let vfrom = valid_from.map(String::from).unwrap_or_else(|| now_str.clone());
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
        // shares the fact_key or the (kind, title) — the same rule cogitod applies.
        let mut predecessors: Vec<String> = supersedes
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .into_iter()
            .collect();
        let keyed: Vec<String> = match fact_key.as_deref() {
            Some(key) => self.conn.prepare(
                "SELECT id FROM decisions WHERE scope=?1 AND kind=?2 AND fact_key=?3
                   AND id != ?4 AND superseded_by IS NULL AND valid_until IS NULL",
            )?.query_map(params![scope, d.kind, key, id], |r| r.get(0))?
                .filter_map(|r| r.ok()).collect(),
            None => Vec::new(),
        };
        let titled: Vec<String> = self.conn.prepare(
            "SELECT id FROM decisions WHERE scope=?1 AND kind=?2 AND title=?3
               AND id != ?4 AND superseded_by IS NULL AND valid_until IS NULL",
        )?.query_map(params![scope, d.kind, d.subject, id], |r| r.get(0))?
            .filter_map(|r| r.ok()).collect();
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
    pub fn search(&self, query: &str, scopes: &[&str], kinds: &[String], limit: usize) -> Result<Vec<Decision>> {
        self.search_with(query, scopes, kinds, limit, false)
    }

    /// `search` with supersession control. `include_superseded` relaxes the active-only filter so
    /// retired decisions surface too — the historical arm of "what changed and why".
    pub fn search_with(&self, query: &str, scopes: &[&str], kinds: &[String], limit: usize, include_superseded: bool) -> Result<Vec<Decision>> {
        if scopes.is_empty() {
            return Ok(Vec::new());
        }
        let (rows, rowids) = self.select_decisions(scopes, kinds, include_superseded)?;
        let lexical = self.lexical_indices(query, &rowids, scopes, kinds, limit, include_superseded)?;
        let qe = self.query_embedding(query);
        Ok(rank(query, qe.as_deref(), rows, lexical, now_epoch(), limit))
    }

    /// Fetch candidate rows with their integer rowids, in scope and kind order. The
    /// rowid is the join key between the semantic candidates and the FTS5 lexical index.
    fn select_decisions(&self, scopes: &[&str], kinds: &[String], include_superseded: bool) -> Result<(Vec<Decision>, Vec<i64>)> {
        let validity = if include_superseded { "" } else { " AND superseded_by IS NULL AND valid_until IS NULL" };
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
    /// mapped to indices into `rowids`. Mirrors cogitod's `lexicalSearchIds` + RRF position.
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
        Ok(ordered.iter().filter_map(|r| index.get(r).copied()).collect())
    }

    /// Run the FTS5 lexical query (narrow-then-broad over quoted terms) and return the matched
    /// rowids ordered by `bm25(decisions_fts, 0, 10, 5, 1)`. This is the exact engine cogitod's
    /// `MemoryRepository.lexicalSearchIds` runs, so parity is a query shape, not a re-derivation.
    fn lexical_rowids(&self, query: &str, scopes: &[&str], kinds: &[String], limit: usize, include_superseded: bool) -> Result<Vec<i64>> {
        let terms = crate::search::tokenize(query);
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let quoted: Vec<String> = terms.iter().map(|t| format!("\"{}\"", t.replace('"', ""))).collect();
        let narrow_floor = limit.min(5);
        let overfetch = limit.saturating_mul(10).max(limit);

        let validity = if include_superseded { "" } else { " AND d.superseded_by IS NULL AND d.valid_until IS NULL" };
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
                 FROM decisions WHERE id=?1 AND superseded_by IS NULL",
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
        let mut stmt = self.conn.prepare(
            "SELECT commit_hash, commit_subject FROM decision_git_refs
             WHERE decision_id=?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![decision_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Search active decisions across scopes and return full records (id + temporal
    /// window) in hybrid-ranked order. Structured counterpart of `search`.
    pub fn search_records(&self, query: &str, scopes: &[&str], kinds: &[String], limit: usize) -> Result<Vec<Record>> {
        self.search_records_with(query, scopes, kinds, limit, false)
    }

    /// `search_records` with supersession control. With `include_superseded`, retired decisions
    /// surface too and carry their `superseded_by` / `valid_until` so a caller can follow the chain.
    pub fn search_records_with(&self, query: &str, scopes: &[&str], kinds: &[String], limit: usize, include_superseded: bool) -> Result<Vec<Record>> {
        Ok(self.rank_records(query, scopes, kinds, limit, include_superseded)?.0)
    }

    /// `search_records_with` returning per-result ranking explanations alongside.
    pub fn search_records_explain(&self, query: &str, scopes: &[&str], kinds: &[String], limit: usize, include_superseded: bool) -> Result<Explained> {
        let (records, explanations) = self.rank_records(query, scopes, kinds, limit, include_superseded)?;
        Ok(records.into_iter().zip(explanations).collect())
    }

    /// Search and split into `(results, drops)`: the top `limit` and the next `drop_count`
    /// near-miss candidates, each with its ranking explanation. The drops are the candidates
    /// that fused but lost the top-N slice — "what didn't make it, and by how much".
    pub fn search_records_drops(&self, query: &str, scopes: &[&str], kinds: &[String], limit: usize, include_superseded: bool, drop_count: usize) -> Result<(Explained, Explained)> {
        let (records, explanations) = self.rank_records(query, scopes, kinds, limit + drop_count, include_superseded)?;
        let pairs: Vec<(Record, RankExplanation)> = records.into_iter().zip(explanations).collect();
        let (results, drops) = pairs.split_at(pairs.len().min(limit));
        Ok((results.to_vec(), drops.to_vec()))
    }

    fn rank_records(&self, query: &str, scopes: &[&str], kinds: &[String], limit: usize, include_superseded: bool) -> Result<(Vec<Record>, Vec<RankExplanation>)> {
        if scopes.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let (rows, rowids) = self.select_records(scopes, kinds, include_superseded)?;
        let lexical = self.lexical_indices(query, &rowids, scopes, kinds, limit, include_superseded)?;
        let qe = self.query_embedding(query);
        Ok(rank_by(query, qe.as_deref(), rows, lexical, now_epoch(), limit, |d| RankRow {
            importance: d.importance,
            kind: &d.kind,
            date: &d.date,
            updated_at: if d.updated_at.is_empty() { None } else { Some(&d.updated_at) },
            access_count: d.access_count,
            effectiveness: d.effectiveness,
            embedding: d.embedding.as_deref(),
            title: &d.title,
            content: &d.content,
        }))
    }

    fn select_records(&self, scopes: &[&str], kinds: &[String], include_superseded: bool) -> Result<(Vec<Record>, Vec<i64>)> {
        let validity = if include_superseded { "" } else { " AND superseded_by IS NULL AND valid_until IS NULL" };
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
        let validity = if include_superseded { "" } else { " AND superseded_by IS NULL" };
        let sql = format!(
            "SELECT id,kind,title,content,importance,source,author,commit_sha,date,scope,
                    superseded_by,valid_from,valid_until,updated_at,
                    COALESCE(accessed_count,0)+COALESCE(times_injected,0), effectiveness
             FROM decisions WHERE id=?1{validity}"
        );
        Ok(self
            .conn
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

    /// Walk the supersession chain forward from `id` — `[id, superseded_by(id), superseded_by(...)…]`
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

    pub fn link_git(&self, decision_id: &str, commit_hash: &str, commit_subject: &str) -> Result<()> {
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
                let commit = if d.kind == "commit" { d.sha.clone() } else { String::new() };
                let importance = d.importance.clamp(0.0, 1.0);
                let epoch = iso_to_epoch(&d.date).unwrap_or(0);
                stmt.execute(params![
                    id, d.kind, d.subject, d.body, importance, d.source, d.author, commit,
                    d.date, scope, content_digest, identity, epoch
                ])?;
            }
        }
        tx.commit()?;
        Ok(decisions.len())
    }

    pub fn count_for_scope(&self, scope: &str) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM decisions WHERE scope=?1 AND superseded_by IS NULL",
            params![scope],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// Record explicit retrieval feedback on a decision — the closing half of the usage→quality
    /// loop. A helpful verdict raises the record's effectiveness and a not-helpful verdict lowers
    /// it, mirroring cogitod's `context_retrieval_feedback` `FEEDBACK_SET_SQL`: the delta lands on
    /// the effective value (ungraded prior 0.5), clamped to `[0.01, 1.0]`, and `updated_at` is
    /// bumped so the verdict also moves recency. Returns the new effectiveness, or `None` when the
    /// id is unknown or superseded.
    pub fn feedback(&self, id: &str, helpful: bool) -> Result<Option<f64>> {
        let delta = if helpful { 0.05 } else { -0.03 };
        let updated = self.conn.execute(
            "UPDATE decisions SET
               times_helpful = COALESCE(times_helpful, 0) + ?1,
               effectiveness = MIN(1.0, MAX(0.01, effectiveness + ?2)),
               updated_at = datetime('now')
             WHERE id = ?3 AND superseded_by IS NULL",
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
        let eff: f64 = self
            .conn
            .query_row("SELECT effectiveness FROM decisions WHERE id=?1", params![id], |r| r.get(0))?;
        Ok(Some(eff))
    }
}

/// RRF fusion constant (Cormack et al. 2009), matching cogitod's `RRF_K`.
const RRF_K: f64 = 60.0;
/// BM25 leads the inline fusion in cogitod (arXiv 2605.15184, Table 1).
const BM25_WEIGHT: f64 = 1.5;
/// Hybrid rerank weights (sim / importance / effectiveness), matching cogitod.
const RERANK_W_SIM: f64 = 0.65;
const RERANK_W_IMPORTANCE: f64 = 0.25;
const RERANK_W_EFFECTIVENESS: f64 = 0.10;
/// Floor under recency decay, matching cogitod's `RECENCY_DECAY_FLOOR`: an old-but-best match
/// must stay reachable rather than being buried to zero by age alone.
const RECENCY_DECAY_FLOOR: f64 = 0.3;
const RECENCY_HALF_LIFE_DAYS: f64 = 7.0;
const RECENCY_HALF_LIFE_DECISION_DAYS: f64 = 2.0;
/// Query-conditional recency weighting (mem0's temporal-reasoning idea), matching cogitod.
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
        "originally", "first", "initial", "initially", "previously", "formerly", "history",
        "historical", "past", "earlier", "before",
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

/// Per-result ranking explanation — why a row ranked where it did. Exposed by `--explain`.
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

/// Hybrid rerank matching cogitod's `searchMemoriesHybrid`: reciprocal-rank fusion of a
/// semantic arm (sorted by hybrid score) and a lexical arm (the FTS5 `bm25()` order supplied by
/// the caller, already narrow-then-broad), then slice. Recency enters through the semantic arm's
/// hybrid score — floored, so age cannot bury a best match — never as a multiplicative gate on
/// the fused score.
fn rank(query: &str, query_embedding: Option<&[f32]>, rows: Vec<Decision>, lexical_order: Vec<usize>, now: i64, limit: usize) -> Vec<Decision> {
    rank_by(query, query_embedding, rows, lexical_order, now, limit, |d| RankRow {
        importance: d.importance,
        kind: &d.kind,
        date: &d.date,
        updated_at: if d.updated_at.is_empty() { None } else { Some(&d.updated_at) },
        access_count: d.access_count,
        effectiveness: d.effectiveness,
        embedding: d.embedding.as_deref(),
        title: &d.subject,
        content: &d.body,
    })
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
            let lexical_gate_score = crate::relevance::lexical_score(query, f.content, &lexical_text);
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
        (RERANK_W_SIM * c.sim + RERANK_W_IMPORTANCE * c.importance
            + RERANK_W_EFFECTIVENESS * c.effectiveness)
            * decay
            * recency_mult
    };

    let n = capsules.len();

    // Semantic arm mirrors cogitod's ANN KNN: keep only the nearest-by-cosine rows (the semantic
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
            eprintln!("  LEX[{rank}] sim={:.3} lex_gate={:.4} fused={:.5}", c.sim, c.lexical_gate_score, scores[i]);
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
    // Post-fusion relevance gate (mirrors cogitod's `MemoryRelevanceGate`): drop candidates
    // that cleared BM25/RRF fusion but are not actually relevant to the query, before the
    // final score sort — so a filtered-out noise row can't block a genuine match from the
    // top-N slice. Must run on the full fused set, not just the eventual top `limit`.
    order.retain(|&i| crate::relevance::passes(capsules[i].sim, capsules[i].lexical_gate_score));
    order.sort_by(|&a, &b| scores[b].partial_cmp(&scores[a]).unwrap_or(std::cmp::Ordering::Equal));
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

fn epoch_to_iso(secs: i64) -> String {
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

    fn decision(subject: &str, body: &str, importance: f64, embedding: Option<Vec<f32>>) -> Decision {
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
        // "feline" shares no token with "cat", but its embedding matches — semantic
        // similarity must rank it first and must not require a lexical hit.
        let rows = vec![
            decision("feline", "a small domesticated animal", 0.5, Some(vec![1.0, 0.0])),
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
        // Age must never bury a correct answer to zero — the decay asymptotes at 0.3.
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
                .capture(&decision(&format!("sqlite postgres {i}"), "both", 0.5, None), "global", None)
                .unwrap();
        }
        store
            .capture(&decision("sqlite sqlite sqlite sqlite", "no second token", 0.5, None), "global", None)
            .unwrap();
        let hits = store.search("sqlite postgres", &["global"], &[], 10).unwrap();
        assert_eq!(hits.len(), 5);
        assert!(hits.iter().all(|h| h.subject.contains("postgres")));
    }

    #[test]
    fn fts5_lexical_narrow_then_broad_falls_back() {
        // Only one all-terms row (< narrow floor), so the arm broadens to OR and the
        // partial-match row still surfaces.
        let store = temp_store();
        store
            .capture(&decision("sqlite postgres", "both", 0.5, None), "global", None)
            .unwrap();
        store
            .capture(&decision("sqlite", "only one term", 0.5, None), "global", None)
            .unwrap();
        let hits = store.search("sqlite postgres", &["global"], &[], 10).unwrap();
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn fts5_lexical_orders_multi_term_match_first() {
        // The row matching both query terms must outrank the row matching only one. FTS5 bm25()
        // handles idf + length normalisation natively — this is delegated to SQLite, not derived.
        let store = temp_store();
        store
            .capture(&decision("worktree long", &("node_modules ".repeat(300)), 0.5, None), "global", None)
            .unwrap();
        store
            .capture(&decision("worktree corruption", "corruption", 0.5, None), "global", None)
            .unwrap();
        let hits = store.search("worktree corruption", &["global"], &[], 10).unwrap();
        assert_eq!(hits[0].subject, "worktree corruption");
    }

    #[test]
    fn feedback_moves_effectiveness_and_is_clamped() {
        let store = temp_store();
        let id = store
            .capture(&decision("use sqlite", "single file local-first", 0.5, None), "global", None)
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
            .capture_external(&decision("database choice", "sqlite", 0.5, None), "global", "aaa", None, None, None)
            .unwrap();
        store
            .capture_external(&decision("database choice v2", "postgres now", 0.5, None), "global", "bbb", None, None, Some("aaa"))
            .unwrap();
        // Active search returns only the current (non-superseded) record.
        let hits = store.search("sqlite postgres", &["global"], &[], 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].subject, "database choice v2");
        // Historical search returns both.
        let hits = store.search_records_with("sqlite postgres", &["global"], &[], 10, true).unwrap();
        assert_eq!(hits.len(), 2);
        // The chain walks aaa -> bbb.
        let chain = store.supersession_chain("aaa", 20).unwrap();
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].id, "aaa");
        assert_eq!(chain[1].id, "bbb");
    }

    #[test]
    fn explain_reports_components_and_drops() {
        let store = temp_store();
        for i in 0..3 {
            store
                .capture(&decision(&format!("sqlite postgres {i}"), "both terms", 0.5, None), "global", None)
                .unwrap();
        }
        let explained = store.search_records_explain("sqlite postgres", &["global"], &[], 3, false).unwrap();
        assert_eq!(explained.len(), 3);
        assert!(explained.iter().all(|(_, e)| e.lexical_rank.is_some()));
        assert!(explained.iter().all(|(_, e)| e.semantic_rank.is_none()));
        assert!(explained.iter().all(|(_, e)| e.rrf_score > 0.0));
        let (results, drops) = store.search_records_drops("sqlite postgres", &["global"], &[], 1, false, 5).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(drops.len(), 2);
    }
}
