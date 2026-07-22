//! Dedicated cache ownership, identity, reporting, and build leases.

mod layout;

use std::{
    collections::HashMap,
    env, fs,
    fs::File,
    path::{Path, PathBuf},
    result,
    sync::{Arc, Mutex, OnceLock, Weak},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use once_cell::sync::Lazy;
use sha2::{Digest, Sha256};

use self::layout::{CacheLayout, LAST_USE, is_identity, is_trash_name};
use crate::error::{Result, RuskelError};

/// Default maximum recognized cache usage before maintenance evicts entries.
const HIGH_WATER_BYTES: u64 = 20_000_000_000;

/// Weak registry that shares cache owners by canonical root.
static OWNERS: Lazy<Mutex<HashMap<PathBuf, Weak<CacheOwner>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// A cache entry or operation that Ruskel skipped.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CacheIssue {
    /// Affected cache path.
    path: PathBuf,
    /// Actionable issue description.
    message: String,
}

impl CacheIssue {
    /// Return the affected path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Return an actionable description of the issue.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Create one issue for a cache path.
    fn new(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

/// Status for one workspace entry in the cache.
#[derive(Clone, Debug)]
pub struct WorkspaceCacheStatus {
    /// Full workspace identity digest.
    identity: String,
    /// Recognized entry size.
    size_bytes: u64,
    /// Valid last-use time.
    last_use: Option<SystemTime>,
    /// Observed workspace-lock state.
    locked: bool,
}

impl WorkspaceCacheStatus {
    /// Return the workspace identity digest.
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// Return the recognized entry size in bytes.
    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    /// Return the last-use time when valid metadata exists.
    pub fn last_use(&self) -> Option<SystemTime> {
        self.last_use
    }

    /// Return whether another operation currently holds the workspace lease.
    pub fn is_locked(&self) -> bool {
        self.locked
    }
}

/// Status for one nightly toolchain entry in the cache.
#[derive(Clone, Debug)]
pub struct ToolchainCacheStatus {
    /// Full toolchain identity digest.
    identity: String,
    /// Recognized toolchain-tree size.
    size_bytes: u64,
    /// Valid last-use time.
    last_use: Option<SystemTime>,
    /// Observed toolchain-lock state.
    locked: bool,
    /// Recognized workspace entries.
    workspaces: Vec<WorkspaceCacheStatus>,
}

impl ToolchainCacheStatus {
    /// Return the toolchain identity digest.
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// Return the recognized toolchain-tree size in bytes.
    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    /// Return the last-use time when valid metadata exists.
    pub fn last_use(&self) -> Option<SystemTime> {
        self.last_use
    }

    /// Return whether another operation currently holds the toolchain lease.
    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// Return the recognized workspace entries in identity order.
    pub fn workspaces(&self) -> &[WorkspaceCacheStatus] {
        &self.workspaces
    }
}

/// A read-only snapshot of the dedicated Ruskel cache.
#[derive(Clone, Debug)]
pub struct CacheStatus {
    /// Canonical cache root.
    root: PathBuf,
    /// Total recognized build and trash usage.
    total_bytes: u64,
    /// Recognized toolchain entries.
    toolchains: Vec<ToolchainCacheStatus>,
    /// Recognized trash usage.
    trash_bytes: u64,
    /// Entries that status preserved or could not inspect.
    skipped: Vec<CacheIssue>,
    /// Bytes above the soft high-water mark.
    excess_bytes: u64,
}

impl CacheStatus {
    /// Return the canonical cache root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Return total recognized build and trash usage in bytes.
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// Return recognized toolchain entries in identity order.
    pub fn toolchains(&self) -> &[ToolchainCacheStatus] {
        &self.toolchains
    }

    /// Return recognized trash usage in bytes.
    pub fn trash_bytes(&self) -> u64 {
        self.trash_bytes
    }

    /// Return entries that status could not safely recognize or inspect.
    pub fn skipped(&self) -> &[CacheIssue] {
        &self.skipped
    }

    /// Return usage above the soft high-water mark.
    pub fn excess_bytes(&self) -> u64 {
        self.excess_bytes
    }
}

/// Result of one explicit cache-clean operation.
#[derive(Clone, Debug)]
pub struct CleanReport {
    /// Number of removed top-level owned entries.
    removed_entries: u64,
    /// Recognized bytes removed.
    removed_bytes: u64,
    /// Whether an active shared root lease prevented cleaning.
    root_busy: bool,
    /// Unrecognized entries preserved by cleaning.
    skipped: Vec<CacheIssue>,
    /// Failed removal operations.
    failures: Vec<CacheIssue>,
    /// Recognized cache usage after cleaning.
    usage_after: u64,
}

impl CleanReport {
    /// Return the number of removed toolchain and trash entries.
    pub fn removed_entries(&self) -> u64 {
        self.removed_entries
    }

    /// Return the recognized bytes removed by this operation.
    pub fn removed_bytes(&self) -> u64 {
        self.removed_bytes
    }

    /// Return whether an active request prevented the clean.
    pub fn root_busy(&self) -> bool {
        self.root_busy
    }

    /// Return unrecognized entries that the clean preserved.
    pub fn skipped(&self) -> &[CacheIssue] {
        &self.skipped
    }

    /// Return removal failures.
    pub fn failures(&self) -> &[CacheIssue] {
        &self.failures
    }

    /// Return recognized usage after the clean.
    pub fn usage_after(&self) -> u64 {
        self.usage_after
    }

    /// Return whether the clean removed all safe entries without contention or failure.
    pub fn is_complete(&self) -> bool {
        !self.root_busy && self.failures.is_empty()
    }
}

/// Cloneable lazy cache configuration attached to a `Ruskel` instance.
#[derive(Clone, Debug)]
pub struct CacheHandle {
    /// Explicit root that overrides environment and platform resolution.
    explicit_root: Option<PathBuf>,
    /// Lazily resolved process-local owner.
    resolved: Arc<OnceLock<Arc<CacheOwner>>>,
}

impl CacheHandle {
    /// Create a lazy handle with an optional explicit cache root.
    pub fn new(explicit_root: Option<PathBuf>) -> Self {
        Self {
            explicit_root,
            resolved: Arc::new(OnceLock::new()),
        }
    }

    /// Resolve the shared owner only when an operation needs the cache.
    pub fn owner(&self) -> Result<Arc<CacheOwner>> {
        if let Some(owner) = self.resolved.get() {
            return Ok(Arc::clone(owner));
        }

        let root = resolve_root(self.explicit_root.as_deref())?;
        let initialized = CacheLayout::initialize(root)?;
        let canonical_root = initialized.canonical_root()?;

        let mut owners = OWNERS.lock().map_err(|_| {
            RuskelError::Generate("Ruskel cache owner registry is unavailable".to_string())
        })?;
        owners.retain(|_, owner| owner.strong_count() > 0);
        let owner = owners
            .get(&canonical_root)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| {
                let owner = Arc::new(CacheOwner {
                    layout: initialized,
                });
                owners.insert(canonical_root, Arc::downgrade(&owner));
                owner
            });
        drop(self.resolved.set(Arc::clone(&owner)));
        Ok(owner)
    }
}

/// One process-local owner for a canonical cache root.
#[derive(Debug)]
pub struct CacheOwner {
    /// Validated filesystem layout.
    layout: CacheLayout,
}

impl CacheOwner {
    /// Acquire the root, toolchain, and workspace leases for one build.
    pub fn begin_build(
        self: &Arc<Self>,
        toolchain_identity: &str,
        workspace_root: &Path,
    ) -> Result<BuildLease> {
        if !is_identity(toolchain_identity) {
            return Err(RuskelError::Generate(
                "Nightly toolchain identity is not a SHA-256 digest".to_string(),
            ));
        }
        let workspace_identity = identity_for_path(workspace_root);
        let root_lease = self.layout.lock_root_shared()?;
        let toolchain_lock = self.layout.toolchain_lock(toolchain_identity);
        let toolchain_lease = self.layout.lock_shared(&toolchain_lock)?;
        let workspace_lock = self.layout.workspace_lock(&workspace_identity);
        let workspace_lease = self.layout.lock_exclusive(&workspace_lock)?;

        let toolchain_dir = self.layout.build_dir().join(toolchain_identity);
        let workspace_dir = toolchain_dir.join(&workspace_identity);
        fs::create_dir_all(&workspace_dir).map_err(|source| RuskelError::CacheIo {
            action: "create workspace cache entry",
            path: workspace_dir.clone(),
            source,
        })?;

        let now = unix_now()?;
        layout::write_timestamp(&toolchain_dir.join(LAST_USE), now)?;
        layout::write_timestamp(&workspace_dir.join(LAST_USE), now)?;

        Ok(BuildLease {
            owner: Arc::clone(self),
            toolchain_identity: toolchain_identity.to_string(),
            workspace_identity,
            workspace_dir,
            _root_lease: root_lease,
            _toolchain_lease: toolchain_lease,
            _workspace_lease: workspace_lease,
        })
    }

    /// Return a status snapshot while holding a shared root lease.
    pub fn status(&self) -> Result<CacheStatus> {
        let _root = self.layout.lock_root_shared()?;
        self.status_unlocked()
    }

    /// Clean owned build data without waiting for active root leases.
    pub fn clean(&self) -> Result<CleanReport> {
        let Some(_root) = self.layout.try_lock_root()? else {
            let usage_after = self.status()?.total_bytes;
            return Ok(CleanReport {
                removed_entries: 0,
                removed_bytes: 0,
                root_busy: true,
                skipped: Vec::new(),
                failures: Vec::new(),
                usage_after,
            });
        };

        let mut report = CleanReport {
            removed_entries: 0,
            removed_bytes: 0,
            root_busy: false,
            skipped: Vec::new(),
            failures: Vec::new(),
            usage_after: 0,
        };
        self.clean_build_entries(&mut report)?;
        self.clean_trash_entries(&mut report)?;
        let status = self.status_unlocked()?;
        report.usage_after = status.total_bytes;
        report.skipped.extend(status.skipped);
        Ok(report)
    }

    /// Collect status when the caller already holds the required root lease.
    fn status_unlocked(&self) -> Result<CacheStatus> {
        let mut skipped = Vec::new();
        let mut toolchains = Vec::new();
        let build_dir = self.layout.build_dir();

        for entry in read_dir_sorted(&build_dir)? {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if !is_owned_directory(&path) || !is_identity(&name) {
                skipped.push(CacheIssue::new(path, "unrecognized toolchain cache entry"));
                continue;
            }
            match self.toolchain_status(&name, &path, &mut skipped) {
                Ok(status) => toolchains.push(status),
                Err(error) => skipped.push(CacheIssue::new(path, error.to_string())),
            }
        }

        let trash_bytes = self.trash_status(&mut skipped)?;
        let build_bytes = toolchains
            .iter()
            .fold(0_u64, |total, entry| total.saturating_add(entry.size_bytes));
        let total_bytes = build_bytes.saturating_add(trash_bytes);

        Ok(CacheStatus {
            root: self.layout.canonical_root()?,
            total_bytes,
            toolchains,
            trash_bytes,
            skipped,
            excess_bytes: total_bytes.saturating_sub(HIGH_WATER_BYTES),
        })
    }

    /// Collect one recognized toolchain tree and its workspaces.
    fn toolchain_status(
        &self,
        identity: &str,
        path: &Path,
        skipped: &mut Vec<CacheIssue>,
    ) -> Result<ToolchainCacheStatus> {
        let mut workspaces = Vec::new();
        for entry in read_dir_sorted(path)? {
            let name = entry.file_name().to_string_lossy().into_owned();
            let entry_path = entry.path();
            if name == LAST_USE {
                continue;
            }
            if !is_owned_directory(&entry_path) || !is_identity(&name) {
                skipped.push(CacheIssue::new(
                    entry_path,
                    "unrecognized workspace cache entry",
                ));
                continue;
            }
            let size_bytes = match layout::path_size(&entry_path) {
                Ok(size) => size,
                Err(error) => {
                    skipped.push(CacheIssue::new(&entry_path, error.to_string()));
                    continue;
                }
            };
            let last_use = read_system_time(&entry_path.join(LAST_USE), skipped);
            let locked = self.layout.is_locked(&self.layout.workspace_lock(&name))?;
            workspaces.push(WorkspaceCacheStatus {
                identity: name,
                size_bytes,
                last_use,
                locked,
            });
        }
        let size_bytes = layout::path_size(path)?;
        let last_use = read_system_time(&path.join(LAST_USE), skipped);
        let locked = self
            .layout
            .is_locked(&self.layout.toolchain_lock(identity))?;
        Ok(ToolchainCacheStatus {
            identity: identity.to_string(),
            size_bytes,
            last_use,
            locked,
            workspaces,
        })
    }

    /// Measure recognized trash entries and report foreign entries.
    fn trash_status(&self, skipped: &mut Vec<CacheIssue>) -> Result<u64> {
        let mut total = 0_u64;
        for entry in read_dir_sorted(&self.layout.trash_dir())? {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if !is_trash_name(&name) || !is_owned_directory(&path) {
                skipped.push(CacheIssue::new(path, "unrecognized trash entry"));
                continue;
            }
            match layout::path_size(&path) {
                Ok(size) => total = total.saturating_add(size),
                Err(error) => skipped.push(CacheIssue::new(path, error.to_string())),
            }
        }
        Ok(total)
    }

    /// Move and remove recognized toolchain trees.
    fn clean_build_entries(&self, report: &mut CleanReport) -> Result<()> {
        for entry in read_dir_sorted(&self.layout.build_dir())? {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if !is_identity(&name) || !is_owned_directory(&path) {
                report
                    .skipped
                    .push(CacheIssue::new(path, "unrecognized toolchain cache entry"));
                continue;
            }
            let size = layout::path_size(&path).unwrap_or(0);
            let label = format!("{name}.{name}");
            match layout::move_to_trash(self.layout.root(), &path, &label)
                .and_then(|trash| layout::remove_no_follow(&trash))
            {
                Ok(()) => {
                    report.removed_entries += 1;
                    report.removed_bytes = report.removed_bytes.saturating_add(size);
                }
                Err(error) => report
                    .failures
                    .push(CacheIssue::new(path, error.to_string())),
            }
        }
        Ok(())
    }

    /// Remove recognized entries that are already in trash.
    fn clean_trash_entries(&self, report: &mut CleanReport) -> Result<()> {
        for entry in read_dir_sorted(&self.layout.trash_dir())? {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if !is_trash_name(&name) || !is_owned_directory(&path) {
                report
                    .skipped
                    .push(CacheIssue::new(path, "unrecognized trash entry"));
                continue;
            }
            let size = layout::path_size(&path).unwrap_or(0);
            match layout::remove_no_follow(&path) {
                Ok(()) => {
                    report.removed_entries += 1;
                    report.removed_bytes = report.removed_bytes.saturating_add(size);
                }
                Err(error) => report
                    .failures
                    .push(CacheIssue::new(path, error.to_string())),
            }
        }
        Ok(())
    }
}

/// Held leases and output location for one rustdoc build attempt.
#[derive(Debug)]
pub struct BuildLease {
    /// Cache owner that provides paths and keeps process state alive.
    owner: Arc<CacheOwner>,
    /// Full nightly toolchain identity.
    toolchain_identity: String,
    /// Full canonical workspace identity.
    workspace_identity: String,
    /// Cargo target and build directory for this request.
    workspace_dir: PathBuf,
    /// Shared root lease.
    _root_lease: File,
    /// Shared toolchain lease.
    _toolchain_lease: File,
    /// Exclusive workspace lease.
    _workspace_lease: File,
}

impl BuildLease {
    /// Return the absolute directory used for Cargo target and build artifacts.
    pub fn build_dir(&self) -> &Path {
        &self.workspace_dir
    }

    /// Update the entry timestamps after a successful JSON read.
    pub fn touch_success(&self) -> Result<()> {
        let now = unix_now()?;
        let toolchain_dir = self.owner.layout.build_dir().join(&self.toolchain_identity);
        layout::write_timestamp(&toolchain_dir.join(LAST_USE), now)?;
        layout::write_timestamp(&self.workspace_dir.join(LAST_USE), now)
    }

    /// Move the workspace entry to owned trash while its leases are held.
    pub fn move_to_trash(&self) -> Result<PathBuf> {
        let label = format!("{}.{}", self.toolchain_identity, self.workspace_identity);
        layout::move_to_trash(self.owner.layout.root(), &self.workspace_dir, &label)
    }
}

/// Compute a full SHA-256 digest for canonical workspace path bytes.
fn identity_for_path(path: &Path) -> String {
    let mut hasher = Sha256::new();
    hash_path_bytes(&mut hasher, path);
    format!("{:x}", hasher.finalize())
}

/// Resolve the configured cache root and make relative inputs absolute.
fn resolve_root(explicit: Option<&Path>) -> Result<PathBuf> {
    let root = if let Some(explicit) = explicit {
        explicit.to_path_buf()
    } else if let Some(environment) =
        env::var_os("RUSKEL_CACHE_DIR").filter(|value| !value.is_empty())
    {
        PathBuf::from(environment)
    } else {
        dirs::cache_dir()
            .ok_or_else(|| RuskelError::CacheLayout {
                path: PathBuf::from("<platform cache directory>"),
                message: "the platform does not provide a cache directory".to_string(),
            })?
            .join("ruskel")
    };

    if root.is_absolute() {
        Ok(root)
    } else {
        env::current_dir()
            .map(|current| current.join(&root))
            .map_err(|source| RuskelError::CacheIo {
                action: "resolve cache root",
                path: root,
                source,
            })
    }
}

/// Return current Unix time for cache metadata.
fn unix_now() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| RuskelError::Generate("System time is before the Unix epoch".to_string()))
}

/// Read one last-use timestamp and convert invalid metadata into a status issue.
fn read_system_time(path: &Path, skipped: &mut Vec<CacheIssue>) -> Option<SystemTime> {
    match layout::read_timestamp(path) {
        Ok(Some(seconds)) => Some(UNIX_EPOCH + Duration::from_secs(seconds)),
        Ok(None) => {
            skipped.push(CacheIssue::new(path, "last-use metadata is missing"));
            None
        }
        Err(error) => {
            skipped.push(CacheIssue::new(path, error.to_string()));
            None
        }
    }
}

/// Read a directory in deterministic file-name order.
fn read_dir_sorted(path: &Path) -> Result<Vec<fs::DirEntry>> {
    let mut entries = fs::read_dir(path)
        .map_err(|source| RuskelError::CacheIo {
            action: "read cache directory",
            path: path.to_path_buf(),
            source,
        })?
        .collect::<result::Result<Vec<_>, _>>()
        .map_err(|source| RuskelError::CacheIo {
            action: "read cache directory",
            path: path.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(fs::DirEntry::file_name);
    Ok(entries)
}

/// Return whether a path is a real directory and not a symbolic link.
fn is_owned_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
}

#[cfg(unix)]
/// Add platform-native Unix path bytes to an identity digest.
fn hash_path_bytes(hasher: &mut Sha256, path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    hasher.update(path.as_os_str().as_bytes());
}

#[cfg(windows)]
/// Add little-endian native Windows path units to an identity digest.
fn hash_path_bytes(hasher: &mut Sha256, path: &Path) {
    use std::os::windows::ffi::OsStrExt;
    for unit in path.as_os_str().encode_wide() {
        hasher.update(unit.to_le_bytes());
    }
}

#[cfg(not(any(unix, windows)))]
/// Add a lossy path representation on unsupported platforms.
fn hash_path_bytes(hasher: &mut Sha256, path: &Path) {
    hasher.update(path.as_os_str().to_string_lossy().as_bytes());
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn owner(root: &Path) -> Result<Arc<CacheOwner>> {
        CacheHandle::new(Some(root.to_path_buf())).owner()
    }

    #[test]
    fn explicit_root_is_absolute_and_shared_between_handles() -> Result<()> {
        let temp = tempdir()?;
        let first = CacheHandle::new(Some(temp.path().join("cache"))).owner()?;
        let second = CacheHandle::new(Some(temp.path().join("cache"))).owner()?;
        assert!(Arc::ptr_eq(&first, &second));
        assert!(first.layout.root().is_absolute());
        Ok(())
    }

    #[test]
    fn build_lease_creates_identity_entries_and_status() -> Result<()> {
        let temp = tempdir()?;
        let owner = owner(&temp.path().join("cache"))?;
        let toolchain = "a".repeat(64);
        let workspace = fs::canonicalize(temp.path())?;
        let lease = owner.begin_build(&toolchain, &workspace)?;
        fs::write(lease.build_dir().join("artifact"), b"abc")?;
        lease.touch_success()?;
        drop(lease);

        let status = owner.status()?;
        assert_eq!(status.toolchains().len(), 1);
        assert_eq!(status.toolchains()[0].workspaces().len(), 1);
        assert!(status.total_bytes() >= 3);
        Ok(())
    }

    #[test]
    fn clean_preserves_marker_and_lock_namespace() -> Result<()> {
        let temp = tempdir()?;
        let owner = owner(&temp.path().join("cache"))?;
        let toolchain = "b".repeat(64);
        let workspace = fs::canonicalize(temp.path())?;
        let lease = owner.begin_build(&toolchain, &workspace)?;
        fs::write(lease.build_dir().join("artifact"), b"abc")?;
        drop(lease);

        let report = owner.clean()?;
        assert!(report.is_complete());
        assert!(owner.layout.root().join(layout::MARKER_NAME).is_file());
        assert!(owner.layout.root().join("locks").is_dir());
        assert!(owner.layout.root().join("build").is_dir());
        assert!(owner.layout.root().join("trash").is_dir());
        Ok(())
    }

    #[test]
    fn workspace_identity_uses_full_digest() {
        let identity = identity_for_path(Path::new("/one/workspace"));
        assert_eq!(identity.len(), 64);
        assert!(is_identity(&identity));
        assert_ne!(identity, identity_for_path(Path::new("/two/workspace")));
    }
}
