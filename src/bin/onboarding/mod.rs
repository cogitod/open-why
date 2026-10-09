use anyhow::{bail, ensure, Context, Result};
use clap::ValueEnum;
use open_why::{inspect_store, Store, StoreCompatibility, MAX_STORE_INSTANCE_ID_BYTES};
use serde_json::json;
use std::io::Read;
use std::path::Path;

mod doctor;
pub use doctor::doctor;

#[derive(Clone, Copy, ValueEnum)]
pub enum Client {
    Codex,
    ClaudeCode,
    Generic,
}

pub fn setup(path: &Path, client: Client) -> Result<()> {
    ensure!(path.is_absolute(), "--db requires an absolute path");
    let database = path.to_str().context("database path must be valid UTF-8")?;
    let executable = std::env::current_exe().context("locate the running why executable")?;
    let executable = executable
        .to_str()
        .context("executable path must be valid UTF-8")?;
    let expected = configured_identity()?;
    let identity =
        match inspect_store(path).context("inspect store; run `why doctor --db <path>`")? {
            StoreCompatibility::Missing => {
                let identity = match expected {
                    Some(identity) => identity,
                    None => mint_identity()?,
                };
                // Setup binds only the store. Embedding configuration must not trigger downloads
                // or remote requests before the user connects a client.
                let store = Store::open_with_store_instance_id(path, &identity)?;
                store.store_identity()?.store_instance_id
            }
            StoreCompatibility::Compatible { identity } => {
                check_identity(expected.as_deref(), &identity.store_instance_id)?;
                identity.store_instance_id
            }
            state => bail!("{}", store_guidance(&state)),
        };
    let config = configuration(client, executable, database, &identity)?;
    eprintln!("Store ready: {database:?}");
    eprintln!("Store identity: {identity}");
    eprintln!("Keep this identity with this database; reuse it across clients.");
    match client {
        Client::Codex => eprintln!(
            "Merge stdout into ~/.codex/config.toml. Update an existing open-why entry instead of duplicating it."
        ),
        Client::ClaudeCode => eprintln!(
            "Merge stdout into your project's .mcp.json. Update an existing open-why entry instead of duplicating it; keep local paths out of version control."
        ),
        Client::Generic => eprintln!(
            "Use stdout as the command/args/env entry in your MCP client's configuration."
        ),
    }
    eprintln!("Reconnect the client and check its MCP server status, then ask about an absolute repository path.");
    println!("{config}");
    Ok(())
}

pub(super) fn configured_identity() -> Result<Option<String>> {
    let Some(value) = optional_env("OPEN_WHY_STORE_INSTANCE_ID")? else {
        return Ok(None);
    };
    ensure!(
        !value.is_empty()
            && value.len() <= MAX_STORE_INSTANCE_ID_BYTES
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b)),
        "invalid_identity: OPEN_WHY_STORE_INSTANCE_ID must contain 1 to {MAX_STORE_INSTANCE_ID_BYTES} ASCII letters, digits, '.', '_', ':', or '-'"
    );
    Ok(Some(value))
}

pub(super) fn optional_env(name: &str) -> Result<Option<String>> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(_) => bail!("{name} must be valid UTF-8"),
    }
}

pub(super) fn check_identity(expected: Option<&str>, actual: &str) -> Result<()> {
    ensure!(
        expected.is_none_or(|expected| expected == actual),
        "identity_mismatch: OPEN_WHY_STORE_INSTANCE_ID differs from this database; restore its original configuration or choose a new database path"
    );
    Ok(())
}

fn mint_identity() -> Result<String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .context("open OS randomness source")?
        .read_exact(&mut bytes)
        .context("read OS randomness for a new store identity")?;
    let mut id = String::from("open-why:");
    use std::fmt::Write;
    for byte in bytes {
        write!(id, "{byte:02x}")?;
    }
    Ok(id)
}

pub(super) fn store_guidance(state: &StoreCompatibility) -> String {
    match state {
        StoreCompatibility::Missing =>
            "store is missing; run `why setup --db <absolute-path> --client <client>`".into(),
        StoreCompatibility::Uninitialized =>
            "store is uninitialized; setup will not overwrite an existing file. Choose a new database path".into(),
        StoreCompatibility::MigrationRequired { from, to, .. } => format!(
            "migration_required: schema {from} needs migration to {to}; setup does not migrate. Follow the documented store migration procedure with its original identity, or choose a new path"
        ),
        StoreCompatibility::Incompatible { code, .. } => {
            if *code == open_why::StoreCompatibilityErrorCode::LiveWalIndeterminate {
                "live_wal_indeterminate: read-only inspection cannot verify this store. Close its writers and use a safely checkpointed snapshot or a new database path; do not delete WAL/SHM files".into()
            } else {
                format!("store is incompatible ({code:?}); use a compatible build or a verified backup. Setup will not repair or overwrite it")
            }
        }
        StoreCompatibility::Compatible { .. } => "store is compatible".into(),
    }
}

fn configuration(
    client: Client,
    executable: &str,
    database: &str,
    identity: &str,
) -> Result<String> {
    let entry = json!({
        "command": executable,
        "args": ["serve"],
        "env": {
            "OPEN_WHY_DB": database,
            "OPEN_WHY_STORE_INSTANCE_ID": identity
        }
    });
    match client {
        Client::Generic => Ok(serde_json::to_string_pretty(&entry)?),
        Client::ClaudeCode => {
            let mut entry = entry;
            entry["type"] = json!("stdio");
            Ok(serde_json::to_string_pretty(&json!({"mcpServers": {"open-why": entry}}))?)
        }
        Client::Codex => Ok(format!(
            "[mcp_servers.open-why]\ncommand = {}\nargs = [\"serve\"]\n\n[mcp_servers.open-why.env]\nOPEN_WHY_DB = {}\nOPEN_WHY_STORE_INSTANCE_ID = {}",
            toml_string(executable), toml_string(database), toml_string(identity)
        )),
    }
}

// TOML basic strings permit Unicode, but require escapes for DEL as well as ASCII controls.
fn toml_string(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0}'..='\u{1f}' | '\u{7f}' => {
                use std::fmt::Write;
                write!(out, "\\u{:04X}", ch as u32).expect("write to string");
            }
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}
