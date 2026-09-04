#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
use anyhow::Context;
use anyhow::Result;
use std::fs::File;
use std::path::{Path, PathBuf};

pub(crate) fn sqlite_open_flags(flags: rusqlite::OpenFlags) -> rusqlite::OpenFlags {
    #[cfg(target_vendor = "apple")]
    return flags | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW;

    // Linux and Android anchor SQLite below `/proc/self/fd/<parent>`. SQLite's
    // NOFOLLOW flag rejects that procfs symlink before opening the validated
    // leaf; openat(O_NOFOLLOW), descriptor/path identity checks, and
    // SQLITE_FCNTL_HAS_MOVED provide the equivalent leaf and handoff defenses.
    #[cfg(not(target_vendor = "apple"))]
    flags
}

pub(crate) struct PreparedStorePath {
    sqlite_path: PathBuf,
    #[cfg(unix)]
    opened_file: Option<File>,
    parent_guard: Option<File>,
}

impl PreparedStorePath {
    pub(crate) fn sqlite_path(&self) -> &Path {
        &self.sqlite_path
    }

    fn verify_path_target(&self) -> Result<()> {
        #[cfg(unix)]
        if let Some(opened_file) = &self.opened_file {
            use std::os::unix::fs::MetadataExt;

            let opened_by_descriptor = opened_file.metadata()?;
            let opened = std::fs::metadata(&self.sqlite_path)?;
            anyhow::ensure!(
                opened_by_descriptor.dev() == opened.dev()
                    && opened_by_descriptor.ino() == opened.ino(),
                "store path changed while it was being opened"
            );
        }
        Ok(())
    }

    pub(crate) fn open_connection<F>(&self, open: F) -> Result<rusqlite::Connection>
    where
        F: FnOnce(&Path) -> rusqlite::Result<rusqlite::Connection>,
    {
        self.open_connection_with_hooks(open, || {}, || {})
    }

    fn open_connection_with_hooks<F, B, A>(
        &self,
        open: F,
        before_sqlite_open: B,
        after_sqlite_open: A,
    ) -> Result<rusqlite::Connection>
    where
        F: FnOnce(&Path) -> rusqlite::Result<rusqlite::Connection>,
        B: FnOnce(),
        A: FnOnce(),
    {
        self.verify_path_target()?;
        before_sqlite_open();
        let connection = open(&self.sqlite_path)?;
        after_sqlite_open();
        self.verify_connection_target(&connection)?;
        Ok(connection)
    }

    fn verify_connection_target(&self, connection: &rusqlite::Connection) -> Result<()> {
        self.verify_path_target()?;
        verify_connection_has_not_moved(connection)?;
        self.verify_path_target()?;
        verify_connection_has_not_moved(connection)?;
        Ok(())
    }

    pub(crate) fn into_parent_guard(self) -> Option<File> {
        self.parent_guard
    }
}

#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
fn verify_connection_has_not_moved(connection: &rusqlite::Connection) -> Result<()> {
    let mut moved = 0_i32;
    // SAFETY: `connection.handle()` is valid for this call, `main` is a
    // NUL-terminated static database name, and SQLite writes one integer to
    // the supplied pointer for SQLITE_FCNTL_HAS_MOVED.
    let result = unsafe {
        rusqlite::ffi::sqlite3_file_control(
            connection.handle(),
            c"main".as_ptr(),
            rusqlite::ffi::SQLITE_FCNTL_HAS_MOVED,
            std::ptr::from_mut(&mut moved).cast(),
        )
    };
    anyhow::ensure!(
        result == rusqlite::ffi::SQLITE_OK && moved == 0,
        "store path changed while it was being opened"
    );
    Ok(())
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
fn verify_connection_has_not_moved(_connection: &rusqlite::Connection) -> Result<()> {
    Ok(())
}

#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
pub(crate) fn prepare(
    path: &Path,
    create: bool,
    writable: bool,
) -> Result<Option<PreparedStorePath>> {
    prepare_with_hook(path, create, writable, || {})
}

#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
fn prepare_with_hook<F>(
    path: &Path,
    create: bool,
    writable: bool,
    before_file_open: F,
) -> Result<Option<PreparedStorePath>>
where
    F: FnOnce(),
{
    use rustix::fs::{fchmod, mkdirat, open, openat, Mode, OFlags};
    use rustix::io::Errno;
    use std::path::Component;

    let file_name = path
        .file_name()
        .filter(|name| !name.is_empty())
        .context("store path must name a database file")?;
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let directory_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let start = if path.is_absolute() { "/" } else { "." };
    let mut parent_fd = open(start, directory_flags, Mode::empty())?;

    for component in parent.components() {
        let name = match component {
            Component::RootDir | Component::CurDir => continue,
            Component::ParentDir => std::ffi::OsStr::new(".."),
            Component::Normal(name) => name,
            Component::Prefix(_) => anyhow::bail!("unsupported store path prefix"),
        };
        parent_fd = match openat(&parent_fd, name, directory_flags, Mode::empty()) {
            Ok(fd) => fd,
            Err(Errno::NOENT) => {
                if !create {
                    return Ok(None);
                }
                match mkdirat(&parent_fd, name, Mode::RWXU) {
                    Ok(()) | Err(Errno::EXIST) => {}
                    Err(error) => return Err(error.into()),
                }
                openat(&parent_fd, name, directory_flags, Mode::empty())?
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("open store directory component {}", name.to_string_lossy())
                })
            }
        };
    }

    before_file_open();
    let open_existing = || {
        let access = if writable {
            OFlags::RDWR
        } else {
            OFlags::RDONLY
        };
        openat(
            &parent_fd,
            file_name,
            access | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
    };
    let file_fd = if create {
        let create_flags =
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        match openat(&parent_fd, file_name, create_flags, Mode::RUSR | Mode::WUSR) {
            Ok(fd) => {
                fchmod(&fd, Mode::RUSR | Mode::WUSR)?;
                fd
            }
            Err(Errno::EXIST) => open_existing().context("open existing store database")?,
            Err(error) => return Err(error).context("create new store database"),
        }
    } else {
        match open_existing() {
            Ok(fd) => fd,
            Err(Errno::NOENT) => return Ok(None),
            Err(error) => return Err(error).context("open existing store database"),
        }
    };
    let sqlite_parent = stable_directory_path(&parent_fd, parent)?;
    let sqlite_path = sqlite_parent.join(file_name);

    Ok(Some(PreparedStorePath {
        sqlite_path,
        opened_file: Some(File::from(file_fd)),
        parent_guard: Some(File::from(parent_fd)),
    }))
}

#[cfg(all(
    unix,
    not(any(target_vendor = "apple", target_os = "linux", target_os = "android"))
))]
pub(crate) fn prepare(
    _path: &Path,
    _create: bool,
    _writable: bool,
) -> Result<Option<PreparedStorePath>> {
    anyhow::bail!("secure new-store paths are unsupported on this Unix target")
}

#[cfg(target_vendor = "apple")]
fn stable_directory_path<Fd: std::os::fd::AsFd>(fd: Fd, _original: &Path) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStrExt;

    let path = rustix::fs::getpath(fd)?;
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(path.to_bytes())))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn stable_directory_path<Fd: std::os::fd::AsFd>(fd: Fd, _original: &Path) -> Result<PathBuf> {
    use std::os::fd::AsRawFd;

    Ok(PathBuf::from(format!(
        "/proc/self/fd/{}",
        fd.as_fd().as_raw_fd()
    )))
}

#[cfg(not(unix))]
pub(crate) fn prepare(
    path: &Path,
    create: bool,
    _writable: bool,
) -> Result<Option<PreparedStorePath>> {
    if create {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
    }
    if !create && !path.try_exists()? {
        return Ok(None);
    }
    Ok(Some(PreparedStorePath {
        sqlite_path: path.to_path_buf(),
        parent_guard: None,
    }))
}

#[cfg(all(
    test,
    unix,
    not(any(target_vendor = "apple", target_os = "linux", target_os = "android"))
))]
mod unsupported_unix_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn prepare_rejects_before_creating_any_path() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "open-why-unsupported-unix-{}-{nonce}",
            std::process::id()
        ));
        let result = prepare(&root.join("nested/store.db"), true, true);
        let Err(error) = result else {
            panic!("unsupported Unix prepare unexpectedly succeeded")
        };
        assert_eq!(
            error.to_string(),
            "secure new-store paths are unsupported on this Unix target"
        );
        assert!(!root.exists());
    }
}

#[cfg(all(test, unix, any(target_vendor = "apple", target_os = "linux")))]
mod tests {
    use super::*;
    use rusqlite::{Connection, OpenFlags};
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static FIXTURE_SERIAL: AtomicU64 = AtomicU64::new(0);

    #[cfg(target_os = "linux")]
    #[test]
    fn procfd_anchor_uses_descriptor_checks_instead_of_sqlite_nofollow() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "open-why-procfd-open-{}-{nonce}",
                std::process::id()
            ));
        let target = root.join("store.db");
        let prepared = prepare(&target, true, true).unwrap().unwrap();
        assert!(
            std::fs::symlink_metadata(prepared.sqlite_path().parent().unwrap())
                .unwrap()
                .file_type()
                .is_symlink()
        );

        let rejected = Connection::open_with_flags(
            prepared.sqlite_path(),
            OpenFlags::default() | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .unwrap_err();
        assert_eq!(
            rejected.sqlite_error().unwrap().extended_code,
            rusqlite::ffi::SQLITE_CANTOPEN_SYMLINK
        );

        let connection = prepared
            .open_connection(|path| {
                Connection::open_with_flags(path, sqlite_open_flags(OpenFlags::default()))
            })
            .unwrap();
        connection
            .execute_batch("CREATE TABLE procfd_probe (value INTEGER NOT NULL);")
            .unwrap();
        drop(connection);

        assert!(target.is_file());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn replacement_race_stays_on_validated_directory() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "open-why-store-path-race-{}-{nonce}",
                std::process::id()
            ));
        let outside = root.join("outside");
        let intended = root.join("intended");
        let parked = root.join("parked");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o751)).unwrap();

        let target = intended.join("store.db");
        let prepared = prepare_with_hook(&target, true, true, || {
            std::fs::rename(&intended, &parked).unwrap();
            symlink(&outside, &intended).unwrap();
        });

        let prepared = prepared.unwrap().unwrap();
        let connection = prepared
            .open_connection(|path| {
                Connection::open_with_flags(path, sqlite_open_flags(OpenFlags::default()))
            })
            .unwrap();
        connection
            .execute_batch("CREATE TABLE race_probe (value INTEGER NOT NULL);")
            .unwrap();
        drop(connection);

        assert!(parked.join("store.db").is_file());
        assert!(!outside.join("store.db").exists());
        assert_eq!(
            std::fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
            0o751
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn existing_store_replacement_race_stays_on_validated_directory() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "open-why-existing-path-race-{}-{nonce}",
                std::process::id()
            ));
        let outside = root.join("outside");
        let intended = root.join("intended");
        let parked = root.join("parked");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir(&intended).unwrap();
        let target = intended.join("store.db");
        std::fs::File::create(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).unwrap();
        let outside_target = outside.join("store.db");
        std::fs::write(&outside_target, b"outside sentinel").unwrap();
        std::fs::set_permissions(&outside_target, std::fs::Permissions::from_mode(0o640)).unwrap();

        let prepared = prepare_with_hook(&target, false, true, || {
            std::fs::rename(&intended, &parked).unwrap();
            symlink(&outside, &intended).unwrap();
        })
        .unwrap()
        .unwrap();
        let connection = prepared
            .open_connection(|path| {
                Connection::open_with_flags(
                    path,
                    sqlite_open_flags(
                        OpenFlags::SQLITE_OPEN_READ_WRITE
                            | OpenFlags::SQLITE_OPEN_NO_MUTEX
                            | OpenFlags::SQLITE_OPEN_URI,
                    ),
                )
            })
            .unwrap();
        connection
            .execute_batch("CREATE TABLE existing_race_probe (value INTEGER NOT NULL);")
            .unwrap();
        drop(connection);

        assert!(parked.join("store.db").is_file());
        assert_eq!(std::fs::read(&outside_target).unwrap(), b"outside sentinel");
        assert_eq!(
            std::fs::metadata(&outside_target)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
        assert_eq!(
            std::fs::metadata(parked.join("store.db"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    fn run_sqlite_handoff_race(create: bool, restore_before_validation: bool) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let serial = FIXTURE_SERIAL.fetch_add(1, Ordering::Relaxed);
        let root = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "open-why-sqlite-handoff-{}-{nonce}-{serial}",
                std::process::id(),
            ));
        let directory = root.join("store");
        std::fs::create_dir_all(&directory).unwrap();
        let intended_target = directory.join("store.db");
        let parked_target = directory.join("parked.db");
        let outside_target = directory.join("outside.db");
        let outside_setup = Connection::open(&outside_target).unwrap();
        outside_setup
            .execute_batch(
                "CREATE TABLE outside_records (value TEXT NOT NULL);
                 INSERT INTO outside_records VALUES ('outside');",
            )
            .unwrap();
        drop(outside_setup);
        std::fs::set_permissions(&outside_target, std::fs::Permissions::from_mode(0o640)).unwrap();
        let outside_observer = Connection::open(&outside_target).unwrap();
        let outside_version: i64 = outside_observer
            .pragma_query_value(None, "data_version", |row| row.get(0))
            .unwrap();
        let outside_bytes = std::fs::read(&outside_target).unwrap();

        let intended_bytes = if create {
            None
        } else {
            let intended_setup = Connection::open(&intended_target).unwrap();
            intended_setup
                .execute_batch(
                    "CREATE TABLE intended_records (value TEXT NOT NULL);
                     INSERT INTO intended_records VALUES ('intended');",
                )
                .unwrap();
            drop(intended_setup);
            std::fs::set_permissions(&intended_target, std::fs::Permissions::from_mode(0o640))
                .unwrap();
            Some(std::fs::read(&intended_target).unwrap())
        };
        let prepared = prepare(&intended_target, create, true).unwrap().unwrap();
        let result = prepared.open_connection_with_hooks(
            |path| {
                Connection::open_with_flags(
                    path,
                    sqlite_open_flags(
                        OpenFlags::SQLITE_OPEN_READ_WRITE
                            | OpenFlags::SQLITE_OPEN_NO_MUTEX
                            | OpenFlags::SQLITE_OPEN_URI,
                    ),
                )
            },
            || {
                std::fs::rename(&intended_target, &parked_target).unwrap();
                std::fs::rename(&outside_target, &intended_target).unwrap();
            },
            || {
                if restore_before_validation {
                    std::fs::rename(&intended_target, &outside_target).unwrap();
                    std::fs::rename(&parked_target, &intended_target).unwrap();
                }
            },
        );
        assert!(result.is_err());

        let final_outside = if restore_before_validation {
            &outside_target
        } else {
            &intended_target
        };
        assert_eq!(std::fs::read(final_outside).unwrap(), outside_bytes);
        assert_eq!(
            outside_observer
                .pragma_query_value(None, "data_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            outside_version
        );
        assert_eq!(
            std::fs::metadata(final_outside)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
        assert_eq!(
            outside_observer
                .query_row("SELECT count(*) FROM outside_records", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );

        let final_intended = if restore_before_validation {
            &intended_target
        } else {
            &parked_target
        };
        assert!(final_intended.is_file());
        assert_eq!(
            std::fs::metadata(final_intended)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            if create { 0o600 } else { 0o640 }
        );
        if let Some(intended_bytes) = intended_bytes {
            assert_eq!(std::fs::read(final_intended).unwrap(), intended_bytes);
            assert_eq!(
                Connection::open(final_intended)
                    .unwrap()
                    .query_row("SELECT count(*) FROM intended_records", [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                1
            );
        } else {
            assert_eq!(std::fs::metadata(final_intended).unwrap().len(), 0);
        }
        drop(outside_observer);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn new_store_one_swap_at_sqlite_handoff_is_safe() {
        run_sqlite_handoff_race(true, false);
    }

    #[test]
    fn new_store_swap_back_at_sqlite_handoff_is_safe() {
        run_sqlite_handoff_race(true, true);
    }

    #[test]
    fn existing_store_one_swap_at_sqlite_handoff_is_safe() {
        run_sqlite_handoff_race(false, false);
    }

    #[test]
    fn existing_store_swap_back_at_sqlite_handoff_is_safe() {
        run_sqlite_handoff_race(false, true);
    }
}
