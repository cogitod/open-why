use super::*;
use rusqlite::backup::Backup;
use std::time::Duration;

impl Store {
    /// Create a consistent SQLite snapshot at a new destination path.
    ///
    /// The source remains open and committed WAL state is included. The
    /// destination is never overwritten; an unsuccessful backup removes the
    /// newly created database file before returning the error.
    pub fn backup_to(&self, destination: &Path) -> Result<()> {
        self.backup_to_with(destination, |source, target| {
            Backup::new(source, target)?.run_to_completion(128, Duration::from_millis(1), None)?;
            Ok(())
        })
    }

    pub(super) fn backup_to_with(
        &self,
        destination: &Path,
        copy: impl FnOnce(&Connection, &mut Connection) -> Result<()>,
    ) -> Result<()> {
        let prepared = crate::private_store_path::prepare_new(destination)
            .with_context(|| format!("prepare backup destination {}", destination.display()))?;
        let result = (|| {
            #[cfg(unix)]
            let open_flags = crate::private_store_path::sqlite_open_flags(
                OpenFlags::SQLITE_OPEN_READ_WRITE
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX
                    | OpenFlags::SQLITE_OPEN_URI,
            );
            #[cfg(not(unix))]
            let open_flags = OpenFlags::SQLITE_OPEN_READ_WRITE;
            let mut target = prepared
                .open_connection(|path| Connection::open_with_flags(path, open_flags))
                .with_context(|| format!("open backup destination {}", destination.display()))?;
            self.verify_file()?;
            copy(&self.conn, &mut target)?;
            verify_sqlite_integrity(&target)?;
            prepared.verify_connection_target(&target)?;
            let source_identity = self.store_identity()?;
            let result = match inspect_connection(&target) {
                StoreCompatibility::Compatible { identity } if identity == source_identity => {
                    Ok(())
                }
                other => anyhow::bail!("backup snapshot failed identity validation: {other:?}"),
            };
            prepared.verify_connection_target(&target)?;
            result
        })();
        if let Err(error) = result {
            if let Err(cleanup) = prepared.remove_created_file() {
                anyhow::bail!("{error:#}; failed to remove incomplete backup: {cleanup:#}");
            }
            return Err(error);
        }
        Ok(())
    }
}

impl Store {
    /// Read an existing current-schema store without creating, migrating, or loading models.
    /// SQLite may maintain WAL sidecars when opening a live WAL database.
    pub fn open_existing_read_only(path: &Path, expected_identity: Option<&str>) -> Result<Self> {
        let prepared = crate::private_store_path::prepare(path, false, false)?
            .context("store does not exist; maintenance never creates a source store")?;
        let flags = crate::private_store_path::sqlite_open_flags(
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_URI,
        );
        let conn = prepared.open_connection(|path| Connection::open_with_flags(path, flags))?;
        conn.pragma_update(None, "query_only", true)?;
        let store = Self {
            conn,
            embedder: None,
            _store_parent: prepared.into_parent_guard(),
        };
        let identity = store.store_identity()?;
        if let Some(expected) = expected_identity {
            anyhow::ensure!(
                identity.store_instance_id == expected,
                "identity_mismatch: maintenance source does not match configured identity"
            );
        }
        Ok(store)
    }

    /// Check SQLite structure, foreign keys, schema/ledger and every sealed record digest.
    /// This detects corruption; it does not authenticate the original author or certify truth.
    pub fn verify_integrity(&self) -> Result<StoreIdentity> {
        self.verify_file()?;
        let identity = self.store_identity()?;
        verify_sqlite_integrity(&self.conn)?;
        let mut stmt = self.conn.prepare("SELECT id, scope FROM decisions")?;
        let records =
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for record in records {
            let (id, scope) = record?;
            anyhow::ensure!(
                matches!(
                    self.evidence_identity_in_scope(&id, &scope)?,
                    EvidenceIdentityResolution::Ok { .. }
                ),
                "record evidence digest validation failed"
            );
        }
        self.verify_file()?;
        Ok(identity)
    }
}

fn verify_sqlite_integrity(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA integrity_check")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    for row in rows {
        anyhow::ensure!(row? == "ok", "SQLite integrity check failed");
    }
    anyhow::ensure!(
        !conn.prepare("PRAGMA foreign_key_check")?.exists([])?,
        "SQLite foreign key check failed"
    );
    Ok(())
}
