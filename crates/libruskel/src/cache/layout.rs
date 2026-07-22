//! Filesystem layout and no-follow operations for the Ruskel cache.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, ErrorKind, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use fs4::{FileExt, TryLockError};

use crate::error::{Result, RuskelError};

/// Immutable ownership marker file name.
pub(super) const MARKER_NAME: &str = "ruskel.cache";
/// Timestamp file for the last completed maintenance pass.
pub(super) const MAINTENANCE_STAMP: &str = "maintenance.stamp";
/// Timestamp file for toolchain and workspace use.
pub(super) const LAST_USE: &str = "ruskel.last-use";
/// Version-1 ownership marker contents.
const MARKER_CONTENT: &str = "ruskel-cache\nversion=1\n";
/// Standard cache-directory tag contents.
const CACHE_TAG_CONTENT: &str = "Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag created by Ruskel.\n";
/// Process-local suffix source for atomic metadata and trash names.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Validated paths that form one cache layout.
#[derive(Clone, Debug)]
pub(super) struct CacheLayout {
    /// Absolute cache root.
    root: PathBuf,
}

impl CacheLayout {
    /// Initialize or validate a cache root.
    pub(super) fn initialize(root: PathBuf) -> Result<Self> {
        ensure_directory(&root, "create cache root")?;
        let marker_path = root.join(MARKER_NAME);

        if !marker_path.exists() {
            validate_unmarked_root(&root)?;
        }

        let mut marker = match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&marker_path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                validate_regular_file(&marker_path, "ownership marker")?;
                open_rw(&marker_path, "open ownership marker")?
            }
            Err(source) => return Err(cache_io("create ownership marker", &marker_path, source)),
        };

        FileExt::lock(&marker)
            .map_err(|source| cache_io("lock ownership marker", &marker_path, source))?;
        validate_marker(&mut marker, &marker_path)?;

        for path in [
            root.join("locks"),
            root.join("locks/toolchain"),
            root.join("locks/workspace"),
            root.join("trash"),
            root.join("build"),
        ] {
            ensure_directory(&path, "create cache directory")?;
        }

        let tag_path = root.join("CACHEDIR.TAG");
        if tag_path.exists() {
            validate_regular_file(&tag_path, "cache directory tag")?;
        } else {
            write_new_file(
                &tag_path,
                CACHE_TAG_CONTENT.as_bytes(),
                "create cache directory tag",
            )?;
        }

        drop(marker);
        Ok(Self { root })
    }

    /// Return the canonical cache root.
    pub(super) fn canonical_root(&self) -> Result<PathBuf> {
        fs::canonicalize(&self.root)
            .map_err(|source| cache_io("canonicalize cache root", &self.root, source))
    }

    /// Return the cache root.
    pub(super) fn root(&self) -> &Path {
        &self.root
    }

    /// Return the build directory.
    pub(super) fn build_dir(&self) -> PathBuf {
        self.root.join("build")
    }

    /// Return the trash directory.
    pub(super) fn trash_dir(&self) -> PathBuf {
        self.root.join("trash")
    }

    /// Return the toolchain lock path for an identity.
    pub(super) fn toolchain_lock(&self, identity: &str) -> PathBuf {
        self.root
            .join("locks/toolchain")
            .join(format!("{identity}.lock"))
    }

    /// Return the workspace lock path for an identity.
    pub(super) fn workspace_lock(&self, identity: &str) -> PathBuf {
        self.root
            .join("locks/workspace")
            .join(format!("{identity}.lock"))
    }

    /// Return the cross-process garbage-collection lock path.
    pub(super) fn gc_lock(&self) -> PathBuf {
        self.root.join("locks/gc.lock")
    }

    /// Return the completed-maintenance timestamp path.
    pub(super) fn maintenance_stamp(&self) -> PathBuf {
        self.root.join(MAINTENANCE_STAMP)
    }

    /// Open and acquire a shared root lease.
    pub(super) fn lock_root_shared(&self) -> Result<File> {
        let path = self.root.join(MARKER_NAME);
        validate_regular_file(&path, "ownership marker")?;
        let file = open_rw(&path, "open ownership marker")?;
        FileExt::lock_shared(&file).map_err(|source| cache_io("lock cache root", &path, source))?;
        Ok(file)
    }

    /// Try to acquire an exclusive root lease.
    pub(super) fn try_lock_root(&self) -> Result<Option<File>> {
        let path = self.root.join(MARKER_NAME);
        validate_regular_file(&path, "ownership marker")?;
        let file = open_rw(&path, "open ownership marker")?;
        match FileExt::try_lock(&file) {
            Ok(()) => Ok(Some(file)),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(source)) => Err(cache_io("lock cache root", &path, source)),
        }
    }

    /// Open a stable lock file and acquire a shared lease.
    pub(super) fn lock_shared(&self, path: &Path) -> Result<File> {
        let file = open_or_create_lock(path)?;
        FileExt::lock_shared(&file)
            .map_err(|source| cache_io("acquire shared cache lease", path, source))?;
        Ok(file)
    }

    /// Open a stable lock file and acquire an exclusive lease.
    pub(super) fn lock_exclusive(&self, path: &Path) -> Result<File> {
        let file = open_or_create_lock(path)?;
        FileExt::lock(&file)
            .map_err(|source| cache_io("acquire exclusive cache lease", path, source))?;
        Ok(file)
    }

    /// Try to acquire an exclusive lease on a stable lock file.
    pub(super) fn try_lock_exclusive(&self, path: &Path) -> Result<Option<File>> {
        let file = open_or_create_lock(path)?;
        match FileExt::try_lock(&file) {
            Ok(()) => Ok(Some(file)),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(source)) => {
                Err(cache_io("acquire exclusive cache lease", path, source))
            }
        }
    }

    /// Return whether a stable lock file is currently held.
    pub(super) fn is_locked(&self, path: &Path) -> Result<bool> {
        let file = open_or_create_lock(path)?;
        match FileExt::try_lock(&file) {
            Ok(()) => Ok(false),
            Err(TryLockError::WouldBlock) => Ok(true),
            Err(TryLockError::Error(source)) => Err(cache_io("inspect cache lease", path, source)),
        }
    }
}

/// Write a metadata timestamp with an atomic same-directory rename.
pub(super) fn write_timestamp(path: &Path, unix_seconds: u64) -> Result<()> {
    let parent = path.parent().ok_or_else(|| RuskelError::CacheLayout {
        path: path.to_path_buf(),
        message: "metadata path has no parent".to_string(),
    })?;
    ensure_directory(parent, "create metadata directory")?;
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(".ruskel.tmp.{}.{}", process::id(), counter));
    write_new_file(
        &temp,
        format!("{unix_seconds}\n").as_bytes(),
        "write metadata",
    )?;
    fs::rename(&temp, path).map_err(|source| {
        drop(fs::remove_file(&temp));
        cache_io("replace metadata", path, source)
    })
}

/// Read a metadata timestamp as seconds since the Unix epoch.
pub(super) fn read_timestamp(path: &Path) -> Result<Option<u64>> {
    match fs::read_to_string(path) {
        Ok(value) => value
            .trim()
            .parse::<u64>()
            .map(Some)
            .map_err(|_| RuskelError::CacheLayout {
                path: path.to_path_buf(),
                message: "metadata timestamp is invalid".to_string(),
            }),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(source) => Err(cache_io("read metadata", path, source)),
    }
}

/// Return the recognized size of a path without following symbolic links.
pub(super) fn path_size(path: &Path) -> Result<u64> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| cache_io("inspect cache entry", path, source))?;
    if metadata.file_type().is_symlink() {
        return Err(RuskelError::CacheLayout {
            path: path.to_path_buf(),
            message: "symbolic links are not cache-owned entries".to_string(),
        });
    }
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    if !metadata.is_dir() {
        return Ok(0);
    }

    let mut total = 0_u64;
    for entry in
        fs::read_dir(path).map_err(|source| cache_io("read cache directory", path, source))?
    {
        let entry = entry.map_err(|source| cache_io("read cache directory", path, source))?;
        total = total.saturating_add(path_size(&entry.path())?);
    }
    Ok(total)
}

/// Rename a recognized cache entry into trash.
pub(super) fn move_to_trash(root: &Path, path: &Path, label: &str) -> Result<PathBuf> {
    let trash = root.join("trash");
    ensure_directory(&trash, "create trash directory")?;
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let destination = trash.join(format!("{label}.{}.{}", process::id(), counter));
    fs::rename(path, &destination)
        .map_err(|source| cache_io("move cache entry to trash", path, source))?;
    Ok(destination)
}

/// Remove a tree without following symbolic links.
pub(super) fn remove_no_follow(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(cache_io("inspect cache entry for removal", path, source)),
    };

    if metadata.file_type().is_symlink() || metadata.is_file() {
        return fs::remove_file(path).map_err(|source| cache_io("remove cache file", path, source));
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)
            .map_err(|source| cache_io("read cache directory for removal", path, source))?
        {
            let entry = entry
                .map_err(|source| cache_io("read cache directory for removal", path, source))?;
            remove_no_follow(&entry.path())?;
        }
        return fs::remove_dir(path)
            .map_err(|source| cache_io("remove cache directory", path, source));
    }
    Ok(())
}

/// Return whether a name is a full lowercase SHA-256 digest.
pub(super) fn is_identity(name: &str) -> bool {
    name.len() == 64
        && name
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

/// Return whether a trash name was generated by this layout.
pub(super) fn is_trash_name(name: &str) -> bool {
    let mut parts = name.split('.');
    let Some(first) = parts.next() else {
        return false;
    };
    let Some(second) = parts.next() else {
        return false;
    };
    let Some(pid) = parts.next() else {
        return false;
    };
    let Some(counter) = parts.next() else {
        return false;
    };
    parts.next().is_none()
        && is_identity(first)
        && is_identity(second)
        && pid.parse::<u32>().is_ok()
        && counter.parse::<u64>().is_ok()
}

/// Reject foreign entries before creating an ownership marker.
fn validate_unmarked_root(root: &Path) -> Result<()> {
    for entry in fs::read_dir(root).map_err(|source| cache_io("read cache root", root, source))? {
        let entry = entry.map_err(|source| cache_io("read cache root", root, source))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !matches!(
            name.as_ref(),
            "CACHEDIR.TAG" | MAINTENANCE_STAMP | "locks" | "trash" | "build"
        ) {
            return Err(RuskelError::CacheLayout {
                path: root.to_path_buf(),
                message: "nonempty directory has no Ruskel ownership marker".to_string(),
            });
        }
    }
    Ok(())
}

/// Validate a complete marker or finish an empty interrupted marker.
fn validate_marker(marker: &mut File, path: &Path) -> Result<()> {
    marker
        .seek(SeekFrom::Start(0))
        .map_err(|source| cache_io("read ownership marker", path, source))?;
    let mut content = String::new();
    marker
        .read_to_string(&mut content)
        .map_err(|source| cache_io("read ownership marker", path, source))?;

    if content.is_empty() {
        marker
            .seek(SeekFrom::Start(0))
            .and_then(|_| marker.write_all(MARKER_CONTENT.as_bytes()))
            .and_then(|_| marker.sync_all())
            .map_err(|source| cache_io("initialize ownership marker", path, source))?;
        return Ok(());
    }
    if content != MARKER_CONTENT {
        return Err(RuskelError::CacheLayout {
            path: path.to_path_buf(),
            message: "unsupported or invalid cache layout version".to_string(),
        });
    }
    Ok(())
}

/// Create a directory or validate its existing no-follow type.
fn ensure_directory(path: &Path, action: &'static str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(RuskelError::CacheLayout {
            path: path.to_path_buf(),
            message: "expected an owned directory, not a file or symbolic link".to_string(),
        }),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(|source| cache_io(action, path, source))
        }
        Err(source) => Err(cache_io(action, path, source)),
    }
}

/// Require a regular file and reject symbolic links.
fn validate_regular_file(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| cache_io("inspect cache metadata", path, source))?;
    if metadata.is_file() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(RuskelError::CacheLayout {
            path: path.to_path_buf(),
            message: format!("{label} must be a regular file"),
        })
    }
}

/// Open one existing cache metadata file for reading and writing.
fn open_rw(path: &Path, action: &'static str) -> Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|source| cache_io(action, path, source))
}

/// Open or create one stable cache lock file.
fn open_or_create_lock(path: &Path) -> Result<File> {
    if path.exists() {
        validate_regular_file(path, "cache lock")?;
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|source| cache_io("open cache lock", path, source))
}

/// Create, write, and flush one new metadata file.
fn write_new_file(path: &Path, content: &[u8], action: &'static str) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| cache_io(action, path, source))?;
    file.write_all(content)
        .and_then(|_| file.sync_all())
        .map_err(|source| cache_io(action, path, source))
}

/// Attach an operation and path to one cache I/O error.
fn cache_io(action: &'static str, path: &Path, source: io::Error) -> RuskelError {
    RuskelError::CacheIo {
        action,
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn initializes_and_reopens_owned_layout() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("cache");

        let first = CacheLayout::initialize(root.clone())?;
        let marker = fs::read_to_string(root.join(MARKER_NAME))?;
        assert_eq!(marker, MARKER_CONTENT);
        assert_eq!(first.root(), root);

        CacheLayout::initialize(root)?;
        Ok(())
    }

    #[test]
    fn completes_empty_interrupted_marker() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("cache");
        fs::create_dir(&root)?;
        fs::File::create(root.join(MARKER_NAME))?;

        CacheLayout::initialize(root.clone())?;

        assert_eq!(fs::read_to_string(root.join(MARKER_NAME))?, MARKER_CONTENT);
        assert!(root.join("locks/toolchain").is_dir());
        Ok(())
    }

    #[test]
    fn refuses_nonempty_unmarked_root() -> Result<()> {
        let temp = tempdir()?;
        fs::write(temp.path().join("foreign"), b"data")?;

        let error = CacheLayout::initialize(temp.path().to_path_buf())
            .expect_err("foreign root must be rejected");
        assert!(error.to_string().contains("no Ruskel ownership marker"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn refuses_marker_symlink() -> Result<()> {
        let temp = tempdir()?;
        let target = temp.path().join("target");
        fs::write(&target, MARKER_CONTENT)?;
        symlink(&target, temp.path().join(MARKER_NAME))?;

        let error = CacheLayout::initialize(temp.path().to_path_buf())
            .expect_err("marker symlink must be rejected");
        assert!(error.to_string().contains("regular file"));
        Ok(())
    }

    #[test]
    fn no_follow_removal_unlinks_symlink_target_safely() -> Result<()> {
        let temp = tempdir()?;
        let tree = temp.path().join("tree");
        fs::create_dir(&tree)?;
        let outside = temp.path().join("outside");
        fs::write(&outside, b"keep")?;

        #[cfg(unix)]
        symlink(&outside, tree.join("link"))?;
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&outside, tree.join("link"))?;

        remove_no_follow(&tree)?;
        assert_eq!(fs::read(&outside)?, b"keep");
        Ok(())
    }
}
