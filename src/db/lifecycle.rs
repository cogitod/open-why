use super::*;

impl Store {
    pub(super) fn open_with_embedder_and_identity(
        path: &Path,
        embedder: Option<Box<dyn Embedder>>,
        store_instance_id: Option<&str>,
    ) -> Result<Store> {
        let prepared = match crate::private_store_path::prepare(path, false, true)? {
            Some(prepared) => prepared,
            None => {
                require_store_instance_id(store_instance_id)?;
                crate::private_store_path::prepare(path, true, true)?
                    .context("new store path was not prepared")?
            }
        };
        #[cfg(unix)]
        let open_flags = crate::private_store_path::sqlite_open_flags(
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_URI,
        );
        #[cfg(not(unix))]
        let open_flags = OpenFlags::default();
        let conn = prepared
            .open_connection(|sqlite_path| Connection::open_with_flags(sqlite_path, open_flags))
            .with_context(|| format!("open {}", path.display()))?;
        let store_parent = prepared.into_parent_guard();
        let store = Store {
            conn,
            embedder,
            _store_parent: store_parent,
        };
        store.migrate_with_provider_identity(store_instance_id)?;
        Ok(store)
    }

    pub(super) fn migrate_with_provider_identity(
        &self,
        store_instance_id: Option<&str>,
    ) -> Result<()> {
        self.migrate_with_hook(store_instance_id, |_| Ok(()))
    }

    pub(super) fn migrate_with_hook(
        &self,
        store_instance_id: Option<&str>,
        before_commit: impl FnOnce(&Connection) -> Result<()>,
    ) -> Result<()> {
        match inspect_connection(&self.conn) {
            StoreCompatibility::Compatible { identity } => {
                return verify_store_binding(&identity, store_instance_id)
            }
            StoreCompatibility::Missing => anyhow::bail!("store path disappeared during migration"),
            StoreCompatibility::Uninitialized | StoreCompatibility::MigrationRequired { .. } => {}
            StoreCompatibility::Incompatible { code, message, .. } => {
                anyhow::bail!("store compatibility {code:?}: {message}")
            }
        }

        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let migration_required = match inspect_connection(&tx) {
            StoreCompatibility::Compatible { identity } => {
                verify_store_binding(&identity, store_instance_id)?;
                tx.rollback()?;
                return Ok(());
            }
            StoreCompatibility::Uninitialized => false,
            StoreCompatibility::MigrationRequired { .. } => true,
            StoreCompatibility::Missing => {
                anyhow::bail!("store path disappeared during migration")
            }
            StoreCompatibility::Incompatible { code, message, .. } => {
                anyhow::bail!("store compatibility {code:?}: {message}")
            }
        };
        let store_instance_id = require_store_instance_id(store_instance_id)?;
        if migration_required {
            Self::rebuild_legacy_v0_on(&tx)?;
        }
        Self::create_core_schema_on(&tx)?;
        Self::create_feedback_schema_on(&tx)?;
        Self::ensure_fts_on(&tx)?;
        Self::create_identity_schema_on(&tx)?;

        tx.execute(
            "UPDATE decisions
             SET declared_valid_until=valid_until
             WHERE declared_valid_until IS NULL AND valid_until IS NOT NULL",
            [],
        )?;
        Self::backfill_record_digests_on(&tx)?;
        Self::ensure_identity_triggers_on(&tx)?;

        Self::append_migration_ledger_on(&tx)?;
        let schema_sha256 = expected_schema_sha256_v1()?;
        anyhow::ensure!(
            schema_sha256_on(&tx)? == schema_sha256,
            "migrated store schema differs from the build-known v1 schema"
        );
        tx.execute(
            "INSERT INTO open_why_metadata
               (singleton,schema_family,schema_version,schema_sha256,migration_plan_digest,store_instance_id)
             VALUES (1,?1,?2,?3,?4,?5)",
            params![
                STORE_SCHEMA_FAMILY,
                STORE_SCHEMA_VERSION,
                schema_sha256,
                migration_plan_digest(),
                store_instance_id
            ],
        )?;
        tx.pragma_update(None, "user_version", STORE_SCHEMA_VERSION)?;
        before_commit(&tx)?;

        match inspect_connection(&tx) {
            StoreCompatibility::Compatible { .. } => {}
            other => anyhow::bail!("migrated store failed validation: {other:?}"),
        }
        tx.commit()?;
        Ok(())
    }

    fn rebuild_legacy_v0_on(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "DROP TRIGGER IF EXISTS decisions_fts_ai;
             DROP TRIGGER IF EXISTS decisions_fts_ad;
             DROP TRIGGER IF EXISTS decisions_fts_au;
             DROP TABLE IF EXISTS decisions_fts;
             ALTER TABLE decisions RENAME TO decisions_v0;
             DROP INDEX idx_decisions_identity;
             DROP INDEX idx_decisions_scope;",
        )?;
        Self::create_core_schema_on(conn)?;
        conn.execute_batch(
            "INSERT INTO decisions (
                rowid,id,kind,title,content,importance,source,author,commit_sha,date,scope,
                superseded_by,valid_from,valid_until,fact_key,embedding,content_digest,
                source_identity,created_epoch,updated_at,accessed_count,times_injected,
                effectiveness,tags,times_helpful,declared_valid_until,record_digest_v1
             )
             SELECT rowid,id,kind,title,content,importance,source,author,commit_sha,date,scope,
                superseded_by,valid_from,valid_until,fact_key,embedding,content_digest,
                source_identity,created_epoch,updated_at,accessed_count,times_injected,
                effectiveness,tags,times_helpful,valid_until,NULL
             FROM decisions_v0;
             DROP TABLE decisions_v0;",
        )?;
        Ok(())
    }

    fn append_migration_ledger_on(conn: &Connection) -> Result<()> {
        for (index, (migration_id, payload)) in MIGRATION_STEPS.iter().enumerate() {
            let sequence = i64::try_from(index + 1)?;
            let checksum = sha256_hex(payload.as_bytes());
            let existing: Option<(String, String)> = conn
                .query_row(
                    "SELECT migration_id,checksum_sha256 FROM open_why_migrations
                     WHERE sequence=?1",
                    params![sequence],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            match existing {
                Some((id, found)) if id == *migration_id && found == checksum => {}
                Some(_) => anyhow::bail!("migration ledger conflicts at sequence {sequence}"),
                None => {
                    conn.execute(
                        "INSERT INTO open_why_migrations
                           (sequence,migration_id,checksum_sha256,applied_at)
                         VALUES (?1,?2,?3,datetime('now'))",
                        params![sequence, migration_id, checksum],
                    )?;
                }
            }
        }
        Ok(())
    }

    fn create_core_schema_on(conn: &Connection) -> Result<()> {
        conn.execute_batch(CORE_SCHEMA_V1_SQL)?;
        Ok(())
    }

    fn create_feedback_schema_on(conn: &Connection) -> Result<()> {
        conn.execute_batch(FEEDBACK_SCHEMA_V1_SQL)?;
        Ok(())
    }

    fn create_identity_schema_on(conn: &Connection) -> Result<()> {
        conn.execute_batch(IDENTITY_SCHEMA_V1_SQL)?;
        Ok(())
    }

    /// Native FTS5 external-content lexical index with `scope`, `title`, `content`, and
    /// `tags` columns, synchronized by triggers,
    /// ranked by `bm25(decisions_fts, 0, 10, 5, 1)`: scope weight 0, title 10, content 5,
    /// tags 1. This makes the lexical arm byte-for-byte the same engine the TS side calls.
    fn ensure_fts_on(conn: &Connection) -> Result<()> {
        conn.execute_batch(FTS_SCHEMA_V1_SQL)?;
        Self::ensure_fts_triggers_on(conn)?;
        // Backfill stores created before the FTS index existed. Detect it by the inverted
        // index being empty while the content table has rows. The FTS5 external-content
        // `'rebuild'` command is unreliable against a TEXT-primary-key content table, so
        // backfill with the same explicit insert shape the triggers use.
        let idx_count: i64 =
            conn.query_row("SELECT count(*) FROM decisions_fts_idx", [], |r| r.get(0))?;
        let content_count: i64 =
            conn.query_row("SELECT count(*) FROM decisions", [], |r| r.get(0))?;
        if idx_count == 0 && content_count > 0 {
            conn.execute_batch("DROP TABLE IF EXISTS decisions_fts;")?;
            conn.execute_batch(FTS_SCHEMA_V1_SQL)?;
            Self::ensure_fts_triggers_on(conn)?;
            conn.execute_batch(
                "INSERT INTO decisions_fts(rowid, scope, title, content, tags)
                 SELECT rowid, scope, title, content, tags FROM decisions;",
            )?;
        }
        Ok(())
    }

    fn ensure_fts_triggers_on(conn: &Connection) -> Result<()> {
        conn.execute_batch(FTS_TRIGGERS_V1_SQL)?;
        Ok(())
    }

    fn ensure_identity_triggers_on(conn: &Connection) -> Result<()> {
        conn.execute_batch(IDENTITY_TRIGGERS_V1_SQL)?;
        Ok(())
    }

    fn backfill_record_digests_on(conn: &Connection) -> Result<()> {
        let rows = Self::record_digest_rows_on(conn)?;
        for row in rows {
            let sealed = record_digest_v1(&row)?;
            conn.execute(
                "UPDATE decisions SET record_digest_v1=?1 WHERE id=?2",
                params![sealed, row.id],
            )?;
        }
        Ok(())
    }

    fn record_digest_rows_on(conn: &Connection) -> Result<Vec<RecordDigestRow>> {
        let mut stmt = conn.prepare(
            "SELECT id,scope,kind,title,content,importance,source,author,commit_sha,date,tags,fact_key,
                    valid_from,declared_valid_until,record_digest_v1
             FROM decisions ORDER BY id",
        )?;
        let rows = stmt.query_map([], record_digest_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub(super) fn record_digest_row_in_scope_on(
        conn: &Connection,
        id: &str,
        scope: &str,
    ) -> Result<Option<RecordDigestRow>> {
        Ok(conn
            .query_row(
                "SELECT id,scope,kind,title,content,importance,source,author,commit_sha,date,tags,fact_key,
                        valid_from,declared_valid_until,record_digest_v1
                 FROM decisions WHERE id=?1 AND scope=?2",
                params![id, scope],
                record_digest_row,
            )
            .optional()?)
    }

    pub(super) fn record_digest_row_by_id_on(
        conn: &Connection,
        id: &str,
    ) -> Result<Option<RecordDigestRow>> {
        Ok(conn
            .query_row(
                "SELECT id,scope,kind,title,content,importance,source,author,commit_sha,date,tags,fact_key,
                        valid_from,declared_valid_until,record_digest_v1
                 FROM decisions WHERE id=?1",
                params![id],
                record_digest_row,
            )
            .optional()?)
    }

    pub(super) fn evidence_identity_on(
        conn: &Connection,
        id: &str,
        scope: &str,
    ) -> Result<EvidenceIdentityResolution> {
        let fail = |code, message| EvidenceIdentityResolution::Error {
            contract: EVIDENCE_IDENTITY_CONTRACT,
            code,
            message,
            retryable: false,
        };
        let Some(row) = Self::record_digest_row_in_scope_on(conn, id, scope)? else {
            return Ok(fail(
                EvidenceIdentityErrorCode::NotFound,
                "record was not found in the requested scope".to_owned(),
            ));
        };
        let Some(sealed) = row.sealed_digest.as_deref() else {
            return Ok(fail(
                EvidenceIdentityErrorCode::IdentityConflict,
                "record identity does not match its sealed evidence".to_owned(),
            ));
        };
        let calculated = record_digest_v1(&row).ok();
        if calculated.as_deref() != Some(sealed) {
            return Ok(fail(
                EvidenceIdentityErrorCode::IdentityConflict,
                "record identity does not match its sealed evidence".to_owned(),
            ));
        }
        let store_instance_id: String = conn.query_row(
            "SELECT store_instance_id FROM open_why_metadata WHERE singleton=1",
            [],
            |record| record.get(0),
        )?;
        Ok(EvidenceIdentityResolution::Ok {
            identity: EvidenceIdentity {
                contract: EVIDENCE_IDENTITY_CONTRACT,
                record_digest_contract: RECORD_DIGEST_CONTRACT,
                store_instance_id,
                scope: row.scope,
                record_id: row.id,
                record_digest: sealed.to_owned(),
            },
        })
    }

    /// Return the persistent provider-owned store and verified schema identity.
    pub fn store_identity(&self) -> Result<StoreIdentity> {
        let tx = self.conn.unchecked_transaction()?;
        let compatibility = inspect_connection(&tx);
        tx.rollback()?;
        match compatibility {
            StoreCompatibility::Compatible { identity } => Ok(identity),
            StoreCompatibility::Incompatible { code, message, .. } => {
                anyhow::bail!("store compatibility {code:?}: {message}")
            }
            other => anyhow::bail!("open store is not schema-compatible: {other:?}"),
        }
    }

    /// Return the sealed identity for an exact record in an exact scope.
    pub fn evidence_identity_in_scope(
        &self,
        id: &str,
        scope: &str,
    ) -> Result<EvidenceIdentityResolution> {
        let tx = self.conn.unchecked_transaction()?;
        let resolution = Self::evidence_identity_on(&tx, id, scope)?;
        tx.rollback()?;
        Ok(resolution)
    }
}
