use super::*;

impl Store {
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

    pub(super) fn capture_external_with_pre_retirement_hook<F>(
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
}
