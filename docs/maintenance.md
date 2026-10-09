# Backup, restore, and recovery

Use absolute, symlink-free paths. Commands print JSON on success and exit nonzero
on failure. They never download models, create a missing source, or migrate a
source schema. `OPEN_WHY_STORE_INSTANCE_ID`, when set, must match the source.
A backup contains your records; protect it like the original database.

```bash
export OPEN_WHY_DB="$HOME/.cache/open-why/open-why.db"
why backup --to "$HOME/open-why-backup.db"
why verify-backup "$HOME/open-why-backup.db"
why restore "$HOME/open-why-backup.db" --to "$HOME/open-why-restored.db"
why doctor --db "$HOME/open-why-restored.db"
```

Choose new destination filenames for every backup or restore. Existing files,
symlinks, and destination daemon endpoints are refused. There is deliberately no
`--force` or in-place restore: no destructive confirmation is necessary because
these commands cannot overwrite existing data. The restored store has the same
identity, records, history and evidence digests as the snapshot. It is a copy of
the same logical store, not an independently minted store identity.

Verification checks the schema and migration ledger, SQLite integrity and foreign
keys, and each record's sealed digest. It detects corruption, not authenticity:
someone able to rewrite the database can also rewrite stored digests. Keep an
independently stored SHA-256 if you need to detect changes to the snapshot file.

## Running processes

Backup uses SQLite's online snapshot API and includes committed WAL data while
a daemon or another writer is running. Verification and backup can encounter
SQLite locks; retry after the writer completes. A read-only connection to a live
WAL database may maintain SQLite sidecars. Never copy only the live `.db` file or
delete `-wal` / `-shm` files to make an error disappear.

To switch to the restored copy, stop your daemon's supervisor and disconnect all
MCP clients first. Keep the original store, run `why setup --db` on the restored
path, and replace each client's configuration with the printed path and identity.
Restart the daemon with that same configuration, then reconnect clients. Do not
rename a replacement database underneath a live daemon; its connections reject a
changed physical file.

Daemon endpoints now append `.sock` to the complete database filename, e.g.
`open-why.db.sock`. Old daemons used extension replacement (`open-why.sock`).
New clients do not reuse that legacy endpoint. Stop old daemons before upgrading.
A stale endpoint causes a clear failure rather than silently selecting another
transport. After stopping its supervisor and verifying no daemon owns that
endpoint, remove **only that socket**, then restart. Never remove a socket based
only on its age. A regular file at an endpoint path is never automatically removed.
Long database paths may exceed your OS's Unix socket limit; use a shorter path
for daemon mode. Direct stdio remains available when no endpoint exists.

## Interrupted operations and upgrades

An interrupted snapshot can leave a new incomplete destination; never use it
until `verify-backup` succeeds. Preserve it for diagnosis or choose another new
path and retry. Existing source data is not overwritten. Process-abort tests are
not evidence of safety against power loss, broken storage hardware or a hostile
process running as the same OS user.

Before upgrading: create and verify a backup, stop clients/daemons, install the
chosen version, and test it against a restored copy. Normal opens may migrate
recognized older schemas transactionally. Maintenance commands require the
current compatible schema and refuse unknown or legacy layouts without
modification. Use the old executable to make a backup before migration. A rollback
uses the old executable and its pre-upgrade backup at a new path; it must not
attempt to downgrade a newer database in place.
