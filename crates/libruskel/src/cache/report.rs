//! Public cache status and cleaning reports.

use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

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
    pub(super) fn new(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
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
    pub(super) identity: String,
    /// Canonical workspace root recorded by a current Ruskel build.
    pub(super) workspace_root: Option<PathBuf>,
    /// Sorted package labels observed in this workspace entry.
    pub(super) packages: Vec<String>,
    /// Recognized entry size.
    pub(super) size_bytes: u64,
    /// Valid last-use time.
    pub(super) last_use: Option<SystemTime>,
    /// Observed workspace-lock state.
    pub(super) locked: bool,
}

impl WorkspaceCacheStatus {
    /// Return the workspace identity digest.
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// Return the canonical workspace root when display metadata exists.
    pub fn workspace_root(&self) -> Option<&Path> {
        self.workspace_root.as_deref()
    }

    /// Return sorted package name and version labels observed in this entry.
    pub fn packages(&self) -> &[String] {
        &self.packages
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
    pub(super) identity: String,
    /// Recognized toolchain-tree size.
    pub(super) size_bytes: u64,
    /// Valid last-use time.
    pub(super) last_use: Option<SystemTime>,
    /// Observed toolchain-lock state.
    pub(super) locked: bool,
    /// Recognized workspace entries.
    pub(super) workspaces: Vec<WorkspaceCacheStatus>,
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
    pub(super) root: PathBuf,
    /// Total recognized build and trash usage.
    pub(super) total_bytes: u64,
    /// Recognized toolchain entries.
    pub(super) toolchains: Vec<ToolchainCacheStatus>,
    /// Recognized trash usage.
    pub(super) trash_bytes: u64,
    /// Entries that status preserved or could not inspect.
    pub(super) skipped: Vec<CacheIssue>,
    /// Bytes above the soft high-water mark.
    pub(super) excess_bytes: u64,
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
    /// Canonical cache root operated on by this clean.
    pub(super) root: PathBuf,
    /// Number of removed top-level owned entries.
    pub(super) removed_entries: u64,
    /// Recognized bytes removed.
    pub(super) removed_bytes: u64,
    /// Whether an active shared root lease prevented cleaning.
    pub(super) root_busy: bool,
    /// Unrecognized entries preserved by cleaning.
    pub(super) skipped: Vec<CacheIssue>,
    /// Failed removal operations.
    pub(super) failures: Vec<CacheIssue>,
    /// Recognized cache usage after cleaning.
    pub(super) usage_after: u64,
}

impl CleanReport {
    /// Return the canonical cache root.
    pub fn root(&self) -> &Path {
        &self.root
    }

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

    /// Return whether the clean removed all safe entries without contention or
    /// failure.
    pub fn is_complete(&self) -> bool {
        !self.root_busy && self.failures.is_empty()
    }
}
