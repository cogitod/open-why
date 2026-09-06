use super::*;

impl Store {
    /// Reconcile one exact external supersession observed after the predecessor
    /// was imported. Both sealed records must already exist in the same scope.
    /// Exact replay is a no-op; conflicting or invalid lifecycle state fails
    /// before effect.
    pub fn reconcile_external_supersession(&self, transition: &ExternalSupersession) -> Result<()> {
        if transition.predecessor_id == transition.successor_id {
            return Err(SupersessionCycle.into());
        }
        if transition.predecessor_id.is_empty()
            || transition.successor_id.is_empty()
            || transition.scope.is_empty()
            || transition.valid_until.is_empty()
            || transition.valid_until.len() > MAX_TEMPORAL_VALUE_BYTES
        {
            return Err(CurrentRecordErrorCode::InvalidTemporalData.into());
        }
        let retirement_epoch = iso_to_epoch(&transition.valid_until)
            .ok_or(CurrentRecordErrorCode::InvalidTemporalData)?;
        let tx = self.conn.unchecked_transaction()?;
        let predecessor = Self::record_digest_row_in_scope_on(
            &tx,
            &transition.predecessor_id,
            &transition.scope,
        )?
        .ok_or(SupersessionTargetNotFound)?;
        let successor =
            Self::record_digest_row_in_scope_on(&tx, &transition.successor_id, &transition.scope)?
                .ok_or(SupersessionTargetNotFound)?;
        ensure_exact_record_replay(&predecessor, &predecessor)?;
        ensure_exact_record_replay(&successor, &successor)?;

        let state: (Option<String>, Option<String>, Option<String>) = tx.query_row(
            "SELECT superseded_by,valid_from,valid_until FROM decisions
             WHERE id=?1 AND scope=?2",
            params![transition.predecessor_id, transition.scope],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        if state.0.as_deref() == Some(transition.successor_id.as_str())
            && state.2.as_deref() == Some(transition.valid_until.as_str())
        {
            tx.rollback()?;
            return Ok(());
        }
        if state.0.is_some() || state.2.is_some() {
            return Err(SupersessionConflict.into());
        }
        if state
            .1
            .as_deref()
            .and_then(iso_to_epoch)
            .is_some_and(|epoch| epoch > retirement_epoch)
        {
            return Err(CurrentRecordErrorCode::InvalidTemporalData.into());
        }
        let successor_valid_from: Option<String> = tx.query_row(
            "SELECT valid_from FROM decisions WHERE id=?1 AND scope=?2",
            params![transition.successor_id, transition.scope],
            |row| row.get(0),
        )?;
        if successor_valid_from
            .as_deref()
            .and_then(iso_to_epoch)
            .is_some_and(|epoch| epoch > retirement_epoch)
        {
            return Err(CurrentRecordErrorCode::InvalidTemporalData.into());
        }
        ensure_acyclic_retirement_on(
            &tx,
            &transition.successor_id,
            &transition.predecessor_id,
            &transition.scope,
        )?;
        let changed = tx.execute(
            "UPDATE decisions SET superseded_by=?1,valid_until=?2
             WHERE id=?3 AND scope=?4 AND superseded_by IS NULL AND valid_until IS NULL",
            params![
                transition.successor_id,
                transition.valid_until,
                transition.predecessor_id,
                transition.scope
            ],
        )?;
        if changed != 1 {
            return Err(SupersessionConflict.into());
        }
        tx.commit()?;
        Ok(())
    }
}
