use super::*;

impl Store {
    /// Move every record in one scope to another, resealing each record's identity.
    ///
    /// A record's scope is the second canonical field of `record_digest_v1`, and
    /// `decisions_identity_update_guard` refuses to let it change. That is deliberate: records
    /// are immutable so a silent edit cannot pass itself off as history.
    ///
    /// This is the one sanctioned exception, and it exists for a single situation — a store whose
    /// scopes were written wrong by an import, where every affected record is unreachable by
    /// scope and the alternative is a corpus nobody can retrieve. It stays narrow on purpose:
    ///
    /// - Only `scope` and `record_digest_v1` change. Content, identity, lifecycle and supersession
    ///   are untouched, and no record is created or destroyed.
    /// - Each digest is recomputed from the record's own fields, so every row stays
    ///   self-consistent and independently verifiable afterwards. This is a reseal, not a bypass:
    ///   a record that was tampered with before the call still fails verification after it.
    /// - The guard is dropped and restored inside one transaction, so any failure — including a
    ///   crash — rolls back to a guarded store rather than leaving an unguarded one behind.
    ///
    /// Deciding *which* scope a record belongs to is the caller's. This crate cannot know what an
    /// importer meant by a given scope string and deliberately does not guess.
    ///
    /// Returns the number of records moved. Moving a scope to itself, or one that holds no
    /// records, is a no-op that writes nothing, which makes repeated runs safe.
    pub fn rescope(&self, from: &str, to: &str) -> Result<usize> {
        anyhow::ensure!(
            !from.is_empty() && !to.is_empty(),
            "rescope requires both a source and a destination scope"
        );
        if from == to {
            return Ok(0);
        }

        let tx = self.conn.unchecked_transaction()?;
        let ids: Vec<String> = {
            let mut statement =
                tx.prepare("SELECT id FROM decisions WHERE scope=?1 ORDER BY id")?;
            let rows = statement.query_map(params![from], |row| row.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        if ids.is_empty() {
            tx.rollback()?;
            return Ok(0);
        }

        tx.execute_batch("DROP TRIGGER IF EXISTS decisions_identity_update_guard;")?;
        let mut moved = 0usize;
        for id in &ids {
            let Some(mut row) = Self::record_digest_row_in_scope_on(&tx, id, from)? else {
                continue;
            };
            row.scope = to.to_owned();
            let resealed = record_digest_v1(&row)?;
            moved += tx.execute(
                "UPDATE decisions SET scope=?1, record_digest_v1=?2 WHERE id=?3 AND scope=?4",
                params![to, resealed, id, from],
            )?;
        }
        // Restored from the schema's own definition, so the guard that comes back is exactly the
        // one the schema declares rather than a copy that could drift.
        tx.execute_batch(IDENTITY_TRIGGERS_V1_SQL)?;
        tx.commit()?;
        Ok(moved)
    }
}
