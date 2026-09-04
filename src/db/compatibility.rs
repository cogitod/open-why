use super::*;

/// Inspect an existing store without creating or migrating any filesystem or
/// database state.
pub(super) fn inspect_store(path: &Path) -> Result<StoreCompatibility> {
    let Some(prepared) = crate::private_store_path::prepare(path, false, false)? else {
        return Ok(StoreCompatibility::Missing);
    };
    let anchored_path = prepared.sqlite_path();
    if store_may_have_live_wal(anchored_path)? {
        return Ok(incompatible(
            StoreCompatibilityErrorCode::LiveWalIndeterminate,
            "store may have committed state in a live WAL and cannot be inspected without side effects",
            None,
        ));
    }
    let uri = immutable_sqlite_uri(anchored_path);
    let inspect_flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_URI;
    let inspect_flags = crate::private_store_path::sqlite_open_flags(inspect_flags);
    let conn = prepared
        .open_connection(|_| Connection::open_with_flags(uri, inspect_flags))
        .with_context(|| format!("inspect {}", path.display()))?;
    conn.pragma_update(None, "query_only", true)?;
    let tx = conn.unchecked_transaction()?;
    let compatibility = inspect_connection(&tx);
    tx.rollback()?;
    if store_may_have_live_wal(anchored_path)? {
        return Ok(incompatible(
            StoreCompatibilityErrorCode::LiveWalIndeterminate,
            "store entered WAL mode during read-only inspection",
            None,
        ));
    }
    Ok(compatibility)
}

pub(super) fn inspect_connection(conn: &Connection) -> StoreCompatibility {
    inspect_connection_inner(conn).unwrap_or_else(|_| StoreCompatibility::Incompatible {
        code: StoreCompatibilityErrorCode::SchemaCorrupt,
        message: "store schema could not be read safely".to_owned(),
        found_version: None,
    })
}

fn immutable_sqlite_uri(path: &Path) -> String {
    let mut uri = String::from("file:");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b':' | b'.' | b'_' | b'-' | b'~' => {
                uri.push(char::from(byte))
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri.push_str("?immutable=1");
    uri
}

fn store_may_have_live_wal(path: &Path) -> Result<bool> {
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", path.display()));
        if sidecar.exists() {
            return Ok(true);
        }
    }
    let mut header = [0_u8; 20];
    let mut file = std::fs::File::open(path)?;
    if file.read(&mut header)? < header.len() || &header[..16] != b"SQLite format 3\0" {
        return Ok(false);
    }
    Ok(header[18] == 2 || header[19] == 2)
}

pub(super) fn require_store_instance_id(store_instance_id: Option<&str>) -> Result<&str> {
    let Some(store_instance_id) = store_instance_id else {
        return Err(StoreIdentityBindingError {
            code: StoreIdentityBindingErrorCode::IdentityRequired,
            message: "an explicit provider store identity is required for initial binding",
        }
        .into());
    };
    if !valid_store_instance_id(store_instance_id) {
        return Err(StoreIdentityBindingError {
            code: StoreIdentityBindingErrorCode::InvalidIdentity,
            message: "provider store identity must be 1 to 128 safe ASCII bytes",
        }
        .into());
    }
    Ok(store_instance_id)
}

pub(super) fn verify_store_binding(identity: &StoreIdentity, expected: Option<&str>) -> Result<()> {
    if let Some(expected) = expected {
        require_store_instance_id(Some(expected))?;
        if identity.store_instance_id != expected {
            return Err(StoreIdentityBindingError {
                code: StoreIdentityBindingErrorCode::IdentityMismatch,
                message: "provider store identity does not match the bound store",
            }
            .into());
        }
    }
    Ok(())
}

fn inspect_connection_inner(conn: &Connection) -> Result<StoreCompatibility> {
    let version: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let object_count: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_schema
         WHERE name NOT LIKE 'sqlite_%' AND name NOT LIKE 'decisions_fts_%'",
        [],
        |row| row.get(0),
    )?;
    let has_metadata = object_exists(conn, "table", "open_why_metadata")?;
    let has_ledger = object_exists(conn, "table", "open_why_migrations")?;

    if version == 0 {
        if has_metadata || has_ledger {
            return Ok(incompatible(
                StoreCompatibilityErrorCode::PartialMigration,
                "store contains partial identity migration state",
                Some(version),
            ));
        }
        if object_count == 0 {
            return Ok(StoreCompatibility::Uninitialized);
        }
        if schema_sha256_on(conn)? == expected_legacy_schema_sha256_v0()? {
            return Ok(StoreCompatibility::MigrationRequired {
                from: 0,
                to: STORE_SCHEMA_VERSION,
                plan_digest: migration_plan_digest(),
            });
        }
        return Ok(incompatible(
            StoreCompatibilityErrorCode::ShapeDrift,
            "database is not a recognized open-why store",
            Some(version),
        ));
    }
    if version > STORE_SCHEMA_VERSION {
        return Ok(incompatible(
            StoreCompatibilityErrorCode::SchemaNewer,
            "store schema is newer than this open-why build",
            Some(version),
        ));
    }
    if version != STORE_SCHEMA_VERSION || !has_metadata || !has_ledger {
        return Ok(incompatible(
            StoreCompatibilityErrorCode::PartialMigration,
            "store identity migration is incomplete",
            Some(version),
        ));
    }

    let metadata_count: i64 =
        conn.query_row("SELECT count(*) FROM open_why_metadata", [], |row| {
            row.get(0)
        })?;
    if metadata_count != 1 {
        return Ok(incompatible(
            StoreCompatibilityErrorCode::SchemaCorrupt,
            "store identity metadata is not a singleton",
            Some(version),
        ));
    }
    let (family, metadata_version, stored_schema, stored_plan, store_instance_id): (
        String,
        u32,
        String,
        String,
        String,
    ) = conn.query_row(
        "SELECT schema_family,schema_version,schema_sha256,migration_plan_digest,
                store_instance_id
         FROM open_why_metadata WHERE singleton=1",
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    if metadata_version > STORE_SCHEMA_VERSION {
        return Ok(incompatible(
            StoreCompatibilityErrorCode::SchemaNewer,
            "store metadata is newer than this open-why build",
            Some(metadata_version),
        ));
    }
    if family != STORE_SCHEMA_FAMILY || metadata_version != version {
        return Ok(incompatible(
            StoreCompatibilityErrorCode::SchemaCorrupt,
            "store schema identity metadata is inconsistent",
            Some(version),
        ));
    }
    if !valid_store_instance_id(&store_instance_id) {
        return Ok(incompatible(
            StoreCompatibilityErrorCode::SchemaCorrupt,
            "store instance identity is invalid",
            Some(version),
        ));
    }
    if stored_plan != migration_plan_digest() {
        return Ok(incompatible(
            StoreCompatibilityErrorCode::ChecksumMismatch,
            "store migration plan digest does not match",
            Some(version),
        ));
    }
    let mut stmt = conn.prepare(
        "SELECT sequence,migration_id,checksum_sha256
         FROM open_why_migrations ORDER BY sequence",
    )?;
    let ledger = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, usize>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if ledger.len() != MIGRATION_STEPS.len() {
        return Ok(incompatible(
            StoreCompatibilityErrorCode::PartialMigration,
            "store migration ledger is incomplete",
            Some(version),
        ));
    }
    for (index, ((sequence, id, checksum), (expected_id, specification))) in
        ledger.iter().zip(MIGRATION_STEPS).enumerate()
    {
        if *sequence != index + 1
            || id != expected_id
            || checksum != &sha256_hex(specification.as_bytes())
        {
            return Ok(incompatible(
                StoreCompatibilityErrorCode::ChecksumMismatch,
                "store migration ledger checksum does not match",
                Some(version),
            ));
        }
    }
    let expected_schema = expected_schema_sha256_v1()?;
    if !required_shape_is_valid(conn)?
        || stored_schema != expected_schema
        || schema_sha256_on(conn)? != expected_schema
    {
        return Ok(incompatible(
            StoreCompatibilityErrorCode::ShapeDrift,
            "store schema shape does not match its declared identity",
            Some(version),
        ));
    }

    Ok(StoreCompatibility::Compatible {
        identity: StoreIdentity {
            store_instance_id,
            schema_family: STORE_SCHEMA_FAMILY,
            schema_version: STORE_SCHEMA_VERSION,
            schema_sha256: stored_schema,
        },
    })
}

fn incompatible(
    code: StoreCompatibilityErrorCode,
    message: &str,
    found_version: Option<u32>,
) -> StoreCompatibility {
    StoreCompatibility::Incompatible {
        code,
        message: message.to_owned(),
        found_version,
    }
}

pub(super) fn object_exists(conn: &Connection, kind: &str, name: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE type=?1 AND name=?2",
        params![kind, name],
        |row| row.get(0),
    )?;
    Ok(count == 1)
}

fn required_shape_is_valid(conn: &Connection) -> Result<bool> {
    for (kind, name) in REQUIRED_OBJECTS {
        if !object_exists(conn, kind, name)? {
            return Ok(false);
        }
    }
    let expected: HashSet<&str> = REQUIRED_DECISION_COLUMNS.iter().copied().collect();
    let mut stmt = conn.prepare("SELECT name FROM pragma_table_info('decisions')")?;
    let actual = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    if actual.len() != expected.len() || !expected.iter().all(|name| actual.contains(*name)) {
        return Ok(false);
    }
    for (table, expected_columns) in [
        (
            "decision_git_refs",
            &["commit_hash", "commit_subject", "created_at", "decision_id"][..],
        ),
        (
            "feedback_log",
            &["created_at", "delta", "helpful", "id", "memory_id"][..],
        ),
        (
            "open_why_metadata",
            &[
                "migration_plan_digest",
                "schema_family",
                "schema_sha256",
                "schema_version",
                "singleton",
                "store_instance_id",
            ][..],
        ),
        (
            "open_why_migrations",
            &["applied_at", "checksum_sha256", "migration_id", "sequence"][..],
        ),
    ] {
        let sql = format!("SELECT name FROM pragma_table_info('{table}') ORDER BY name");
        let mut stmt = conn.prepare(&sql)?;
        let columns = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if columns != expected_columns {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn valid_store_instance_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_STORE_INSTANCE_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

pub(super) fn valid_evidence_identity_shape(identity: &EvidenceIdentity) -> bool {
    identity.contract == EVIDENCE_IDENTITY_CONTRACT
        && identity.record_digest_contract == RECORD_DIGEST_CONTRACT
        && valid_store_instance_id(&identity.store_instance_id)
        && !identity.scope.is_empty()
        && identity.scope.len() <= MAX_COMMIT_LINK_SCOPE_BYTES
        && !identity.record_id.is_empty()
        && identity.record_id.len() <= MAX_COMMIT_LINK_RECORD_ID_BYTES
        && identity.record_digest.len() == 64
        && identity
            .record_digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
