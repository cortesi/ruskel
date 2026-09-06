//! Typed, no-follow inventory of the cache-owned build and trash trees.

use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    result,
};

use super::{
    layout::{self, CacheLayout, LAST_USE, is_identity, is_trash_name},
    report::CacheIssue,
};
use crate::error::{Result, RuskelError};

/// Optional display metadata stored inside each workspace entry.
pub const WORKSPACE_METADATA: &str = "ruskel.workspace.json";

/// One recognized workspace cache entry.
#[derive(Clone, Debug)]
pub struct WorkspaceInventory {
    /// Owning toolchain identity.
    pub toolchain: String,
    /// Workspace identity.
    pub identity: String,
    /// Absolute entry path.
    pub path: PathBuf,
    /// Recognized entry size.
    pub size_bytes: Option<u64>,
    /// Parsed last-use timestamp.
    pub last_use: Option<u64>,
    /// Physical cache root used to validate the complete ancestry.
    cache_root: PathBuf,
}

impl WorkspaceInventory {
    /// Return whether this snapshot still names an owned workspace directory.
    pub fn revalidate(&self) -> bool {
        self.path
            .file_name()
            .is_some_and(|name| name == self.identity.as_str())
            && layout::validate_owned_directory(&self.cache_root, &self.path).is_ok()
            && is_identity(&self.identity)
            && layout::read_timestamp(&self.path.join(LAST_USE))
                .is_ok_and(|value| value == self.last_use)
            && layout::path_size(&self.path).ok() == self.size_bytes
    }
}

/// One recognized toolchain cache entry.
#[derive(Clone, Debug)]
pub struct ToolchainInventory {
    /// Toolchain identity.
    pub identity: String,
    /// Absolute entry path.
    pub path: PathBuf,
    /// Recognized toolchain metadata and workspace size.
    pub size_bytes: u64,
    /// Parsed last-use timestamp.
    pub last_use: Option<u64>,
    /// Recognized workspaces in identity order.
    pub workspaces: Vec<WorkspaceInventory>,
    /// Physical cache root used to validate the complete ancestry.
    cache_root: PathBuf,
}

impl ToolchainInventory {
    /// Return whether this snapshot still names an owned toolchain directory.
    pub fn revalidate(&self) -> bool {
        self.path
            .file_name()
            .is_some_and(|name| name == self.identity.as_str())
            && layout::validate_owned_directory(&self.cache_root, &self.path).is_ok()
            && is_identity(&self.identity)
            && layout::read_timestamp(&self.path.join(LAST_USE))
                .is_ok_and(|value| value == self.last_use)
    }
}

/// One recognized trash entry.
#[derive(Clone, Debug)]
pub struct TrashInventory {
    /// Trash entry name.
    pub name: String,
    /// Absolute entry path.
    pub path: PathBuf,
    /// Recognized entry size.
    pub size_bytes: Option<u64>,
    /// Physical cache root used to validate the complete ancestry.
    cache_root: PathBuf,
}

impl TrashInventory {
    /// Return whether this snapshot still names an owned trash directory.
    pub fn revalidate(&self) -> bool {
        self.path
            .file_name()
            .is_some_and(|name| name == self.name.as_str())
            && layout::validate_owned_directory(&self.cache_root, &self.path).is_ok()
            && is_trash_name(&self.name)
            && layout::path_size(&self.path).ok() == self.size_bytes
    }
}

/// One deterministic snapshot of recognized cache-owned data.
#[derive(Clone, Debug)]
pub struct CacheInventory {
    /// Recognized toolchains.
    pub toolchains: Vec<ToolchainInventory>,
    /// Recognized trash entries.
    pub trash: Vec<TrashInventory>,
    /// Foreign, malformed, or unreadable entries.
    pub issues: Vec<CacheIssue>,
}

impl CacheInventory {
    /// Traverse the recognized cache tree without following symbolic links.
    pub fn collect(layout: &CacheLayout) -> Result<Self> {
        layout.validate_static_directories()?;
        let mut inventory = Self {
            toolchains: Vec::new(),
            trash: Vec::new(),
            issues: Vec::new(),
        };
        inventory.collect_build(layout)?;
        inventory.collect_trash(layout)?;
        Ok(inventory)
    }

    /// Return canonical recognized build and trash usage.
    pub fn recognized_usage(&self) -> u64 {
        let build = self.toolchains.iter().fold(0_u64, |total, toolchain| {
            total.saturating_add(toolchain.size_bytes)
        });
        self.trash.iter().fold(build, |total, trash| {
            total.saturating_add(trash.size_bytes.unwrap_or(0))
        })
    }

    /// Return recognized trash usage.
    pub fn trash_usage(&self) -> u64 {
        self.trash.iter().fold(0_u64, |total, trash| {
            total.saturating_add(trash.size_bytes.unwrap_or(0))
        })
    }

    /// Traverse toolchain and workspace entries.
    fn collect_build(&mut self, layout: &CacheLayout) -> Result<()> {
        for entry in read_dir_sorted(&layout.build_dir())? {
            let identity = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if !is_identity(&identity) || !is_owned_directory(&path) {
                self.issues
                    .push(CacheIssue::new(path, "unrecognized toolchain cache entry"));
                continue;
            }
            let toolchain = self.collect_toolchain(layout, identity, path);
            self.toolchains.push(toolchain);
        }
        Ok(())
    }

    /// Traverse one recognized toolchain.
    fn collect_toolchain(
        &mut self,
        layout: &CacheLayout,
        identity: String,
        path: PathBuf,
    ) -> ToolchainInventory {
        let last_use_path = path.join(LAST_USE);
        let last_use = read_timestamp(&last_use_path, &mut self.issues);
        let mut size_bytes = metadata_size(&last_use_path, &mut self.issues);
        let mut workspaces = Vec::new();

        match read_dir_sorted(&path) {
            Ok(entries) => {
                for entry in entries {
                    let workspace = entry.file_name().to_string_lossy().into_owned();
                    let workspace_path = entry.path();
                    if workspace == LAST_USE {
                        continue;
                    }
                    if !is_identity(&workspace) || !is_owned_directory(&workspace_path) {
                        self.issues.push(CacheIssue::new(
                            workspace_path,
                            "unrecognized workspace cache entry",
                        ));
                        continue;
                    }
                    let workspace_last_use =
                        read_timestamp(&workspace_path.join(LAST_USE), &mut self.issues);
                    let workspace_size = match layout::path_size(&workspace_path) {
                        Ok(size) => {
                            size_bytes = size_bytes.saturating_add(size);
                            Some(size)
                        }
                        Err(error) => {
                            self.issues
                                .push(CacheIssue::new(&workspace_path, error.to_string()));
                            None
                        }
                    };
                    workspaces.push(WorkspaceInventory {
                        toolchain: identity.clone(),
                        identity: workspace,
                        path: workspace_path,
                        size_bytes: workspace_size,
                        last_use: workspace_last_use,
                        cache_root: layout.root().to_path_buf(),
                    });
                }
            }
            Err(error) => self.issues.push(CacheIssue::new(&path, error.to_string())),
        }

        ToolchainInventory {
            identity,
            path,
            size_bytes,
            last_use,
            workspaces,
            cache_root: layout.root().to_path_buf(),
        }
    }

    /// Traverse recognized trash entries.
    fn collect_trash(&mut self, layout: &CacheLayout) -> Result<()> {
        for entry in read_dir_sorted(&layout.trash_dir())? {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if !is_trash_name(&name) || !is_owned_directory(&path) {
                self.issues
                    .push(CacheIssue::new(path, "unrecognized trash entry"));
                continue;
            }
            let size_bytes = match layout::path_size(&path) {
                Ok(size) => Some(size),
                Err(error) => {
                    self.issues.push(CacheIssue::new(&path, error.to_string()));
                    None
                }
            };
            self.trash.push(TrashInventory {
                name,
                path,
                size_bytes,
                cache_root: layout.root().to_path_buf(),
            });
        }
        Ok(())
    }
}

/// Read a timestamp and retain malformed or missing metadata as an issue.
fn read_timestamp(path: &Path, issues: &mut Vec<CacheIssue>) -> Option<u64> {
    match layout::read_timestamp(path) {
        Ok(Some(value)) => Some(value),
        Ok(None) => {
            issues.push(CacheIssue::new(path, "last-use metadata is missing"));
            None
        }
        Err(error) => {
            issues.push(CacheIssue::new(path, error.to_string()));
            None
        }
    }
}

/// Measure one optional metadata file without treating absence as owned usage.
fn metadata_size(path: &Path, issues: &mut Vec<CacheIssue>) -> u64 {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => metadata.len(),
        Ok(_) => {
            issues.push(CacheIssue::new(
                path,
                "cache metadata must be a regular file",
            ));
            0
        }
        Err(error) if error.kind() == ErrorKind::NotFound => 0,
        Err(error) => {
            issues.push(CacheIssue::new(path, error.to_string()));
            0
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

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn inventory_excludes_foreign_data_and_revalidates_candidates() -> Result<()> {
        let temp = tempdir()?;
        let layout = CacheLayout::initialize(temp.path().join("cache"))?;
        let toolchain = "a".repeat(64);
        let workspace = "b".repeat(64);
        let toolchain_path = layout.build_dir().join(&toolchain);
        let workspace_path = toolchain_path.join(&workspace);
        fs::create_dir_all(&workspace_path)?;
        layout::write_timestamp(&toolchain_path.join(LAST_USE), 10)?;
        layout::write_timestamp(&workspace_path.join(LAST_USE), 10)?;
        fs::write(workspace_path.join("artifact"), b"owned")?;
        fs::write(layout.build_dir().join("foreign"), vec![0_u8; 4096])?;

        let inventory = CacheInventory::collect(&layout)?;
        assert_eq!(inventory.toolchains.len(), 1);
        assert_eq!(inventory.toolchains[0].workspaces.len(), 1);
        assert!(inventory.recognized_usage() < 4096);
        assert!(
            inventory
                .issues
                .iter()
                .any(|issue| issue.path().ends_with("foreign"))
        );

        let candidate = inventory.toolchains[0].workspaces[0].clone();
        fs::remove_dir_all(&workspace_path)?;
        assert!(!candidate.revalidate());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn inventory_rejects_symlinks_and_invalid_timestamps() -> Result<()> {
        use std::os::unix::fs::symlink;

        let temp = tempdir()?;
        let layout = CacheLayout::initialize(temp.path().join("cache"))?;
        let toolchain = "c".repeat(64);
        let workspace = "d".repeat(64);
        let toolchain_path = layout.build_dir().join(&toolchain);
        let workspace_path = toolchain_path.join(&workspace);
        fs::create_dir_all(&workspace_path)?;
        fs::write(toolchain_path.join(LAST_USE), b"invalid")?;
        layout::write_timestamp(&workspace_path.join(LAST_USE), 10)?;
        symlink(
            temp.path(),
            layout
                .trash_dir()
                .join(format!("{toolchain}.{workspace}.1.1")),
        )?;

        let inventory = CacheInventory::collect(&layout)?;
        assert!(inventory.toolchains[0].last_use.is_none());
        assert!(inventory.trash.is_empty());
        assert!(
            inventory
                .issues
                .iter()
                .any(|issue| issue.message().contains("timestamp"))
        );
        assert!(
            inventory
                .issues
                .iter()
                .any(|issue| issue.message().contains("trash"))
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn revalidation_rejects_a_replaced_owned_parent() -> Result<()> {
        let temp = tempdir()?;
        let layout = CacheLayout::initialize(temp.path().join("cache"))?;
        let toolchain = "e".repeat(64);
        let workspace = "f".repeat(64);
        let toolchain_path = layout.build_dir().join(&toolchain);
        let workspace_path = toolchain_path.join(&workspace);
        fs::create_dir_all(&workspace_path)?;
        layout::write_timestamp(&toolchain_path.join(LAST_USE), 10)?;
        layout::write_timestamp(&workspace_path.join(LAST_USE), 10)?;
        fs::write(workspace_path.join("artifact"), b"owned")?;
        fs::write(toolchain_path.join("sentinel"), b"keep")?;

        let inventory = CacheInventory::collect(&layout)?;
        let toolchain_candidate = inventory.toolchains[0].clone();
        let workspace_candidate = toolchain_candidate.workspaces[0].clone();
        let saved = temp.path().join("toolchain-saved");
        fs::rename(&toolchain_path, &saved)?;
        symlink(&saved, &toolchain_path)?;

        assert!(!toolchain_candidate.revalidate());
        assert!(!workspace_candidate.revalidate());
        assert_eq!(fs::read(saved.join("sentinel"))?, b"keep");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn trash_revalidation_rejects_a_replaced_owned_parent() -> Result<()> {
        let temp = tempdir()?;
        let layout = CacheLayout::initialize(temp.path().join("cache"))?;
        let name = format!("{}.{}.1.1", "a".repeat(64), "b".repeat(64));
        let trash_entry = layout.trash_dir().join(&name);
        fs::create_dir(&trash_entry)?;
        fs::write(trash_entry.join("sentinel"), b"keep")?;

        let inventory = CacheInventory::collect(&layout)?;
        let candidate = inventory.trash[0].clone();
        let outside = temp.path().join("trash-outside");
        fs::create_dir(&outside)?;
        let saved = outside.join(&name);
        fs::rename(&trash_entry, &saved)?;
        fs::remove_dir(layout.trash_dir())?;
        symlink(&outside, layout.trash_dir())?;

        assert!(!candidate.revalidate());
        assert_eq!(fs::read(saved.join("sentinel"))?, b"keep");
        Ok(())
    }
}
