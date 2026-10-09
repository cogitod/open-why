//! A daemon connection is bound to a canonical pathname, physical file, and store identity.
//! Endpoint discovery alone never authorizes access to a store.
use crate::db::{self, Store};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::os::unix::{
    fs::{FileTypeExt, MetadataExt, PermissionsExt},
    net::UnixStream,
};
use std::path::{Component, PathBuf};
use std::time::Duration;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct Binding {
    version: u32,
    path: PathBuf,
    identity: String,
    device: u64,
    inode: u64,
}
impl Binding {
    pub(super) fn configured() -> Result<(Self, Store)> {
        let path = db::default_path();
        ensure!(
            !path.components().any(|c| matches!(c, Component::ParentDir)),
            "MCP database paths must not contain parent traversal"
        );
        // Opening through Store validates symlink-free path traversal and identity before
        // endpoint discovery. No embedding model is loaded just to authenticate a proxy.
        let store = match std::env::var("OPEN_WHY_STORE_INSTANCE_ID") {
            Ok(id) => Store::open_with_store_instance_id(&path, &id)?,
            Err(std::env::VarError::NotPresent) => Store::open(&path)?,
            Err(e) => return Err(e.into()),
        };
        let path = path.canonicalize()?;
        let meta = std::fs::metadata(&path)?;
        ensure!(
            meta.nlink() == 1,
            "hard-linked database paths are ambiguous"
        );
        store.verify_file()?;
        let binding = Self {
            version: 1,
            path,
            identity: store.store_identity()?.store_instance_id,
            device: meta.dev(),
            inode: meta.ino(),
        };
        binding.verify(&store)?;
        Ok((binding, store))
    }
    pub(super) fn socket(&self) -> PathBuf {
        let mut name = self.path.as_os_str().to_os_string();
        name.push(".sock");
        name.into()
    }
    pub(super) fn verify(&self, store: &Store) -> Result<()> {
        store.verify_file()?;
        let meta = std::fs::symlink_metadata(&self.path)?;
        ensure!(
            meta.is_file()
                && meta.nlink() == 1
                && meta.dev() == self.device
                && meta.ino() == self.inode,
            "store path changed; stop the daemon and reconnect"
        );
        Ok(())
    }
    pub(super) fn connect(&self) -> Result<Option<UnixStream>> {
        let path = self.socket();
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        ensure!(
            meta.file_type().is_socket()
                && meta.permissions().mode() & 0o077 == 0
                && meta.uid() == std::fs::metadata(&self.path)?.uid(),
            "unsafe daemon endpoint"
        );
        let mut stream = UnixStream::connect(path).context("daemon endpoint unavailable; stop its supervisor and remove the stale socket before retrying")?;
        timeout(&stream, Some(Duration::from_secs(5)))?;
        let mut bytes = serde_json::to_vec(self)?;
        bytes.push(b'\n');
        ensure!(bytes.len() <= 4096, "store binding exceeds transport limit");
        stream.write_all(&bytes)?;
        ensure!(
            read_line(&mut stream)? == b"OK\n",
            "daemon rejected store binding"
        );
        timeout(&stream, None)?;
        Ok(Some(stream))
    }
    pub(super) fn accept(&self, stream: &mut UnixStream) -> Result<()> {
        timeout(stream, Some(Duration::from_secs(5)))?;
        let requested: Binding = serde_json::from_slice(&read_line(stream)?)?;
        ensure!(requested == *self, "daemon store binding mismatch");
        stream.write_all(b"OK\n")?;
        timeout(stream, None)
    }
}
fn timeout(stream: &UnixStream, timeout: Option<Duration>) -> Result<()> {
    stream.set_read_timeout(timeout)?;
    stream.set_write_timeout(timeout)?;
    Ok(())
}
fn read_line(stream: &mut UnixStream) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    for _ in 0..4096 {
        let mut byte = [0];
        stream.read_exact(&mut byte)?;
        bytes.push(byte[0]);
        if byte[0] == b'\n' {
            return Ok(bytes);
        }
    }
    anyhow::bail!("daemon binding exceeds transport limit")
}
