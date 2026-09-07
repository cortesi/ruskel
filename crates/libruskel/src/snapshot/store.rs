//! Safe snapshot tree comparison and persistence.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Result as IoResult, Write},
    path::{Path, PathBuf},
    str,
};

use fs4::FileExt;
use tempfile::{Builder, TempDir};

use crate::{
    error::{Result, RuskelError},
    snapshot::{
        ApiSnapshot, GENERATED_SOURCE_HEADER, SnapshotProfile,
        manifest::{MARKER_FILENAME, Manifest, ManifestCrate},
    },
};

/// Persistence behavior for one snapshot destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotMode {
    /// Replace the generated tree when captured bytes differ.
    Update,
    /// Compare captured bytes without changing the generated tree.
    Check,
}

/// One typed snapshot comparison result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotChangeKind {
    /// The captured entry does not exist in the stored tree.
    Added,
    /// The captured and stored entry bytes differ.
    Changed,
    /// A previously managed entry is no longer captured.
    Removed,
    /// The captured and stored entry bytes are identical.
    Unchanged,
    /// The destination contains an entry that the marker does not own.
    Unexpected,
    /// A bounded backup shows an interrupted directory swap.
    Interrupted,
}

/// One path and its typed comparison result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotChange {
    /// Path relative to the snapshot parent.
    path: PathBuf,
    /// Typed comparison result.
    kind: SnapshotChangeKind,
}

impl SnapshotChange {
    /// Return the path relative to the snapshot parent.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Return the comparison result.
    pub fn kind(&self) -> SnapshotChangeKind {
        self.kind
    }
}

/// Complete ordered result from one snapshot synchronization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotReport {
    /// Ordered marker, crate, drift, and interruption results.
    changes: Vec<SnapshotChange>,
    /// Ordered binary-only package names.
    skipped_packages: Vec<String>,
}

impl SnapshotReport {
    /// Return marker and file results in display order.
    pub fn changes(&self) -> &[SnapshotChange] {
        &self.changes
    }

    /// Return skipped binary-only packages in package order.
    pub fn skipped_packages(&self) -> &[String] {
        &self.skipped_packages
    }

    /// Return whether the destination matches the captured snapshot.
    pub fn is_current(&self) -> bool {
        self.changes
            .iter()
            .all(|change| change.kind == SnapshotChangeKind::Unchanged)
    }
}

/// One physically resolved snapshot destination and its observed marker state.
#[derive(Debug)]
pub struct SnapshotStore {
    /// Physical output path.
    path: PathBuf,
    /// Persistent physical sibling lock.
    lock_path: PathBuf,
    /// Exact bounded sibling backup prefix.
    backup_prefix: String,
    /// Selected persistence mode.
    mode: SnapshotMode,
    /// Exact marker bytes observed during open.
    observed_marker: Option<Vec<u8>>,
    /// Validated logical stored marker.
    manifest: Option<Manifest>,
    /// Output or interrupted backup used for comparisons.
    logical_path: PathBuf,
    /// Ordered interrupted backup report paths.
    interrupted: Vec<PathBuf>,
}

/// Validated filesystem state for one generated tree.
#[derive(Debug)]
struct TreeState {
    /// Exact current marker bytes.
    marker: Option<Vec<u8>>,
    /// Validated current marker.
    manifest: Option<Manifest>,
    /// Sorted entries not owned by the marker.
    unexpected: Vec<PathBuf>,
    /// Marker-owned files that are absent.
    missing: BTreeSet<String>,
}

impl SnapshotStore {
    /// Open and validate one generated destination.
    pub fn open(path: impl AsRef<Path>, mode: SnapshotMode) -> Result<Self> {
        let (path, lock_path, backup_prefix) = physical_identity(path.as_ref())?;
        let _lock = acquire_lock(&lock_path, &path)?;
        let backups = find_backups(&path, &backup_prefix)?;
        if backups.len() > 1 {
            return Err(store_error(
                &path,
                format!("multiple interrupted backups use prefix '{backup_prefix}'"),
            ));
        }

        let mut interrupted = Vec::new();
        let backup_state = backups
            .first()
            .map(|backup| validate_tree(backup, false, false).map(|state| (backup, state)))
            .transpose()?;

        if mode == SnapshotMode::Update {
            if let Some((backup, _)) = backup_state {
                if path.exists() {
                    let output = validate_tree(&path, false, false)?;
                    if output.manifest.is_none() {
                        return Err(store_error(
                            &path,
                            "cannot remove an interrupted backup beside an incomplete output",
                        ));
                    }
                    remove_validated_tree(backup, &path)?;
                } else {
                    fs::rename(backup, &path).map_err(|error| {
                        store_error(
                            &path,
                            format!("failed to restore interrupted backup: {error}"),
                        )
                    })?;
                }
            }
        } else if let Some((backup, _)) = &backup_state {
            interrupted.push(
                backup
                    .file_name()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| backup.to_path_buf()),
            );
        }

        let actual = validate_tree(&path, mode == SnapshotMode::Check, true)?;
        let observed_marker = actual.marker.clone();
        let (manifest, logical_path) = if mode == SnapshotMode::Check && !path.exists() {
            backup_state
                .map(|(backup, state)| (state.manifest, backup.to_path_buf()))
                .unwrap_or((None, path.clone()))
        } else {
            (actual.manifest, path.clone())
        };
        Ok(Self {
            path,
            lock_path,
            backup_prefix,
            mode,
            observed_marker,
            manifest,
            logical_path,
            interrupted,
        })
    }

    /// Return the stored capture profile without local toolchain validation.
    pub fn profile(&self) -> Option<&SnapshotProfile> {
        self.manifest.as_ref().map(|manifest| &manifest.profile)
    }

    /// Compare or atomically update the destination with one captured snapshot.
    pub fn sync(&self, snapshot: &ApiSnapshot) -> Result<SnapshotReport> {
        let desired_manifest = Manifest::from_snapshot(snapshot.profile(), snapshot.crates());
        let desired_marker = desired_manifest.serialize();
        let desired = desired_files(snapshot, &desired_marker);
        let _lock = acquire_lock(&self.lock_path, &self.path)?;
        let actual = validate_tree(&self.path, self.mode == SnapshotMode::Check, true)?;

        if actual.marker != self.observed_marker {
            if tree_is_identical(&actual, &self.path, &desired)? {
                return report(
                    &actual,
                    actual.manifest.as_ref(),
                    &self.path,
                    &desired_manifest,
                    &desired,
                    snapshot.skipped_packages(),
                    &self.interrupted,
                );
            }
            return Err(store_error(
                &self.path,
                "destination marker changed after open; refusing to overwrite a newer snapshot",
            ));
        }

        let current = if self.logical_path == self.path {
            actual
        } else {
            validate_tree(&self.logical_path, true, true)?
        };

        let report = report(
            &current,
            self.manifest.as_ref(),
            &self.logical_path,
            &desired_manifest,
            &desired,
            snapshot.skipped_packages(),
            &self.interrupted,
        )?;
        if self.mode == SnapshotMode::Check || report.is_current() {
            return Ok(report);
        }
        if !current.unexpected.is_empty() {
            return Err(store_error(
                &self.path,
                format!(
                    "destination contains unowned entry '{}'",
                    current.unexpected[0].display()
                ),
            ));
        }

        let temporary = write_complete_tree(&self.path, &desired)?;
        let check = validate_tree(temporary.path(), false, false)?;
        if !tree_is_identical(&check, temporary.path(), &desired)? {
            return Err(store_error(
                &self.path,
                "temporary snapshot tree failed validation",
            ));
        }
        commit_tree(&self.path, &self.backup_prefix, temporary)?;
        Ok(report)
    }
}

/// Resolve a caller spelling to one physical output and lock identity.
fn physical_identity(input: &Path) -> Result<(PathBuf, PathBuf, String)> {
    if input.file_name().is_none() {
        return Err(store_error(
            input,
            "destination must have a final path component",
        ));
    }
    if let Ok(metadata) = fs::symlink_metadata(input)
        && metadata.file_type().is_symlink()
    {
        return Err(store_error(input, "destination cannot be a symlink"));
    }
    let path = if input.exists() {
        input.canonicalize().map_err(|error| {
            store_error(
                input,
                format!("failed to canonicalize destination: {error}"),
            )
        })?
    } else {
        let parent = input.parent().unwrap_or_else(|| Path::new("."));
        let parent = parent.canonicalize().map_err(|error| {
            store_error(
                input,
                format!("failed to canonicalize destination parent: {error}"),
            )
        })?;
        parent.join(input.file_name().expect("checked final component"))
    };
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| store_error(&path, "destination name must be UTF-8"))?;
    let backup_prefix = format!(".{name}.ruskel-snapshot-backup-");
    let lock_path = path
        .parent()
        .expect("physical destination has parent")
        .join(format!(".{name}.ruskel-snapshot.lock"));
    Ok((path, lock_path, backup_prefix))
}

/// Acquire the persistent exclusive advisory lock.
fn acquire_lock(lock_path: &Path, destination: &Path) -> Result<File> {
    let file = match OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(lock_path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(lock_path).map_err(|error| {
                store_error(
                    destination,
                    format!("failed to inspect snapshot lock: {error}"),
                )
            })?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(store_error(
                    destination,
                    "snapshot lock must be a regular file",
                ));
            }
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(lock_path)
                .map_err(|error| {
                    store_error(
                        destination,
                        format!("failed to open snapshot lock: {error}"),
                    )
                })?
        }
        Err(error) => {
            return Err(store_error(
                destination,
                format!("failed to open snapshot lock: {error}"),
            ));
        }
    };
    let metadata = file.metadata().map_err(|error| {
        store_error(
            destination,
            format!("failed to inspect open snapshot lock: {error}"),
        )
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(store_error(
            destination,
            "snapshot lock must be a regular file",
        ));
    }
    FileExt::lock(&file)
        .map_err(|error| store_error(destination, format!("failed to lock snapshot: {error}")))?;
    Ok(file)
}

/// Find bounded sibling backup candidates in lexical order.
fn find_backups(path: &Path, prefix: &str) -> Result<Vec<PathBuf>> {
    let parent = path.parent().expect("destination has parent");
    let mut backups = fs::read_dir(parent)
        .map_err(|error| store_error(path, format!("failed to inspect snapshot parent: {error}")))?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()
                .filter(|name| name.starts_with(prefix))
                .map(|_| entry.path())
        })
        .collect::<Vec<_>>();
    backups.sort();
    Ok(backups)
}

/// Validate one generated tree without following owned symlinks.
fn validate_tree(
    path: &Path,
    allow_unmarked: bool,
    allow_missing_owned: bool,
) -> Result<TreeState> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(TreeState {
                marker: None,
                manifest: None,
                unexpected: Vec::new(),
                missing: BTreeSet::new(),
            });
        }
        Err(error) => {
            return Err(store_error(
                path,
                format!("failed to inspect destination: {error}"),
            ));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(store_error(
            path,
            "destination must be a directory and not a symlink",
        ));
    }
    let marker_path = path.join(MARKER_FILENAME);
    let marker = match fs::symlink_metadata(&marker_path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(store_error(
                &marker_path,
                "ownership marker must be a regular file",
            ));
        }
        Ok(_) => Some(fs::read(&marker_path).map_err(|error| {
            store_error(
                &marker_path,
                format!("failed to read ownership marker: {error}"),
            )
        })?),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => {
            return Err(store_error(
                &marker_path,
                format!("failed to inspect marker: {error}"),
            ));
        }
    };
    let Some(marker_bytes) = marker else {
        let mut unexpected = directory_entries(path)?;
        unexpected.sort();
        if !allow_unmarked && !unexpected.is_empty() {
            return Err(store_error(
                path,
                "non-empty destination has no ownership marker",
            ));
        }
        return Ok(TreeState {
            marker: None,
            manifest: None,
            unexpected,
            missing: BTreeSet::new(),
        });
    };
    let manifest = Manifest::parse(&marker_bytes, &marker_path)?;
    let owned = manifest
        .crates
        .iter()
        .map(|entry| entry.file.as_str())
        .collect::<BTreeSet<_>>();
    let mut unexpected = Vec::new();
    for entry in directory_entries(path)? {
        let name = entry.to_string_lossy();
        if name != MARKER_FILENAME && !owned.contains(name.as_ref()) {
            unexpected.push(entry);
        }
    }
    unexpected.sort();
    if !allow_unmarked && !unexpected.is_empty() {
        return Err(store_error(
            path,
            format!(
                "destination contains unowned entry '{}'",
                unexpected[0].display()
            ),
        ));
    }
    let mut missing = BTreeSet::new();
    for entry in &manifest.crates {
        let file = path.join(&entry.file);
        let metadata = match fs::symlink_metadata(&file) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound && allow_missing_owned => {
                missing.insert(entry.file.clone());
                continue;
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Err(store_error(&file, "managed file is missing"));
            }
            Err(error) => {
                return Err(store_error(
                    &file,
                    format!("failed to inspect managed file: {error}"),
                ));
            }
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(store_error(
                &file,
                "managed path must be a regular file and not a symlink",
            ));
        }
        let bytes = fs::read(&file)
            .map_err(|error| store_error(&file, format!("failed to read managed file: {error}")))?;
        let text = str::from_utf8(&bytes)
            .map_err(|error| store_error(&file, format!("managed file is not UTF-8: {error}")))?;
        if !has_generated_header(text, entry) {
            return Err(store_error(
                &file,
                "managed file does not have the generated header",
            ));
        }
    }
    Ok(TreeState {
        marker: Some(marker_bytes),
        manifest: Some(manifest),
        unexpected,
        missing,
    })
}

/// Accept the current header and the exact legacy identity header for
/// migration.
fn has_generated_header(text: &str, entry: &ManifestCrate) -> bool {
    if text.starts_with(GENERATED_SOURCE_HEADER) {
        return true;
    }
    let legacy = format!(
        "// @generated by `ruskel-snapshot`; do not edit.\n// snapshot-format: 1\n// package: {}\n// crate: {}\n\n",
        entry.package, entry.crate_name
    );
    text.starts_with(&legacy)
}

/// Return each immediate destination entry as a relative path.
fn directory_entries(path: &Path) -> Result<Vec<PathBuf>> {
    fs::read_dir(path)
        .map_err(|error| store_error(path, format!("failed to read destination: {error}")))?
        .map(|entry| {
            entry
                .map(|entry| PathBuf::from(entry.file_name()))
                .map_err(|error| {
                    store_error(path, format!("failed to read destination entry: {error}"))
                })
        })
        .collect()
}

/// Build all desired marker and crate file bytes.
fn desired_files(snapshot: &ApiSnapshot, marker: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut desired = BTreeMap::from([(MARKER_FILENAME.to_string(), marker.to_vec())]);
    for entry in snapshot.crates() {
        desired.insert(
            entry.filename().to_string(),
            entry.contents().as_bytes().to_vec(),
        );
    }
    desired
}

/// Compare a validated complete tree byte-for-byte.
fn tree_is_identical(
    state: &TreeState,
    root: &Path,
    desired: &BTreeMap<String, Vec<u8>>,
) -> Result<bool> {
    if !state.unexpected.is_empty() || !state.missing.is_empty() {
        return Ok(false);
    }
    let Some(manifest) = &state.manifest else {
        return Ok(false);
    };
    if desired.len() != manifest.crates.len() + 1 {
        return Ok(false);
    }
    for (name, bytes) in desired {
        match fs::read(root.join(name)) {
            Ok(current) if current == *bytes => {}
            Ok(_) => return Ok(false),
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(store_error(
                    root,
                    format!("failed to compare '{name}': {error}"),
                ));
            }
        }
    }
    Ok(true)
}

/// Build one canonical typed comparison report.
fn report(
    current: &TreeState,
    stored_manifest: Option<&Manifest>,
    destination: &Path,
    desired_manifest: &Manifest,
    desired: &BTreeMap<String, Vec<u8>>,
    skipped: &[String],
    interrupted: &[PathBuf],
) -> Result<SnapshotReport> {
    let marker_kind = match current.marker.as_ref() {
        None => SnapshotChangeKind::Added,
        Some(actual) if Some(actual) == desired.get(MARKER_FILENAME) => {
            SnapshotChangeKind::Unchanged
        }
        Some(_) => SnapshotChangeKind::Changed,
    };
    let mut changes = vec![SnapshotChange {
        path: PathBuf::from(MARKER_FILENAME),
        kind: marker_kind,
    }];
    for entry in &desired_manifest.crates {
        let kind = match fs::read(destination.join(&entry.file)) {
            Ok(actual) if Some(&actual) == desired.get(&entry.file) => {
                SnapshotChangeKind::Unchanged
            }
            Ok(_) => SnapshotChangeKind::Changed,
            Err(error) if error.kind() == ErrorKind::NotFound => SnapshotChangeKind::Added,
            Err(error) => {
                return Err(store_error(
                    destination,
                    format!("failed to compare '{}': {error}", entry.file),
                ));
            }
        };
        changes.push(SnapshotChange {
            path: PathBuf::from(&entry.file),
            kind,
        });
    }
    let old = stored_manifest
        .map(|manifest| {
            manifest
                .crates
                .iter()
                .map(|entry| entry.file.as_str())
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let new = desired_manifest
        .crates
        .iter()
        .map(|entry| entry.file.as_str())
        .collect::<BTreeSet<_>>();
    let mut trailing = old
        .difference(&new)
        .map(|name| SnapshotChange {
            path: PathBuf::from(name),
            kind: SnapshotChangeKind::Removed,
        })
        .collect::<Vec<_>>();
    trailing.extend(
        current
            .unexpected
            .iter()
            .cloned()
            .map(|path| SnapshotChange {
                path,
                kind: SnapshotChangeKind::Unexpected,
            }),
    );
    trailing.sort_by(|left, right| left.path.cmp(&right.path));
    changes.extend(trailing);
    changes.extend(interrupted.iter().cloned().map(|path| SnapshotChange {
        path,
        kind: SnapshotChangeKind::Interrupted,
    }));
    Ok(SnapshotReport {
        changes,
        skipped_packages: skipped.to_vec(),
    })
}

/// Write and sync one complete sibling temporary tree.
fn write_complete_tree(destination: &Path, desired: &BTreeMap<String, Vec<u8>>) -> Result<TempDir> {
    let parent = destination.parent().expect("destination has parent");
    let temporary = Builder::new()
        .prefix(".ruskel-snapshot-new-")
        .tempdir_in(parent)
        .map_err(|error| {
            store_error(
                destination,
                format!("failed to create temporary tree: {error}"),
            )
        })?;
    for (name, bytes) in desired {
        let path = temporary.path().join(name);
        let mut file = File::create(&path).map_err(|error| {
            store_error(destination, format!("failed to create '{name}': {error}"))
        })?;
        file.write_all(bytes).map_err(|error| {
            store_error(destination, format!("failed to write '{name}': {error}"))
        })?;
        file.sync_all().map_err(|error| {
            store_error(destination, format!("failed to sync '{name}': {error}"))
        })?;
    }
    Ok(temporary)
}

/// Commit a complete temporary tree with filesystem renames.
fn commit_tree(destination: &Path, backup_prefix: &str, temporary: TempDir) -> Result<()> {
    commit_tree_with(destination, backup_prefix, temporary, |from, to| {
        fs::rename(from, to)
    })
}

/// Commit with an injectable rename operation for rollback proof.
fn commit_tree_with<R>(
    destination: &Path,
    backup_prefix: &str,
    temporary: TempDir,
    mut rename: R,
) -> Result<()>
where
    R: FnMut(&Path, &Path) -> IoResult<()>,
{
    let parent = destination.parent().expect("destination has parent");
    let backup_dir = Builder::new()
        .prefix(backup_prefix)
        .tempdir_in(parent)
        .map_err(|error| {
            store_error(
                destination,
                format!("failed to reserve backup path: {error}"),
            )
        })?;
    let backup = backup_dir.keep();
    fs::remove_dir(&backup).map_err(|error| {
        store_error(
            destination,
            format!("failed to prepare backup path: {error}"),
        )
    })?;
    let had_destination = destination.exists();
    if had_destination {
        rename(destination, &backup).map_err(|error| {
            store_error(
                destination,
                format!("failed to move old snapshot to backup: {error}"),
            )
        })?;
    }
    let new_tree = temporary.keep();
    if let Err(commit_error) = rename(&new_tree, destination) {
        if had_destination && let Err(restore_error) = rename(&backup, destination) {
            let cleanup = remove_validated_tree(&new_tree, destination);
            let message = match cleanup {
                Ok(()) => format!(
                    "failed to commit new snapshot: {commit_error}; rollback also failed: {restore_error}"
                ),
                Err(cleanup_error) => format!(
                    "failed to commit new snapshot: {commit_error}; rollback also failed: {restore_error}; temporary cleanup also failed: {cleanup_error}"
                ),
            };
            return Err(store_error(destination, message));
        }
        if let Err(cleanup_error) = remove_validated_tree(&new_tree, destination) {
            return Err(store_error(
                destination,
                format!(
                    "failed to commit new snapshot: {commit_error}; temporary cleanup also failed: {cleanup_error}"
                ),
            ));
        }
        return Err(store_error(
            destination,
            format!("failed to commit new snapshot: {commit_error}"),
        ));
    }
    if had_destination {
        remove_validated_tree(&backup, destination)?;
    }
    Ok(())
}

/// Remove only a complete tree whose marker proves ownership.
fn remove_validated_tree(tree: &Path, destination: &Path) -> Result<()> {
    validate_tree(tree, false, true)?;
    fs::remove_dir_all(tree).map_err(|error| {
        store_error(
            destination,
            format!(
                "failed to remove validated backup '{}': {error}",
                tree.display()
            ),
        )
    })
}

/// Build a path-specific snapshot store error.
fn store_error(path: &Path, message: impl Into<String>) -> RuskelError {
    RuskelError::SnapshotStore {
        path: path.to_path_buf(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::{fs::MetadataExt, fs::symlink};
    use std::{
        fs,
        io::Error,
        process,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
    };

    use super::*;
    use crate::snapshot::{CrateSnapshot, SnapshotFeatures};

    fn profile() -> SnapshotProfile {
        SnapshotProfile::from_marker(
            1,
            "nightly-2099-01-01".into(),
            "fixture-target".into(),
            SnapshotFeatures::default(),
        )
        .expect("fixture profile")
    }

    fn crate_snapshot(package: &str, crate_name: &str, body: &str) -> CrateSnapshot {
        CrateSnapshot {
            package: package.into(),
            crate_name: crate_name.into(),
            filename: format!("{package}.rs"),
            contents: format!(
                "{}pub mod {crate_name} {{\n    {body}\n}}\n",
                GENERATED_SOURCE_HEADER
            ),
        }
    }

    fn snapshot(crates: Vec<CrateSnapshot>) -> ApiSnapshot {
        ApiSnapshot {
            profile: profile(),
            crates,
            skipped_packages: vec!["tool".into()],
        }
    }

    fn marker_for(snapshot: &ApiSnapshot) -> Vec<u8> {
        Manifest::from_snapshot(snapshot.profile(), snapshot.crates()).serialize()
    }

    #[test]
    fn generated_header_accepts_the_legacy_form_for_migration() {
        let entry = ManifestCrate {
            package: "alpha-package".into(),
            crate_name: "alpha_crate".into(),
            file: "alpha-package.rs".into(),
        };
        let legacy = "// @generated by `ruskel-snapshot`; do not edit.\n// snapshot-format: 1\n// package: alpha-package\n// crate: alpha_crate\n\npub mod alpha_crate {}\n";

        assert!(has_generated_header(legacy, &entry));
        assert!(!has_generated_header(
            &legacy.replace("// crate: alpha_crate", "// crate: other"),
            &entry,
        ));
    }

    #[test]
    fn update_is_bytewise_noop_and_removes_only_stale_managed_files() -> Result<()> {
        let root = tempfile::tempdir()?;
        let output = root.path().join("api");
        let first = snapshot(vec![
            crate_snapshot("alpha", "alpha", "pub fn one();"),
            crate_snapshot("beta", "beta", "pub fn two();"),
        ]);
        let report = SnapshotStore::open(&output, SnapshotMode::Update)?.sync(&first)?;
        assert_eq!(report.changes()[0].kind(), SnapshotChangeKind::Added);
        assert_eq!(report.skipped_packages(), ["tool"]);

        let marker = output.join(MARKER_FILENAME);
        let before_bytes = fs::read(&marker)?;
        let before = fs::metadata(&marker)?;
        let unchanged = SnapshotStore::open(&output, SnapshotMode::Update)?.sync(&first)?;
        assert!(unchanged.is_current());
        let after = fs::metadata(&marker)?;
        assert_eq!(fs::read(&marker)?, before_bytes);
        assert_eq!(before.modified()?, after.modified()?);
        #[cfg(unix)]
        assert_eq!(before.ino(), after.ino());

        let second = snapshot(vec![crate_snapshot(
            "alpha",
            "alpha",
            "pub fn changed(value: u8);",
        )]);
        let changed = SnapshotStore::open(&output, SnapshotMode::Update)?.sync(&second)?;
        assert!(changed.changes().iter().any(|change| {
            change.path() == Path::new("beta.rs") && change.kind() == SnapshotChangeKind::Removed
        }));
        assert!(!output.join("beta.rs").exists());
        Ok(())
    }

    #[test]
    fn check_classifies_drift_without_mutating_the_tree() -> Result<()> {
        let root = tempfile::tempdir()?;
        let output = root.path().join("api");
        let first = snapshot(vec![crate_snapshot("alpha", "alpha", "pub fn one();")]);
        SnapshotStore::open(&output, SnapshotMode::Update)?.sync(&first)?;
        fs::write(output.join("notes.txt"), b"unowned\n")?;
        let before = fs::metadata(&output)?;
        let marker = output.join(MARKER_FILENAME);
        let marker_before = fs::read(&marker)?;
        let marker_metadata_before = fs::metadata(&marker)?;

        let second = snapshot(vec![crate_snapshot("alpha", "alpha", "pub fn two();")]);
        let report = SnapshotStore::open(&output, SnapshotMode::Check)?.sync(&second)?;
        assert!(!report.is_current());
        assert!(report.changes().iter().any(|change| {
            change.path() == Path::new("alpha.rs") && change.kind() == SnapshotChangeKind::Changed
        }));
        assert!(report.changes().iter().any(|change| {
            change.path() == Path::new("notes.txt")
                && change.kind() == SnapshotChangeKind::Unexpected
        }));
        let after = fs::metadata(&output)?;
        let marker_metadata_after = fs::metadata(&marker)?;
        assert_eq!(fs::read(&marker)?, marker_before);
        assert_eq!(before.modified()?, after.modified()?);
        assert_eq!(before.len(), after.len());
        assert_eq!(
            marker_metadata_before.modified()?,
            marker_metadata_after.modified()?
        );
        assert_eq!(marker_metadata_before.len(), marker_metadata_after.len());
        #[cfg(unix)]
        {
            assert_eq!(before.ino(), after.ino());
            assert_eq!(marker_metadata_before.ino(), marker_metadata_after.ino());
        }
        assert!(SnapshotStore::open(&output, SnapshotMode::Update).is_err());
        assert_eq!(fs::read(output.join("notes.txt"))?, b"unowned\n");
        Ok(())
    }

    #[test]
    fn check_reports_missing_and_interrupted_backup_without_recovery() -> Result<()> {
        let root = tempfile::tempdir()?;
        let output = root.path().join("api");
        let captured = snapshot(vec![crate_snapshot("alpha", "alpha", "pub fn one();")]);
        SnapshotStore::open(&output, SnapshotMode::Update)?.sync(&captured)?;
        fs::remove_file(output.join("alpha.rs"))?;
        let missing = SnapshotStore::open(&output, SnapshotMode::Check)?.sync(&captured)?;
        assert!(missing.changes().iter().any(|change| {
            change.path() == Path::new("alpha.rs") && change.kind() == SnapshotChangeKind::Added
        }));

        fs::write(
            output.join("alpha.rs"),
            captured.crates()[0].contents().as_bytes(),
        )?;
        let backup = root.path().join(".api.ruskel-snapshot-backup-fixture");
        fs::rename(&output, &backup)?;
        let store = SnapshotStore::open(&output, SnapshotMode::Check)?;
        assert_eq!(store.profile(), Some(captured.profile()));
        let report = store.sync(&captured)?;
        assert!(report.changes().iter().any(|change| {
            change.path() == Path::new(".api.ruskel-snapshot-backup-fixture")
                && change.kind() == SnapshotChangeKind::Interrupted
        }));
        assert!(backup.exists());
        assert!(!output.exists());

        SnapshotStore::open(&output, SnapshotMode::Update)?;
        assert!(output.exists());
        assert!(!backup.exists());
        Ok(())
    }

    #[test]
    fn malformed_unsupported_collision_and_header_mismatch_are_rejected() -> Result<()> {
        let root = tempfile::tempdir()?;
        for (name, marker) in [
            ("malformed", b"not toml\n".as_slice()),
            (
                "unsupported",
                b"# @generated by `ruskel-snapshot`; do not edit.\nformat = 2\ntoolchain = \"nightly-2099-01-01\"\ntarget = \"x\"\ndefault_features = true\nall_features = false\nfeatures = []\n",
            ),
            (
                "collision",
                b"# @generated by `ruskel-snapshot`; do not edit.\nformat = 1\ntoolchain = \"nightly-2099-01-01\"\ntarget = \"x\"\ndefault_features = true\nall_features = false\nfeatures = []\n\n[[crates]]\npackage = \"Alpha\"\ncrate = \"alpha\"\nfile = \"Alpha.rs\"\n\n[[crates]]\npackage = \"alpha\"\ncrate = \"alpha\"\nfile = \"alpha.rs\"\n",
            ),
        ] {
            let output = root.path().join(name);
            fs::create_dir(&output)?;
            fs::write(output.join(MARKER_FILENAME), marker)?;
            assert!(SnapshotStore::open(&output, SnapshotMode::Check).is_err());
        }

        let output = root.path().join("identity");
        fs::create_dir(&output)?;
        let captured = snapshot(vec![crate_snapshot("alpha", "alpha", "pub fn one();")]);
        fs::write(output.join(MARKER_FILENAME), marker_for(&captured))?;
        fs::write(
            output.join("alpha.rs"),
            captured.crates()[0].contents().replacen(
                "// @generated by `ruskel-snapshot`; do not edit.",
                "// hand-written",
                1,
            ),
        )?;
        assert!(SnapshotStore::open(&output, SnapshotMode::Check).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn destination_owned_symlinks_and_physical_aliases_are_safe() -> Result<()> {
        let root = tempfile::tempdir()?;
        let real_parent = root.path().join("real");
        fs::create_dir(&real_parent)?;
        let alias_parent = root.path().join("alias");
        symlink(&real_parent, &alias_parent)?;
        let direct = physical_identity(&real_parent.join("api"))?;
        let alias = physical_identity(&alias_parent.join("api"))?;
        assert_eq!(direct.0, alias.0);
        assert_eq!(direct.1, alias.1);
        if Path::new("/private/tmp").is_dir() {
            let name = format!("ruskel-physical-alias-{}", process::id());
            let short = physical_identity(&Path::new("/tmp").join(&name))?;
            let physical = physical_identity(&Path::new("/private/tmp").join(name))?;
            assert_eq!(short.0, physical.0);
            assert_eq!(short.1, physical.1);
        }

        let destination_link = root.path().join("api-link");
        symlink(real_parent.join("api"), &destination_link)?;
        assert!(SnapshotStore::open(&destination_link, SnapshotMode::Check).is_err());

        let output = real_parent.join("api");
        let captured = snapshot(vec![crate_snapshot("alpha", "alpha", "pub fn one();")]);
        SnapshotStore::open(&output, SnapshotMode::Update)?.sync(&captured)?;
        fs::remove_file(output.join("alpha.rs"))?;
        symlink(root.path().join("outside"), output.join("alpha.rs"))?;
        assert!(SnapshotStore::open(&output, SnapshotMode::Check).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_lock_rejects_symlinks_and_preserves_regular_lock_identity() -> Result<()> {
        let root = tempfile::tempdir()?;
        let output = root.path().join("api");
        let lock = root.path().join(".api.ruskel-snapshot.lock");
        let captured = snapshot(vec![crate_snapshot("alpha", "alpha", "pub fn one();")]);

        let live_target = root.path().join("live-lock-target");
        fs::write(&live_target, b"outside")?;
        symlink(&live_target, &lock)?;
        for mode in [SnapshotMode::Update, SnapshotMode::Check] {
            assert!(SnapshotStore::open(&output, mode).is_err());
            assert_eq!(fs::read(&live_target)?, b"outside");
        }
        fs::remove_file(&lock)?;

        let dangling_target = root.path().join("dangling-lock-target");
        symlink(&dangling_target, &lock)?;
        for mode in [SnapshotMode::Update, SnapshotMode::Check] {
            assert!(SnapshotStore::open(&output, mode).is_err());
            assert!(!dangling_target.exists());
        }
        fs::remove_file(&lock)?;

        fs::write(&lock, b"persistent")?;
        SnapshotStore::open(&output, SnapshotMode::Update)?.sync(&captured)?;
        assert_eq!(fs::read(&lock)?, b"persistent");
        let before = fs::metadata(&lock)?;
        SnapshotStore::open(&output, SnapshotMode::Check)?.sync(&captured)?;
        let after = fs::metadata(&lock)?;
        assert_eq!(fs::read(&lock)?, b"persistent");
        assert_eq!(before.len(), after.len());
        assert_eq!(before.ino(), after.ino());

        fs::remove_file(&lock)?;
        SnapshotStore::open(&output, SnapshotMode::Check)?;
        assert!(fs::symlink_metadata(&lock)?.is_file());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_lock_rejects_a_directory() -> Result<()> {
        let root = tempfile::tempdir()?;
        let output = root.path().join("api");
        let lock = root.path().join(".api.ruskel-snapshot.lock");
        fs::create_dir(&lock)?;
        assert!(SnapshotStore::open(&output, SnapshotMode::Update).is_err());
        assert!(SnapshotStore::open(&output, SnapshotMode::Check).is_err());
        Ok(())
    }

    #[test]
    fn marker_compare_and_swap_rejects_stale_overwrite() -> Result<()> {
        let root = tempfile::tempdir()?;
        let output = root.path().join("api");
        let first = snapshot(vec![crate_snapshot("alpha", "alpha", "pub fn one();")]);
        let second = snapshot(vec![crate_snapshot("alpha", "alpha", "pub fn two();")]);
        let stale = SnapshotStore::open(&output, SnapshotMode::Update)?;
        SnapshotStore::open(&output, SnapshotMode::Update)?.sync(&second)?;
        assert!(stale.sync(&first).is_err());

        let same = SnapshotStore::open(&output, SnapshotMode::Update)?;
        SnapshotStore::open(&output, SnapshotMode::Update)?.sync(&second)?;
        assert!(same.sync(&second)?.is_current());
        Ok(())
    }

    #[test]
    fn concurrent_synchronizers_share_one_physical_lock() -> Result<()> {
        let root = tempfile::tempdir()?;
        let real_parent = root.path().join("real");
        fs::create_dir(&real_parent)?;
        let alias_parent = root.path().join("alias");
        #[cfg(unix)]
        symlink(&real_parent, &alias_parent)?;
        #[cfg(not(unix))]
        fs::create_dir(&alias_parent)?;
        let direct_path = real_parent.join("api");
        #[cfg(unix)]
        let alias_path = alias_parent.join("api");
        #[cfg(not(unix))]
        let alias_path = direct_path.clone();
        let direct = SnapshotStore::open(&direct_path, SnapshotMode::Update)?;
        let alias = SnapshotStore::open(&alias_path, SnapshotMode::Update)?;
        assert_eq!(direct.path, alias.path);
        assert_eq!(direct.lock_path, alias.lock_path);
        let captured = Arc::new(snapshot(vec![crate_snapshot(
            "alpha",
            "alpha",
            "pub fn one();",
        )]));
        let first_snapshot = Arc::clone(&captured);
        let first = thread::spawn(move || direct.sync(&first_snapshot));
        let second_snapshot = Arc::clone(&captured);
        let second = thread::spawn(move || alias.sync(&second_snapshot));
        let first_report = first.join().expect("first synchronizer")?;
        let second_report = second.join().expect("second synchronizer")?;
        assert!(first_report.is_current() || second_report.is_current());
        assert_eq!(
            fs::read(direct_path.join("alpha.rs"))?,
            captured.crates()[0].contents().as_bytes()
        );
        Ok(())
    }

    #[test]
    fn failed_second_rename_restores_old_tree_and_cleans_new_tree() -> Result<()> {
        let root = tempfile::tempdir()?;
        let output = root.path().join("api");
        let old = snapshot(vec![crate_snapshot("alpha", "alpha", "pub fn one();")]);
        SnapshotStore::open(&output, SnapshotMode::Update)?.sync(&old)?;
        let desired_snapshot = snapshot(vec![crate_snapshot("alpha", "alpha", "pub fn two();")]);
        let marker = marker_for(&desired_snapshot);
        let desired = desired_files(&desired_snapshot, &marker);
        let temporary = write_complete_tree(&output, &desired)?;
        let calls = AtomicUsize::new(0);
        let result = commit_tree_with(
            &output,
            ".api.ruskel-snapshot-backup-",
            temporary,
            |from, to| {
                if calls.fetch_add(1, Ordering::SeqCst) == 1 {
                    Err(Error::other("injected commit failure"))
                } else {
                    fs::rename(from, to)
                }
            },
        );
        assert!(result.is_err());
        assert_eq!(
            fs::read(output.join("alpha.rs"))?,
            old.crates()[0].contents().as_bytes()
        );
        assert!(find_backups(&output, ".api.ruskel-snapshot-backup-")?.is_empty());
        assert!(fs::read_dir(root.path())?.all(|entry| {
            !entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".ruskel-snapshot-new-")
        }));
        Ok(())
    }
}
