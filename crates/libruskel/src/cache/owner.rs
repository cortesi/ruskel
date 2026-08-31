//! Cache ownership, identity, reporting, and build leases.

use std::{
    collections::{BTreeMap, HashMap},
    env, fs,
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    inventory::{CacheInventory, ToolchainInventory, WORKSPACE_METADATA},
    layout::{self, CacheLayout, LAST_USE, is_identity},
    maintenance::MaintenanceWorker,
    report::{CacheIssue, CacheStatus, CleanReport, ToolchainCacheStatus, WorkspaceCacheStatus},
};
use crate::error::{Result, RuskelError};

/// Default maximum recognized cache usage before maintenance evicts entries.
pub(super) const HIGH_WATER_BYTES: u64 = 20_000_000_000;
/// Weak registry that shares cache owners by canonical root.
static OWNERS: Lazy<Mutex<HashMap<PathBuf, Weak<CacheOwner>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Optional human-readable metadata for one hashed workspace entry.
#[derive(Debug, Deserialize, Serialize)]
struct WorkspaceMetadata {
    /// UTF-8 canonical root that produced the workspace identity.
    workspace_root: String,
    /// Current observed version for each package in the workspace.
    packages: BTreeMap<String, String>,
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
        let owner = if let Some(owner) = owners.get(&canonical_root).and_then(Weak::upgrade) {
            owner
        } else {
            let maintenance = MaintenanceWorker::start(initialized.clone())?;
            let owner = Arc::new(CacheOwner {
                layout: initialized,
                maintenance,
            });
            owners.insert(canonical_root, Arc::downgrade(&owner));
            owner
        };
        drop(self.resolved.set(Arc::clone(&owner)));
        Ok(owner)
    }
}

/// One process-local owner for a canonical cache root.
#[derive(Debug)]
pub struct CacheOwner {
    /// Validated filesystem layout.
    layout: CacheLayout,
    /// Coalesced process-local maintenance worker.
    maintenance: MaintenanceWorker,
}

impl CacheOwner {
    /// Acquire the root, toolchain, and workspace leases for one build.
    pub fn begin_build(
        self: &Arc<Self>,
        toolchain_identity: &str,
        workspace_root: &Path,
        package_name: &str,
        package_version: &str,
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
        update_workspace_metadata(
            &workspace_dir,
            workspace_root,
            package_name,
            package_version,
        )?;

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
                root: self.layout.canonical_root()?,
                removed_entries: 0,
                removed_bytes: 0,
                root_busy: true,
                skipped: Vec::new(),
                failures: Vec::new(),
                usage_after,
            });
        };

        let mut report = CleanReport {
            root: self.layout.canonical_root()?,
            removed_entries: 0,
            removed_bytes: 0,
            root_busy: false,
            skipped: Vec::new(),
            failures: Vec::new(),
            usage_after: 0,
        };
        let inventory = CacheInventory::collect(&self.layout)?;
        report.skipped.extend(inventory.issues.clone());
        for trash in inventory.trash {
            if !trash.revalidate() {
                report.skipped.push(CacheIssue::new(
                    trash.path,
                    "trash entry changed after inventory",
                ));
                continue;
            }
            let size = trash.size_bytes.unwrap_or(0);
            match layout::remove_no_follow(&trash.path) {
                Ok(()) => {
                    report.removed_entries += 1;
                    report.removed_bytes = report.removed_bytes.saturating_add(size);
                }
                Err(error) => report
                    .failures
                    .push(CacheIssue::new(trash.path, error.to_string())),
            }
        }
        for toolchain in inventory.toolchains {
            if !toolchain.revalidate() {
                report.skipped.push(CacheIssue::new(
                    toolchain.path,
                    "toolchain entry changed after inventory",
                ));
                continue;
            }
            let label = format!("{}.{}", toolchain.identity, toolchain.identity);
            match layout::move_to_trash(self.layout.root(), &toolchain.path, &label)
                .and_then(|trash| layout::remove_no_follow(&trash))
            {
                Ok(()) => {
                    report.removed_entries += 1;
                    report.removed_bytes =
                        report.removed_bytes.saturating_add(toolchain.size_bytes);
                }
                Err(error) => report
                    .failures
                    .push(CacheIssue::new(toolchain.path, error.to_string())),
            }
        }
        let status = self.status_unlocked()?;
        report.usage_after = status.total_bytes;
        for issue in status.skipped {
            if !report.skipped.contains(&issue) {
                report.skipped.push(issue);
            }
        }
        Ok(report)
    }

    /// Return whether the cache filesystem currently has low available space.
    pub fn is_low_space(&self) -> bool {
        self.maintenance.is_low_space(self.layout.root())
    }

    /// Submit one maintenance signal after a completed build attempt.
    pub fn signal_maintenance(&self, toolchain_identity: &str, urgent: bool) {
        self.maintenance.signal(toolchain_identity, urgent);
    }

    /// Run synchronous storage recovery after build leases are released.
    pub fn recover_storage(&self, toolchain_identity: &str) -> Result<String> {
        let result = self.maintenance.recover(&self.layout, toolchain_identity)?;
        if result.waited {
            return Ok("waited for the active maintenance pass".to_string());
        }
        if !result.ran {
            return Ok("maintenance was not required".to_string());
        }
        Ok(format!(
            "completed synchronous maintenance and removed {} entries ({} bytes, {} skipped issues)",
            result.removed_entries,
            result.removed_bytes,
            result.issues.len()
        ))
    }

    /// Return whether an error path is inside the owned build namespace.
    pub fn is_entry_error(&self, error: &RuskelError) -> bool {
        matches!(
            error,
            RuskelError::CacheIo { path, .. } if path.starts_with(self.layout.build_dir())
        )
    }

    /// Move one known workspace entry to trash after a lease-setup failure.
    pub fn quarantine_workspace(
        &self,
        toolchain_identity: &str,
        workspace_root: &Path,
    ) -> Result<Option<PathBuf>> {
        let workspace_identity = identity_for_path(workspace_root);
        let _root = self.layout.lock_root_shared()?;
        let _toolchain = self
            .layout
            .lock_shared(&self.layout.toolchain_lock(toolchain_identity))?;
        let _workspace = self
            .layout
            .lock_exclusive(&self.layout.workspace_lock(&workspace_identity))?;
        let path = self
            .layout
            .build_dir()
            .join(toolchain_identity)
            .join(&workspace_identity);
        if !path.exists() {
            return Ok(None);
        }
        let label = format!("{toolchain_identity}.{workspace_identity}");
        layout::move_to_trash(self.layout.root(), &path, &label).map(Some)
    }

    /// Collect status when the caller already holds the required root lease.
    fn status_unlocked(&self) -> Result<CacheStatus> {
        let inventory = CacheInventory::collect(&self.layout)?;
        let mut skipped = inventory.issues.clone();
        let mut toolchains = Vec::new();
        for toolchain in &inventory.toolchains {
            match self.toolchain_status(toolchain, &mut skipped) {
                Ok(status) => toolchains.push(status),
                Err(error) => {
                    skipped.push(CacheIssue::new(&toolchain.path, error.to_string()));
                }
            }
        }

        let trash_bytes = inventory.trash_usage();
        let total_bytes = inventory.recognized_usage();

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
        inventory: &ToolchainInventory,
        skipped: &mut Vec<CacheIssue>,
    ) -> Result<ToolchainCacheStatus> {
        let mut workspaces = Vec::new();
        for workspace in &inventory.workspaces {
            let Some(size_bytes) = workspace.size_bytes else {
                continue;
            };
            let last_use = system_time(workspace.last_use, &workspace.path.join(LAST_USE), skipped);
            let metadata = read_workspace_status_metadata(
                &workspace.path.join(WORKSPACE_METADATA),
                &workspace.identity,
                skipped,
            );
            let locked = self
                .layout
                .is_locked(&self.layout.workspace_lock(&workspace.identity))?;
            workspaces.push(WorkspaceCacheStatus {
                identity: workspace.identity.clone(),
                workspace_root: metadata
                    .as_ref()
                    .map(|metadata| PathBuf::from(&metadata.workspace_root)),
                packages: metadata
                    .map(|metadata| {
                        metadata
                            .packages
                            .into_iter()
                            .map(|(name, version)| format!("{name} {version}"))
                            .collect()
                    })
                    .unwrap_or_default(),
                size_bytes,
                last_use,
                locked,
            });
        }
        let last_use = system_time(inventory.last_use, &inventory.path.join(LAST_USE), skipped);
        let locked = self
            .layout
            .is_locked(&self.layout.toolchain_lock(&inventory.identity))?;
        Ok(ToolchainCacheStatus {
            identity: inventory.identity.clone(),
            size_bytes: inventory.size_bytes,
            last_use,
            locked,
            workspaces,
        })
    }
}

impl Drop for CacheOwner {
    fn drop(&mut self) {
        self.maintenance.shutdown();
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
    hex::encode(hasher.finalize())
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

/// Merge one observed package into a workspace entry's display metadata.
fn update_workspace_metadata(
    workspace_dir: &Path,
    workspace_root: &Path,
    package_name: &str,
    package_version: &str,
) -> Result<()> {
    let Some(workspace_root) = workspace_root.to_str() else {
        return Ok(());
    };
    let path = workspace_dir.join(WORKSPACE_METADATA);
    let mut metadata = read_workspace_metadata(&path)?.unwrap_or_else(|| WorkspaceMetadata {
        workspace_root: workspace_root.to_string(),
        packages: BTreeMap::new(),
    });
    if metadata.workspace_root != workspace_root {
        return Err(RuskelError::CacheLayout {
            path,
            message: "workspace metadata does not match its cache identity".to_string(),
        });
    }
    if metadata
        .packages
        .insert(package_name.to_string(), package_version.to_string())
        .as_deref()
        == Some(package_version)
    {
        return Ok(());
    }
    let mut encoded = serde_json::to_vec(&metadata).map_err(|error| RuskelError::CacheLayout {
        path: path.clone(),
        message: format!("could not encode workspace metadata: {error}"),
    })?;
    encoded.push(b'\n');
    layout::write_atomic_metadata(&path, &encoded, "write workspace metadata")
}

/// Read optional workspace display metadata.
fn read_workspace_metadata(path: &Path) -> Result<Option<WorkspaceMetadata>> {
    layout::read_optional_metadata(path)?.map_or(Ok(None), |encoded| {
        serde_json::from_slice(&encoded)
            .map(Some)
            .map_err(|error| RuskelError::CacheLayout {
                path: path.to_path_buf(),
                message: format!("workspace metadata is invalid: {error}"),
            })
    })
}

/// Read status metadata, converting invalid or mismatched data into a skipped
/// issue.
fn read_workspace_status_metadata(
    path: &Path,
    identity: &str,
    skipped: &mut Vec<CacheIssue>,
) -> Option<WorkspaceMetadata> {
    match read_workspace_metadata(path) {
        Ok(Some(metadata))
            if identity_for_path(Path::new(&metadata.workspace_root)) == identity =>
        {
            Some(metadata)
        }
        Ok(Some(_)) => {
            skipped.push(CacheIssue::new(
                path,
                "workspace metadata does not match its cache identity",
            ));
            None
        }
        Ok(None) => None,
        Err(error) => {
            skipped.push(CacheIssue::new(path, error.to_string()));
            None
        }
    }
}

/// Read one last-use timestamp and convert invalid metadata into a status
/// issue.
fn system_time(
    seconds: Option<u64>,
    path: &Path,
    skipped: &mut Vec<CacheIssue>,
) -> Option<SystemTime> {
    match seconds {
        Some(seconds) => {
            let timestamp = UNIX_EPOCH + Duration::from_secs(seconds);
            if timestamp > SystemTime::now() {
                skipped.push(CacheIssue::new(path, "last-use metadata is in the future"));
                None
            } else {
                Some(timestamp)
            }
        }
        None => None,
    }
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
    use std::{
        env, fs,
        io::{self, BufRead, BufReader, Read, Write},
        process::{Command, Stdio},
        sync::{Barrier, mpsc::sync_channel},
        thread,
        time::Duration,
    };

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
    fn build_lease_records_workspace_and_accumulates_packages() -> Result<()> {
        let temp = tempdir()?;
        let owner = owner(&temp.path().join("cache"))?;
        let toolchain = "a".repeat(64);
        let workspace = fs::canonicalize(temp.path())?;
        let lease = owner.begin_build(&toolchain, &workspace, "first", "0.1.0")?;
        fs::write(lease.build_dir().join("artifact"), b"abc")?;
        lease.touch_success()?;
        drop(lease);
        drop(owner.begin_build(&toolchain, &workspace, "second", "0.2.0")?);
        drop(owner.begin_build(&toolchain, &workspace, "first", "0.1.1")?);

        let status = owner.status()?;
        assert_eq!(status.toolchains().len(), 1);
        assert_eq!(status.toolchains()[0].workspaces().len(), 1);
        let workspace_status = &status.toolchains()[0].workspaces()[0];
        assert_eq!(workspace_status.workspace_root(), Some(workspace.as_path()));
        assert_eq!(
            workspace_status.packages(),
            &["first 0.1.1", "second 0.2.0"]
        );
        assert!(status.total_bytes() >= 3);
        Ok(())
    }

    #[test]
    fn clean_preserves_marker_and_lock_namespace() -> Result<()> {
        let temp = tempdir()?;
        let owner = owner(&temp.path().join("cache"))?;
        let toolchain = "b".repeat(64);
        let workspace = fs::canonicalize(temp.path())?;
        let lease = owner.begin_build(&toolchain, &workspace, "clean-test", "0.1.0")?;
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
    fn status_falls_back_to_identity_for_invalid_workspace_metadata() -> Result<()> {
        let temp = tempdir()?;
        let owner = owner(&temp.path().join("cache"))?;
        let toolchain = "9".repeat(64);
        let workspace = fs::canonicalize(temp.path())?;
        let lease = owner.begin_build(&toolchain, &workspace, "broken", "0.1.0")?;
        let metadata_path = lease.build_dir().join(WORKSPACE_METADATA);
        fs::write(&metadata_path, b"not JSON")?;
        drop(lease);

        let status = owner.status()?;
        let workspace_status = &status.toolchains()[0].workspaces()[0];
        assert_eq!(workspace_status.workspace_root(), None);
        assert!(workspace_status.packages().is_empty());
        assert!(status.skipped().iter().any(|issue| {
            issue.path() == metadata_path
                && issue.message().contains("workspace metadata is invalid")
        }));
        Ok(())
    }

    #[test]
    fn workspace_identity_uses_full_digest() {
        let identity = identity_for_path(Path::new("/one/workspace"));
        assert_eq!(identity.len(), 64);
        assert!(is_identity(&identity));
        assert_ne!(identity, identity_for_path(Path::new("/two/workspace")));
    }

    #[test]
    fn build_leases_serialize_one_workspace_with_barrier_coordination() -> Result<()> {
        let temp = tempdir()?;
        let owner = owner(&temp.path().join("cache"))?;
        let toolchain = "c".repeat(64);
        let workspace = fs::canonicalize(temp.path())?;
        let first = owner.begin_build(&toolchain, &workspace, "serial-test", "0.1.0")?;
        let start = Arc::new(Barrier::new(2));
        let thread_start = Arc::clone(&start);
        let thread_owner = Arc::clone(&owner);
        let thread_toolchain = toolchain;
        let thread_workspace = workspace.clone();
        let handle = thread::spawn(move || {
            thread_start.wait();
            thread_owner.begin_build(&thread_toolchain, &thread_workspace, "serial-test", "0.1.0")
        });

        start.wait();
        assert!(
            owner
                .layout
                .is_locked(&owner.layout.workspace_lock(&identity_for_path(&workspace)))?
        );
        drop(first);
        let second = handle.join().expect("workspace lease thread")?;
        drop(second);
        Ok(())
    }

    #[test]
    fn build_leases_allow_different_workspaces_to_overlap() -> Result<()> {
        let temp = tempdir()?;
        let first_workspace = temp.path().join("first");
        let second_workspace = temp.path().join("second");
        fs::create_dir_all(&first_workspace)?;
        fs::create_dir_all(&second_workspace)?;
        let first_workspace = fs::canonicalize(first_workspace)?;
        let second_workspace = fs::canonicalize(second_workspace)?;
        let owner = owner(&temp.path().join("cache"))?;
        let toolchain = "d".repeat(64);
        let first = owner.begin_build(&toolchain, &first_workspace, "first", "0.1.0")?;
        let start = Arc::new(Barrier::new(2));
        let thread_start = Arc::clone(&start);
        let thread_owner = Arc::clone(&owner);
        let thread_toolchain = toolchain;
        let (acquired_tx, acquired_rx) = sync_channel(0);
        let handle = thread::spawn(move || -> Result<()> {
            thread_start.wait();
            let second = thread_owner.begin_build(
                &thread_toolchain,
                &second_workspace,
                "second",
                "0.1.0",
            )?;
            acquired_tx
                .send(())
                .map_err(|error| RuskelError::Generate(error.to_string()))?;
            drop(second);
            Ok(())
        });

        start.wait();
        acquired_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|error| RuskelError::Generate(error.to_string()))?;
        drop(first);
        handle.join().expect("parallel workspace lease thread")?;
        Ok(())
    }

    #[test]
    fn cache_lock_subprocess_helper() -> Result<()> {
        let Some(mode) = env::var_os("RUSKEL_CACHE_LOCK_HELPER") else {
            return Ok(());
        };
        let root = PathBuf::from(
            env::var_os("RUSKEL_CACHE_LOCK_ROOT")
                .ok_or_else(|| RuskelError::Generate("missing helper root".to_string()))?,
        );
        let layout = CacheLayout::initialize(root)?;
        let identity = "e".repeat(64);
        let _lease = match mode.to_string_lossy().as_ref() {
            "root" => layout.lock_root_shared()?,
            "toolchain" => layout.lock_shared(&layout.toolchain_lock(&identity))?,
            "workspace" => layout.lock_exclusive(&layout.workspace_lock(&identity))?,
            "gc" => layout.lock_exclusive(&layout.gc_lock())?,
            other => {
                return Err(RuskelError::Generate(format!(
                    "unknown lock helper mode: {other}"
                )));
            }
        };
        println!("READY");
        io::stdout().flush()?;
        let mut release = [0_u8; 1];
        io::stdin().read_exact(&mut release)?;
        Ok(())
    }

    #[test]
    fn subprocesses_expose_root_toolchain_workspace_and_gc_locks() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("cache");
        let layout = CacheLayout::initialize(root.clone())?;
        let identity = "e".repeat(64);

        for mode in ["root", "toolchain", "workspace", "gc"] {
            let mut child = Command::new(env::current_exe()?)
                .args([
                    "--exact",
                    "cache::owner::tests::cache_lock_subprocess_helper",
                    "--nocapture",
                ])
                .env("RUSKEL_CACHE_LOCK_HELPER", mode)
                .env("RUSKEL_CACHE_LOCK_ROOT", &root)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| RuskelError::Generate("missing helper stdout".to_string()))?;
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line)? == 0 || line.contains("READY") {
                    break;
                }
            }
            assert!(line.contains("READY"), "lock helper did not become ready");

            let busy = match mode {
                "root" => layout.try_lock_root()?.is_none(),
                "toolchain" => layout
                    .try_lock_exclusive(&layout.toolchain_lock(&identity))?
                    .is_none(),
                "workspace" => layout
                    .try_lock_exclusive(&layout.workspace_lock(&identity))?
                    .is_none(),
                "gc" => layout.try_lock_exclusive(&layout.gc_lock())?.is_none(),
                _ => unreachable!(),
            };
            assert!(busy, "{mode} lock was not visible across processes");

            child
                .stdin
                .take()
                .ok_or_else(|| RuskelError::Generate("missing helper stdin".to_string()))?
                .write_all(b"x")?;
            assert!(child.wait()?.success());
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn clean_reports_partial_removal_failure() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempdir()?;
        let owner = owner(&temp.path().join("cache"))?;
        let toolchain = "f".repeat(64);
        let workspace = fs::canonicalize(temp.path())?;
        let lease = owner.begin_build(&toolchain, &workspace, "partial-clean", "0.1.0")?;
        fs::write(lease.build_dir().join("artifact"), b"data")?;
        let workspace_identity = identity_for_path(&workspace);
        let workspace_path = lease.build_dir().to_path_buf();
        drop(lease);
        fs::set_permissions(&workspace_path, fs::Permissions::from_mode(0o500))?;

        let report = owner.clean()?;
        assert!(!report.is_complete());
        assert_eq!(report.failures().len(), 1);

        for entry in fs::read_dir(owner.layout.trash_dir())? {
            let entry = entry?;
            let protected = entry.path().join(&workspace_identity);
            if protected.exists() {
                fs::set_permissions(protected, fs::Permissions::from_mode(0o700))?;
            }
        }
        let cleanup = owner.clean()?;
        assert!(cleanup.failures().is_empty());
        Ok(())
    }
}
