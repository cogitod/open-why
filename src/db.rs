use crate::store::{Decision, ExternalDecision, Record};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};

pub fn default_path() -> PathBuf {
    std::env::var("OPEN_WHY_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| crate::store::cache_dir().join("open-why.db"))
}

/// The "why" core store. Owns the decision record (temporal + provenance + git linkage).
pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        let store = Store { conn };
        store.migrate()?;
        Ok(store)
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
        self.ensure_valid_from()?;
        Ok(())
    }

    /// Backward-compatible column add for stores created before `valid_from` existed.
    fn ensure_valid_from(&self) -> Result<()> {
        let has: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('decisions') WHERE name='valid_from'",
            [],
            |r| r.get(0),
        )?;
        if has == 0 {
            self.conn.execute_batch("ALTER TABLE decisions ADD COLUMN valid_from TEXT;")?;
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
    pub fn capture_external(
        &self,
        d: &Decision,
        scope: &str,
        id: &str,
        valid_from: Option<&str>,
        supersedes: Option<&str>,
    ) -> Result<String> {
        let content_digest = digest(&format!("{}\n{}", d.subject, d.body));
        let importance = d.importance.clamp(0.0, 1.0);
        let commit = if d.kind == "commit" { d.sha.clone() } else { String::new() };
        let now = now_epoch();
        let now_str = epoch_to_iso(now);
        let vfrom = valid_from.map(String::from).unwrap_or_else(|| now_str.clone());
        let identity = format!("external:{scope}:{id}");
        self.conn.execute(
            "INSERT OR IGNORE INTO decisions
               (id, kind, title, content, importance, source, author, commit_sha, date, scope,
                valid_from, content_digest, source_identity, created_epoch)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            params![
                id, d.kind, d.subject, d.body, importance, d.source, d.author, commit,
                now_str, scope, vfrom, content_digest, identity, now
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
                    superseded_by, valid_from, valid_until, content_digest, source_identity, created_epoch)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,'',?8,?9,?10,?11,?12,?13,?14,?15)",
            )?;
            for r in rows {
                let content_digest = digest(&format!("{}\n{}", r.title, r.content));
                let identity = format!("external:{}:{}", r.scope, r.id);
                let epoch = iso_to_epoch(&r.date).unwrap_or(now_epoch());
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

    /// Search active decisions across scopes and hybrid-rank them.
    pub fn search(&self, query: &str, scopes: &[&str], limit: usize) -> Result<Vec<Decision>> {
        if scopes.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; scopes.len()].join(",");
        let sql = format!(
            "SELECT kind,title,content,importance,source,author,commit_sha,date
             FROM decisions
             WHERE superseded_by IS NULL AND valid_until IS NULL
               AND scope IN ({placeholders})"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let scope_params: Vec<&dyn rusqlite::ToSql> = scopes.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(scope_params.as_slice(), |r| {
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
        })?;
        let mut all = Vec::new();
        for row in rows {
            all.push(row?);
        }
        Ok(rank(query, all, now_epoch(), limit))
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
    pub fn search_records(&self, query: &str, scopes: &[&str], limit: usize) -> Result<Vec<Record>> {
        if scopes.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; scopes.len()].join(",");
        let sql = format!(
            "SELECT id,kind,title,content,importance,source,author,commit_sha,date,scope,
                    superseded_by,valid_from,valid_until
             FROM decisions
             WHERE superseded_by IS NULL AND valid_until IS NULL
               AND scope IN ({placeholders})"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let scope_params: Vec<&dyn rusqlite::ToSql> = scopes.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(scope_params.as_slice(), |r| {
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
            })
        })?;
        let mut all = Vec::new();
        for row in rows {
            all.push(row?);
        }
        Ok(rank_by(query, all, now_epoch(), limit, |d| (&d.title, &d.content, d.importance, &d.date, &d.kind)))
    }

    pub fn get_record(&self, id: &str) -> Result<Option<Record>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id,kind,title,content,importance,source,author,commit_sha,date,scope,
                        superseded_by,valid_from,valid_until
                 FROM decisions WHERE id=?1 AND superseded_by IS NULL",
                params![id],
                |r| {
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
                    })
                },
            )
            .optional()?)
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
}

/// Hybrid rerank: 0.65 similarity + 0.25 importance, × Ebbinghaus recency decay.
/// Effectiveness (0.10 in cogitod) is folded out until grading exists — it is a
/// constant prior that never changes ordering. Similarity is a lexical proxy here;
/// embeddings land in P2. Decisions decay with a 2-day half-life (point-in-time),
/// everything else 7 days.
fn rank(query: &str, rows: Vec<Decision>, now: i64, limit: usize) -> Vec<Decision> {
    rank_by(query, rows, now, limit, |d| (&d.subject, &d.body, d.importance, &d.date, &d.kind))
}

fn rank_by<T>(
    query: &str,
    rows: Vec<T>,
    now: i64,
    limit: usize,
    fields: impl Fn(&T) -> (&str, &str, f64, &str, &str),
) -> Vec<T> {
    let words = crate::search::tokenize(query);
    let mut scored: Vec<(f64, T)> = rows
        .into_iter()
        .filter_map(|d| {
            let (subject, body, importance, date, kind) = fields(&d);
            let lex = crate::search::score(&words, subject, body) as f64;
            if lex <= 0.0 {
                return None;
            }
            let sim = lex / (lex + 10.0);
            let mut s = 0.65 * sim + 0.25 * importance;
            if let Some(epoch) = iso_to_epoch(date) {
                let age_days = ((now - epoch) as f64 / 86_400.0).max(0.0);
                let half_life = if kind == "decision" { 2.0 } else { 7.0 };
                s *= 2.0f64.powf(-age_days / half_life);
            }
            Some((s, d))
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);
    scored.into_iter().map(|(_, d)| d).collect()
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
