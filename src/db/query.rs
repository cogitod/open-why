use super::*;

impl Store {
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
    pub(super) fn lexical_indices(
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
}
