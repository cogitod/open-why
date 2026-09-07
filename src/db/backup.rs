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
            copy(&self.conn, &mut target)?;
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
