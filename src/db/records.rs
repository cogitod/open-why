use super::*;

impl Store {
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

    pub(super) fn get_record_any_on(
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
