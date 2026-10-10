use std::io;
use std::path::{Path, PathBuf};

fn base(env: &str, fallback: Option<PathBuf>) -> PathBuf {
    std::env::var_os(env)
        .map(PathBuf::from)
        .or_else(|| fallback.map(|p| p.join("qrsend")))
        .unwrap_or_else(shared_fallback)
}

/// Where everything goes on a system that names no place for it (no home
/// directory): the temporary directory, which other users share, so one of
/// this user's own.
fn shared_fallback() -> PathBuf {
    #[cfg(unix)]
    let name = format!("qrsend-{}", unsafe { libc::geteuid() });
    #[cfg(not(unix))]
    let name = "qrsend".to_string();
    std::env::temp_dir().join(name)
}

/// Persistent receiver state (inbox).
pub fn data_dir() -> PathBuf {
    base("QRSEND_DATA_DIR", dirs::data_local_dir())
}

/// Sender spools (re-creatable, so they live in the cache).
pub fn cache_dir() -> PathBuf {
    base("QRSEND_CACHE_DIR", dirs::cache_dir())
}

pub fn sessions_dir() -> PathBuf {
    data_dir().join("sessions")
}

pub fn spools_dir() -> PathBuf {
    cache_dir().join("send")
}

/// Device identity and trusted devices.
pub fn config_dir() -> PathBuf {
    base("QRSEND_CONFIG_DIR", dirs::config_dir())
}

/// Creates `dir` (and what is missing above it) for this user's eyes only:
/// these directories hold copies of what was sent and received, and this
/// device's private key. A directory that is already there must be this
/// user's and not writable by everyone. None above it may be writable by
/// everyone either, except the way /tmp is (sticky: each removes only what
/// is theirs), and what sits in such a place must again be this user's:
/// someone else could have made it first, to read along or to plant links.
pub fn create_private(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        let me = unsafe { libc::geteuid() };
        let refuse = |at: &Path| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "{} belongs to another user, or anyone may change it: not used for private data",
                    at.display()
                ),
            )
        };
        let leaf = std::fs::symlink_metadata(dir)?;
        if !leaf.is_dir() || leaf.uid() != me || leaf.mode() & 0o002 != 0 {
            return Err(refuse(dir));
        }
        let full = std::fs::canonicalize(dir)?;
        for (inside, above) in full.ancestors().zip(full.ancestors().skip(1)) {
            let meta = std::fs::metadata(above)?;
            if meta.mode() & 0o002 == 0 {
                continue;
            }
            if meta.mode() & 0o1000 == 0 {
                return Err(refuse(above));
            }
            let owner = std::fs::metadata(inside)?.uid();
            if owner != me && owner != 0 {
                return Err(refuse(inside));
            }
        }
        Ok(())
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)
}

/// Writes a file only this user can read (its content is private). Replaces
/// what is at `path` (by writing next to it and renaming).
pub fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut f = options.open(&tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(tmp, path)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    #[test]
    fn private_directories_and_files() {
        let root = std::env::temp_dir().join(format!("qrsend-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("a/b");
        create_private(&dir).unwrap();
        assert_eq!(std::fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
        let file = dir.join("secret");
        write_private(&file, b"key").unwrap();
        write_private(&file, b"new key").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"new key");
        assert_eq!(std::fs::metadata(&file).unwrap().mode() & 0o777, 0o600);
        // A directory anyone may write to is refused, and so is one below it,
        // and a link.
        let open = root.join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(create_private(&open).is_err());
        assert!(create_private(&open.join("inside")).is_err());
        std::os::unix::fs::symlink(&dir, root.join("link")).unwrap();
        assert!(create_private(&root.join("link")).is_err());
        // Shared the safe way (sticky, like /tmp): fine for what is this user's.
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o1777)).unwrap();
        create_private(&open.join("mine/deeper")).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }
}
