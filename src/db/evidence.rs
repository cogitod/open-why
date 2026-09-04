use super::*;

impl Store {
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

    pub(super) fn get_commit_links_with_hook(
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

    pub(super) fn get_current_evidence_at_with_scope_and_hook(
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

    pub(super) fn get_rationale_history_at_with_hook(
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
}
