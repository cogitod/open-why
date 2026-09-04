use super::*;

pub(super) fn scoped_commit_link_error(
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

pub(super) fn store_error_is_retryable(error: &anyhow::Error) -> bool {
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

pub(super) fn record_digest_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RecordDigestRow> {
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

pub(super) fn record_digest_row_from_external(row: &ExternalDecision) -> RecordDigestRow {
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

pub(super) fn ensure_exact_record_replay(
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

pub(super) struct PendingRetirement {
    pub(super) retirement_at: String,
    pub(super) expected_valid_from: Option<String>,
    pub(super) expected_valid_until: Option<String>,
}

pub(super) fn pending_retirement_time_on(
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

pub(super) fn ensure_acyclic_retirement_on(
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

pub(super) fn record_digest_v1(row: &RecordDigestRow) -> Result<String> {
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

pub(super) fn scoped_current_resolution(
    read: CurrentEvidenceRead,
) -> ScopedCurrentRecordResolution {
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
