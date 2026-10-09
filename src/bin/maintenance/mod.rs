use anyhow::{ensure, Result};
use open_why::{db, Store};
use std::path::{Component, Path};

fn path_check(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute() && !path.components().any(|c| matches!(c, Component::ParentDir)),
        "maintenance requires absolute paths without parent traversal"
    );
    Ok(())
}
fn source(path: &Path) -> Result<Store> {
    path_check(path)?;
    let expected = match std::env::var("OPEN_WHY_STORE_INSTANCE_ID") {
        Ok(id) => Some(id),
        Err(std::env::VarError::NotPresent) => None,
        Err(e) => return Err(e.into()),
    };
    let store = Store::open_existing_read_only(path, expected.as_deref())?;
    store.verify_integrity()?;
    Ok(store)
}
fn copy(path: &Path, to: &Path) -> Result<()> {
    path_check(to)?;
    // A stale daemon endpoint may describe an earlier file at the destination.
    let mut endpoint = to.as_os_str().to_os_string();
    endpoint.push(".sock");
    ensure!(
        std::fs::symlink_metadata(Path::new(&endpoint))
            .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
        "destination has a daemon endpoint or cannot be inspected; choose a new restore path"
    );
    let store = source(path)?;
    store.backup_to(to)?;
    let restored =
        Store::open_existing_read_only(to, Some(&store.store_identity()?.store_instance_id))?;
    let identity = restored.verify_integrity()?;
    println!(
        "{}",
        serde_json::to_string_pretty(
            &serde_json::json!({"status":"ok","path":to,"identity":identity})
        )?
    );
    Ok(())
}
pub fn backup(to: &Path) -> Result<()> {
    copy(&db::default_path(), to)
}
pub fn verify(path: &Path) -> Result<()> {
    let identity = source(path)?.store_identity()?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({"status":"ok","identity":identity}))?
    );
    Ok(())
}
pub fn restore(path: &Path, to: &Path) -> Result<()> {
    copy(path, to)
}
