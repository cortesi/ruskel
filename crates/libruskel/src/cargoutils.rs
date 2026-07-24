use std::{
    env, fs,
    io::{self, Write},
    path::{Component, Path, PathBuf, absolute},
    result,
};

use cargo::{core::Workspace, ops, util::context::GlobalContext};
use rustdoc_json::PackageTarget;
use rustdoc_types::Crate;
use semver::Version;
use tempfile::TempDir;

use super::{
    stdlib,
    target::{Entrypoint, Target},
};
use crate::{
    cache::{BuildLease, CacheHandle},
    error::{Result, RuskelError, convert_cargo_error},
    toolchain::nightly_identity,
};

/// A path to a crate. This can be a directory on the filesystem or the virtual std library.
#[derive(Debug)]
struct CargoPath {
    /// Filesystem root for the crate (None for std library targets).
    root: Option<PathBuf>,
    /// Keeps a temporary directory alive for registry fetches.
    _temp_guard: Option<TempDir>,
    /// Cargo source variant backing this path.
    kind: CargoPathKind,
}

/// Backing source for a cargo path.
#[derive(Debug)]
enum CargoPathKind {
    /// Filesystem-backed crate directory containing a manifest.
    Filesystem,
    /// Standard library crate (actual_crate, display_crate),
    /// e.g., ("alloc", "std") when user requests std::vec
    StdLibrary {
        /// Name of the crate rustdoc should read (e.g., "alloc").
        actual: String,
        /// Crate name originally requested by the user (e.g., "std").
        display: String,
    },
}

impl CargoPath {
    /// Build a cargo path from an existing filesystem directory.
    fn from_path(path: PathBuf) -> Self {
        Self {
            root: Some(path),
            _temp_guard: None,
            kind: CargoPathKind::Filesystem,
        }
    }

    /// Build a cargo path from a temporary directory, keeping the guard alive.
    fn from_temp_dir(temp_dir: TempDir) -> Self {
        let root = temp_dir.path().to_path_buf();
        Self {
            root: Some(root),
            _temp_guard: Some(temp_dir),
            kind: CargoPathKind::Filesystem,
        }
    }

    /// Build a cargo path representing a std library crate mapping.
    fn std(actual: impl Into<String>, display: impl Into<String>) -> Self {
        Self {
            root: None,
            _temp_guard: None,
            kind: CargoPathKind::StdLibrary {
                actual: actual.into(),
                display: display.into(),
            },
        }
    }

    /// Whether this path corresponds to a std library crate.
    fn is_std_library(&self) -> bool {
        matches!(self.kind, CargoPathKind::StdLibrary { .. })
    }

    /// Return the (actual, display) crate names when this path is std.
    fn std_names(&self) -> Option<(&str, &str)> {
        match &self.kind {
            CargoPathKind::StdLibrary { actual, display } => {
                Some((actual.as_str(), display.as_str()))
            }
            _ => None,
        }
    }

    /// Canonical filesystem path for this cargo source (not available for std crates).
    fn canonical_path(&self) -> Result<PathBuf> {
        if self.is_std_library() {
            return Err(RuskelError::Generate(
                "Standard library crates don't have a filesystem path".to_string(),
            ));
        }
        let path = self.as_path()?;
        fs::canonicalize(path).map_err(|err| {
            RuskelError::Generate(format!(
                "Failed to canonicalize path '{}': {err}",
                path.display()
            ))
        })
    }

    /// Return the root directory tied to this Cargo source.
    pub fn as_path(&self) -> Result<&Path> {
        match &self.kind {
            CargoPathKind::Filesystem => self.root.as_deref().ok_or_else(|| {
                RuskelError::Generate("filesystem cargo path missing root directory".to_string())
            }),
            CargoPathKind::StdLibrary { actual, display } => Err(RuskelError::Generate(format!(
                "Standard library crate '{display}' (resolved as '{actual}') does not have a filesystem path"
            ))),
        }
    }

    /// Return the directory containing `manifest_path`, failing when no parent exists.
    fn manifest_dir_from_path(manifest_path: &Path, package_name: &str) -> Result<PathBuf> {
        manifest_path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| {
                RuskelError::Generate(format!(
                    "Package '{package_name}' manifest path '{}' has no parent directory",
                    manifest_path.display()
                ))
            })
    }

    /// Load rustdoc JSON for the crate represented by this cargo path.
    /// Read the crate data for this resolved target using rustdoc JSON generation.
    pub fn read_crate(&self, options: &CrateReadOptions) -> Result<CrateRead> {
        // Handle standard library crates specially
        if let Some((actual_crate, display_crate)) = self.std_names() {
            let display_name = if actual_crate != display_crate {
                Some(display_crate)
            } else {
                None
            };
            return Ok(CrateRead {
                crate_data: stdlib::load_json(actual_crate, display_name)?,
                bin_target: None,
            });
        }

        let manifest_path = self.manifest_path()?;
        let PackageTargetSelection {
            package_target,
            bin_target,
            workspace_root,
            package_name,
            package_version,
        } = select_package_target(
            &manifest_path,
            options.offline,
            options.bin_override.as_deref(),
        )?;
        let include_private =
            options.private_items || bin_target.as_ref().is_some_and(|target| target.is_bin_only);
        let owner = options.cache.owner()?;
        let mut storage_retry: Option<(String, String)> = None;
        let mut retry_budget = RetryBudget::default();

        while retry_budget.begin_attempt() {
            let toolchain_before = nightly_identity()
                .map_err(|error| with_recovery_context(&mut storage_retry, error))?;
            let lease = match owner.begin_build(
                &toolchain_before,
                &workspace_root,
                &package_name,
                &package_version,
            ) {
                Ok(lease) => lease,
                Err(error) if owner.is_entry_error(&error) && retry_budget.take_storage_retry() => {
                    let original = error.to_string();
                    let quarantine = match owner
                        .quarantine_workspace(&toolchain_before, &workspace_root)
                    {
                        Ok(Some(path)) => {
                            format!("moved the damaged cache entry to '{}'", path.display())
                        }
                        Ok(None) => "the damaged cache entry did not exist".to_string(),
                        Err(quarantine_error) => {
                            format!("could not move the damaged cache entry: {quarantine_error}")
                        }
                    };
                    let maintenance = match owner.recover_storage(&toolchain_before) {
                        Ok(action) => action,
                        Err(maintenance_error) => {
                            format!("synchronous maintenance failed: {maintenance_error}")
                        }
                    };
                    owner.signal_maintenance(&toolchain_before, false);
                    storage_retry = Some((original, format!("{quarantine}; {maintenance}")));
                    continue;
                }
                Err(error) => return Err(with_recovery_context(&mut storage_retry, error)),
            };
            let attempt_result = build_once(
                &manifest_path,
                package_target.clone(),
                bin_target.clone(),
                include_private,
                options,
                &lease,
            );
            let attempt_result = match attempt_result {
                Ok(crate_read) => lease
                    .touch_success()
                    .map(|()| crate_read)
                    .map_err(BuildAttemptFailure::Storage),
                error => error,
            };
            let low_space = owner.is_low_space();
            let attempt_result = match attempt_result {
                Err(BuildAttemptFailure::Diagnostic(error)) if low_space => {
                    Err(BuildAttemptFailure::Storage(error))
                }
                other => other,
            };

            match attempt_result {
                Ok(crate_read) => {
                    owner.signal_maintenance(&toolchain_before, low_space);
                    let toolchain_after = nightly_identity()
                        .map_err(|error| with_recovery_context(&mut storage_retry, error))?;
                    if toolchain_after != toolchain_before {
                        drop(lease);
                        if !retry_budget.take_toolchain_retry() {
                            return Err(RuskelError::Generate(
                                "The nightly toolchain changed repeatedly while Ruskel generated rustdoc JSON. Retry the request after the update completes."
                                    .to_string(),
                            ));
                        }
                        continue;
                    }
                    return Ok(crate_read);
                }
                Err(BuildAttemptFailure::Storage(error)) if retry_budget.take_storage_retry() => {
                    let original = error.to_string();
                    let recovery = match lease.move_to_trash() {
                        Ok(path) => {
                            format!("moved the damaged cache entry to '{}'", path.display())
                        }
                        Err(recovery_error) => {
                            format!("could not move the damaged cache entry: {recovery_error}")
                        }
                    };
                    drop(lease);
                    let maintenance = match owner.recover_storage(&toolchain_before) {
                        Ok(action) => action,
                        Err(error) => format!("synchronous maintenance failed: {error}"),
                    };
                    owner.signal_maintenance(&toolchain_before, false);
                    storage_retry = Some((original, format!("{recovery}; {maintenance}")));
                }
                Err(failure) => {
                    let retry_error = failure.into_error();
                    owner.signal_maintenance(&toolchain_before, low_space);
                    return Err(with_recovery_context(&mut storage_retry, retry_error));
                }
            }
        }

        Err(RuskelError::Generate(
            "Ruskel exhausted the rustdoc build retry budget".to_string(),
        ))
    }

    /// Compute the absolute `Cargo.toml` path for this source.
    pub fn manifest_path(&self) -> Result<PathBuf> {
        if self.is_std_library() {
            return Err(RuskelError::Generate(
                "Standard library crates don't have a manifest path".to_string(),
            ));
        }

        let manifest_path = self.as_path()?.join("Cargo.toml");
        absolute(&manifest_path).map_err(|err| {
            RuskelError::Generate(format!(
                "Failed to resolve manifest path for '{}': {err}",
                manifest_path.display()
            ))
        })
    }

    /// Return whether this cargo path includes a `Cargo.toml`.
    pub fn has_manifest(&self) -> Result<bool> {
        if self.is_std_library() {
            return Ok(false);
        }
        Ok(self.as_path()?.join("Cargo.toml").exists())
    }

    /// Identify if the path is a standalone package manifest.
    pub fn is_package(&self) -> Result<bool> {
        if self.is_std_library() {
            return Ok(false);
        }
        Ok(self.has_manifest()? && !self.is_workspace()?)
    }

    /// Identify if the path is a workspace manifest without a package section.
    pub fn is_workspace(&self) -> Result<bool> {
        if self.is_std_library() {
            return Ok(false);
        }

        if !self.has_manifest()? {
            return Ok(false);
        }
        let manifest_path = self.manifest_path()?;
        let manifest = cargo_toml::Manifest::from_path(&manifest_path)
            .map_err(|err| RuskelError::ManifestParse(err.to_string()))?;
        Ok(manifest.workspace.is_some() && manifest.package.is_none())
    }

    /// Find a dependency within the current workspace or registry cache.
    pub fn find_dependency(&self, dependency: &str, offline: bool) -> Result<Option<Self>> {
        if self.is_std_library() {
            return Ok(None);
        }

        let config = create_quiet_cargo_config(offline)?;
        let manifest_path = self.manifest_path()?;

        let workspace =
            Workspace::new(&manifest_path, &config).map_err(|err| convert_cargo_error(&err))?;

        let (_, ps) = ops::fetch(
            &workspace,
            &ops::FetchOptions {
                gctx: &config,
                targets: vec![],
            },
        )
        .map_err(|err| convert_cargo_error(&err))?;

        // Try both the provided name and its hyphenated/underscored version
        let alt_dependency = if dependency.contains('_') {
            dependency.replace('_', "-")
        } else {
            dependency.replace('-', "_")
        };

        for package in ps.packages() {
            let package_name = package.name().as_str();
            if package_name == dependency || package_name == alt_dependency {
                let manifest_dir =
                    Self::manifest_dir_from_path(package.manifest_path(), package_name)?;
                return Ok(Some(Self::from_path(manifest_dir)));
            }
        }
        Ok(None)
    }

    /// Walk upwards from `start_dir` to locate the closest `Cargo.toml`.
    pub fn nearest_manifest(start_dir: &Path) -> Option<Self> {
        let mut current_dir = start_dir.to_path_buf();

        loop {
            let manifest_path = current_dir.join("Cargo.toml");
            if manifest_path.exists() {
                return Some(Self::from_path(current_dir));
            }
            if !current_dir.pop() {
                break;
            }
        }
        None
    }

    /// Find a package in the current workspace by name.
    fn find_workspace_package(&self, module_name: &str) -> Result<Option<ResolvedTarget>> {
        let workspace_manifest_path = self.manifest_path()?;

        // Try both hyphenated and underscored versions
        let alt_name = if module_name.contains('_') {
            module_name.replace('_', "-")
        } else {
            module_name.replace('-', "_")
        };

        let config = create_quiet_cargo_config(false)?;

        let workspace = Workspace::new(&workspace_manifest_path, &config)
            .map_err(|err| convert_cargo_error(&err))?;

        for package in workspace.members() {
            let package_name = package.name().as_str();
            if package_name == module_name || package_name == alt_name {
                let package_path =
                    Self::manifest_dir_from_path(package.manifest_path(), package_name)?;
                return Ok(Some(ResolvedTarget::new(
                    Self::from_path(package_path),
                    &[],
                )));
            }
        }
        Ok(None)
    }
}

/// Create a cargo configuration with minimal output suited for library usage.
fn create_quiet_cargo_config(offline: bool) -> Result<GlobalContext> {
    let mut config = GlobalContext::default().map_err(|err| convert_cargo_error(&err))?;
    config
        .configure(
            0,     // verbose
            true,  // quiet
            None,  // color
            false, // frozen
            false, // locked
            offline,
            &None, // target_dir
            &[],   // unstable_flags
            &[],   // cli_config
        )
        .map_err(|err| convert_cargo_error(&err))?;
    Ok(config)
}

/// Determine which Cargo target should be used for rustdoc JSON generation.
fn select_package_target(
    manifest_path: &Path,
    offline: bool,
    bin_override: Option<&str>,
) -> Result<PackageTargetSelection> {
    let config = create_quiet_cargo_config(offline)?;
    let workspace =
        Workspace::new(manifest_path, &config).map_err(|err| convert_cargo_error(&err))?;
    let workspace_root =
        fs::canonicalize(workspace.root()).map_err(|source| RuskelError::CacheIo {
            action: "canonicalize Cargo workspace root",
            path: workspace.root().to_path_buf(),
            source,
        })?;
    let package = workspace
        .current()
        .map_err(|err| convert_cargo_error(&err))?;
    let package_name = package.name().to_string();
    let package_version = package.version().to_string();

    let has_lib = package.targets().iter().any(|target| target.is_lib());
    let bin_targets: Vec<_> = package
        .targets()
        .iter()
        .filter(|target| target.is_bin())
        .collect();
    let bin_names: Vec<&str> = bin_targets.iter().map(|target| target.name()).collect();

    if let Some(bin_name) = bin_override {
        if bin_names.contains(&bin_name) {
            return Ok(PackageTargetSelection {
                package_target: PackageTarget::Bin(bin_name.to_string()),
                bin_target: Some(BinaryTarget {
                    name: bin_name.to_string(),
                    is_bin_only: !has_lib,
                }),
                workspace_root,
                package_name,
                package_version,
            });
        }

        let available = if bin_names.is_empty() {
            "no binary targets found".to_string()
        } else {
            format!("available: {}", bin_names.join(", "))
        };

        return Err(RuskelError::Generate(format!(
            "error: binary target '{bin_name}' not found in package ({available})"
        )));
    }

    if has_lib {
        return Ok(PackageTargetSelection {
            package_target: PackageTarget::Lib,
            bin_target: None,
            workspace_root,
            package_name,
            package_version,
        });
    }

    if bin_names.is_empty() {
        return Err(RuskelError::Generate(
            "error: no library targets found in package".to_string(),
        ));
    }

    if let Some(default_run) = package.manifest().default_run()
        && bin_names.contains(&default_run)
    {
        return Ok(PackageTargetSelection {
            package_target: PackageTarget::Bin(default_run.to_string()),
            bin_target: Some(BinaryTarget {
                name: default_run.to_string(),
                is_bin_only: true,
            }),
            workspace_root,
            package_name,
            package_version,
        });
    }

    if bin_names.len() == 1 {
        let name = bin_names[0];
        return Ok(PackageTargetSelection {
            package_target: PackageTarget::Bin(name.to_string()),
            bin_target: Some(BinaryTarget {
                name: name.to_string(),
                is_bin_only: true,
            }),
            workspace_root,
            package_name,
            package_version,
        });
    }

    Err(RuskelError::Generate(format!(
        "error: multiple binary targets found in package ({})",
        bin_names.join(", ")
    )))
}

/// Construct a minimal manifest string for a temporary crate that depends on `dependency`.
fn generate_dummy_manifest(
    dependency: &str,
    version: Option<String>,
    features: Option<&[&str]>,
) -> String {
    // Convert underscores to hyphens for Cargo package names
    let cargo_dependency = dependency.replace('_', "-");

    let version_str = version.map_or("*".to_string(), |v| v);
    let features_str = features.map_or(String::new(), |f| {
        let feature_list = f
            .iter()
            .map(|feat| format!("\"{feat}\""))
            .collect::<Vec<_>>()
            .join(", ");
        format!(", features = [{feature_list}]")
    });
    format!(
        r#"[package]
name = "dummy-crate"
version = "0.1.0"

[dependencies]
{cargo_dependency} = {{ version = "{version_str}"{features_str} }}
"#
    )
}

/// Materialize a temporary crate on disk to fetch metadata for a dependency.
fn create_dummy_crate(
    dependency: &str,
    version: Option<String>,
    features: Option<&[&str]>,
) -> Result<CargoPath> {
    let temp_dir = TempDir::new()?;
    let path = temp_dir.path();

    let manifest_path = path.join("Cargo.toml");
    let src_dir = path.join("src");
    fs::create_dir_all(&src_dir)?;

    let lib_rs = src_dir.join("lib.rs");
    let mut file = fs::File::create(lib_rs)?;
    writeln!(file, "// Dummy crate")?;

    let manifest = generate_dummy_manifest(dependency, version, features);
    fs::write(manifest_path, manifest)?;

    Ok(CargoPath::from_temp_dir(temp_dir))
}

/// Metadata describing a selected binary target.
#[derive(Debug, Clone)]
pub struct BinaryTarget {
    /// Name of the selected binary target.
    pub(crate) name: String,
    /// Whether the package has no library target.
    pub(crate) is_bin_only: bool,
}

/// Container for rustdoc JSON and target metadata.
#[derive(Debug)]
pub struct CrateRead {
    /// Parsed rustdoc JSON for the selected target.
    pub(crate) crate_data: Crate,
    /// Binary target metadata when a bin target was selected.
    pub(crate) bin_target: Option<BinaryTarget>,
}

/// Options controlling how rustdoc JSON is generated.
#[derive(Debug, Clone)]
pub struct CrateReadOptions {
    /// Whether to disable default features.
    pub(crate) no_default_features: bool,
    /// Whether to enable all features.
    pub(crate) all_features: bool,
    /// Specific feature list to enable.
    pub(crate) features: Vec<String>,
    /// Whether to include private items in rustdoc output.
    pub(crate) private_items: bool,
    /// Whether to suppress cargo output during rustdoc generation.
    pub(crate) silent: bool,
    /// Whether to force offline mode for cargo operations.
    pub(crate) offline: bool,
    /// Optional override of the binary target name.
    pub(crate) bin_override: Option<String>,
    /// Dedicated cache handle for non-standard-library builds.
    pub(crate) cache: CacheHandle,
}

/// Internal package target selection details for rustdoc JSON.
#[derive(Debug)]
struct PackageTargetSelection {
    /// Cargo package target used for rustdoc JSON.
    package_target: PackageTarget,
    /// Binary target metadata for frontmatter output.
    bin_target: Option<BinaryTarget>,
    /// Canonical root shared by all members of the selected workspace.
    workspace_root: PathBuf,
    /// Selected package name for cache display metadata.
    package_name: String,
    /// Selected package version for cache display metadata.
    package_version: String,
}

/// A resolved Rust package or module target.
#[derive(Debug)]
pub struct ResolvedTarget {
    /// Package directory path (filesystem or temporary).
    package_path: CargoPath,

    /// Module path within the package, excluding the package name. E.g.,
    /// "module::submodule::item". Empty string for package root. This might not necessarily match
    /// the user's input.
    pub filter: String,
}

impl ResolvedTarget {
    /// Build a `ResolvedTarget` with a normalised module filter path.
    fn new(path: CargoPath, components: &[String]) -> Self {
        let filter = if components.is_empty() {
            String::new()
        } else {
            let mut normalized_components = components.to_vec();
            normalized_components[0] = to_import_name(&normalized_components[0]);
            normalized_components.join("::")
        };

        Self {
            package_path: path,
            filter,
        }
    }

    /// Read the crate data for this resolved target using rustdoc JSON generation.
    pub fn read_crate(&self, options: &CrateReadOptions) -> Result<CrateRead> {
        self.package_path.read_crate(options)
    }

    /// Resolve a standard library crate name, optionally overriding the display name.
    fn resolve_std_crate(name: &str, display_name: Option<&str>, path: &[String]) -> Option<Self> {
        stdlib::is_crate(name).then(|| {
            let display = display_name.unwrap_or(name);
            Self::new(CargoPath::std(name.to_string(), display.to_string()), path)
        })
    }

    /// Reject bare standard library module names that require an explicit `std::` prefix.
    fn reject_std_module_name(name: &str) -> Result<()> {
        if stdlib::is_module(name) {
            return Err(RuskelError::InvalidTarget(format!(
                "'{name}' appears to be a standard library module. Use the full path like 'std::{name}'"
            )));
        }

        Ok(())
    }

    /// Resolve a `Target` into a fully-qualified location and filter path.
    pub fn from_target(target: Target, offline: bool) -> Result<Self> {
        match target.entrypoint {
            Entrypoint::Path(path) => Self::from_path_entry(path, &target.path),
            Entrypoint::Name { name, version } => {
                Self::from_named_entry(&name, version, &target.path, offline)
            }
        }
    }

    /// Resolve a filesystem entrypoint to a package or workspace member target.
    fn from_path_entry(path: PathBuf, target_path: &[String]) -> Result<Self> {
        if path.is_file() && path.extension().is_some_and(|ext| ext == "rs") {
            return Self::from_rust_file(path, target_path);
        }

        let cargo_path = CargoPath::from_path(path);
        let cargo_path = CargoPath::from_path(cargo_path.canonical_path()?);
        if cargo_path.is_package()? {
            return Ok(Self::new(cargo_path, target_path));
        }
        if cargo_path.is_workspace()? {
            return Self::from_workspace_path(&cargo_path, target_path);
        }

        Err(RuskelError::InvalidTarget(format!(
            "Path '{}' is neither a package nor a workspace",
            cargo_path.as_path()?.display()
        )))
    }

    /// Resolve a workspace root plus package path to a concrete package target.
    fn from_workspace_path(cargo_path: &CargoPath, target_path: &[String]) -> Result<Self> {
        let Some(package_name) = target_path.first() else {
            return Err(RuskelError::InvalidTarget(
                "No package specified in workspace".to_string(),
            ));
        };

        if let Some(package) = cargo_path.find_workspace_package(package_name)? {
            return Ok(Self::new(package.package_path, &target_path[1..]));
        }

        Err(RuskelError::ModuleNotFound(format!(
            "Package '{package_name}' not found in workspace"
        )))
    }

    /// Resolve a named entrypoint against std, workspace, dependencies, or crates.io.
    fn from_named_entry(
        name: &str,
        version: Option<Version>,
        target_path: &[String],
        offline: bool,
    ) -> Result<Self> {
        if let Some(std_target) = Self::resolve_std_crate(name, None, target_path) {
            return Ok(std_target);
        }
        Self::reject_std_module_name(name)?;

        let current_dir = env::current_dir()?;
        match CargoPath::nearest_manifest(&current_dir) {
            Some(root) => Self::from_manifest_root(&root, name, version, target_path, offline),
            None => Self::from_dummy_crate(name, version, target_path, offline),
        }
    }

    /// Resolve a named target using the nearest manifest as the root context.
    fn from_manifest_root(
        root: &CargoPath,
        name: &str,
        version: Option<Version>,
        target_path: &[String],
        offline: bool,
    ) -> Result<Self> {
        if let Some(workspace_member) = root.find_workspace_package(name)? {
            let Self { package_path, .. } = workspace_member;
            return Ok(Self::new(package_path, target_path));
        }

        if let Some(dependency) = root.find_dependency(name, offline)? {
            return Ok(Self::new(dependency, target_path));
        }

        Self::from_dummy_crate(name, version, target_path, offline)
    }

    /// Retarget a resolved crate path when the first filter component names a dependency.
    fn retarget_dependency(self, original_path: &[String], offline: bool) -> Result<Self> {
        let Some(first_component) = self
            .filter
            .split("::")
            .next()
            .filter(|component| !component.is_empty())
        else {
            return Ok(self);
        };

        if let Some(package_path) = self
            .package_path
            .find_dependency(first_component, offline)?
        {
            return Ok(Self::new(package_path, original_path));
        }

        Ok(self)
    }

    /// Resolve a module path starting from a specific Rust source file.
    fn from_rust_file(file_path: PathBuf, additional_path: &[String]) -> Result<Self> {
        let file_path = fs::canonicalize(file_path)?;
        let mut current_dir = file_path
            .parent()
            .ok_or_else(|| RuskelError::InvalidTarget("Invalid file path".to_string()))?
            .to_path_buf();

        // Find the nearest Cargo.toml
        while !current_dir.join("Cargo.toml").exists() {
            if !current_dir.pop() {
                return Err(RuskelError::ManifestNotFound);
            }
        }

        let cargo_path = CargoPath::from_path(current_dir.clone());
        let relative_path = file_path.strip_prefix(&current_dir).map_err(|_| {
            RuskelError::InvalidTarget("Failed to determine relative path".to_string())
        })?;

        // Convert the relative path to a module path
        let mut components: Vec<_> = relative_path
            .components()
            .filter_map(|c| {
                if let Component::Normal(os_str) = c {
                    os_str.to_str().map(String::from)
                } else {
                    None
                }
            })
            .collect();

        // Remove "src" if it's the first component
        if components.first().is_some_and(|c| c == "src") {
            components.remove(0);
        }

        // Remove the last component (file name) and add it back without the extension
        if let Some(file_name) = components.pop()
            && let Some(stem) = Path::new(&file_name).file_stem().and_then(|s| s.to_str())
        {
            components.push(stem.to_string());
        }

        // Combine the module path with the additional path
        components.extend_from_slice(additional_path);

        Ok(Self::new(cargo_path, &components))
    }

    /// Create a resolved target backed by a temporary crate for registry dependencies.
    fn from_dummy_crate(
        name: &str,
        version: Option<Version>,
        path: &[String],
        offline: bool,
    ) -> Result<Self> {
        let version_str = version.map(|v| v.to_string());
        let dummy = create_dummy_crate(name, version_str, None)?;

        match dummy.find_dependency(name, offline) {
            Ok(Some(dependency_path)) => Ok(Self::new(dependency_path, path)),
            Ok(None) => Err(RuskelError::ModuleNotFound(format!(
                "Dependency '{name}' not found in dummy crate"
            ))),
            Err(err) => {
                if offline {
                    match err {
                        RuskelError::DependencyNotFound => Err(RuskelError::Generate(format!(
                            "crate '{name}' is not cached locally for offline use. Run 'cargo fetch {name}' without --offline first or retry without --offline."
                        ))),
                        RuskelError::Cargo(message)
                            if message.contains("--offline")
                                || message.contains("offline mode") =>
                        {
                            Err(RuskelError::Generate(format!(
                                "crate '{name}' is unavailable in offline mode: {message}"
                            )))
                        }
                        other => Err(other),
                    }
                } else {
                    Err(err)
                }
            }
        }
    }
}

/// Resovles a target specification and returns a ResolvedTarget, pointing to the package
/// directory. If necessary, construct temporary dummy crate to download packages from cargo.io.
/// Parse a textual target specification into a `ResolvedTarget`.
pub fn resolve_target(target_str: &str, offline: bool) -> Result<ResolvedTarget> {
    let (resolved_target_str, original_crate) =
        if let Some(mapped) = stdlib::resolve_reexport(target_str) {
            let original = target_str.split("::").next().unwrap_or("std");
            (mapped, Some(original.to_string()))
        } else {
            (target_str.to_string(), None)
        };

    let target = Target::parse(&resolved_target_str)?;

    match &target.entrypoint {
        Entrypoint::Path(_) => ResolvedTarget::from_target(target, offline),
        Entrypoint::Name { name, version } => {
            if version.is_some() {
                return ResolvedTarget::from_dummy_crate(
                    name,
                    version.clone(),
                    &target.path,
                    offline,
                );
            }

            if let Some(std_target) =
                ResolvedTarget::resolve_std_crate(name, original_crate.as_deref(), &target.path)
            {
                return Ok(std_target);
            }
            ResolvedTarget::reject_std_module_name(name)?;

            ResolvedTarget::from_target(target.clone(), offline)?
                .retarget_dependency(&target.path, offline)
        }
    }
}

/// Convert a package name into its canonical import form by replacing hyphens.
fn to_import_name(package_name: &str) -> String {
    package_name.replace('-', "_")
}

/// Failure category retained until the build path makes its retry decision.
#[derive(Debug)]
enum BuildAttemptFailure {
    /// Probable cache storage damage that permits one cold retry.
    Storage(RuskelError),
    /// Compiler or rustdoc diagnostics that low space can reclassify.
    Diagnostic(RuskelError),
    /// A compatibility, manifest, metadata, or tool invocation failure.
    Final(RuskelError),
}

/// Independent retry reasons within the shared three-attempt limit.
#[derive(Debug, Default)]
struct RetryBudget {
    /// Attempts already started.
    attempts: u8,
    /// Whether the storage retry was consumed.
    storage_used: bool,
    /// Whether the toolchain-change retry was consumed.
    toolchain_used: bool,
}

impl RetryBudget {
    /// Start an attempt when the total limit has not been reached.
    fn begin_attempt(&mut self) -> bool {
        if self.attempts >= 3 {
            return false;
        }
        self.attempts += 1;
        true
    }

    /// Consume the one storage retry when another attempt remains.
    fn take_storage_retry(&mut self) -> bool {
        if self.storage_used || self.attempts >= 3 {
            return false;
        }
        self.storage_used = true;
        true
    }

    /// Consume the one toolchain-change retry when another attempt remains.
    fn take_toolchain_retry(&mut self) -> bool {
        if self.toolchain_used || self.attempts >= 3 {
            return false;
        }
        self.toolchain_used = true;
        true
    }
}

/// Attach the original storage failure and recovery action to a retry failure.
fn with_recovery_context(
    context: &mut Option<(String, String)>,
    retry_error: RuskelError,
) -> RuskelError {
    let Some((original, recovery)) = context.take() else {
        return retry_error;
    };
    RuskelError::Generate(format!(
        "Rustdoc cache recovery failed.\nOriginal failure: {original}\nRecovery: {recovery}\nRetry failure: {retry_error}"
    ))
}

impl BuildAttemptFailure {
    /// Discard the internal retry category after the retry decision.
    fn into_error(self) -> RuskelError {
        match self {
            Self::Storage(error) | Self::Diagnostic(error) | Self::Final(error) => error,
        }
    }
}

/// Run one rustdoc build and read its JSON from a leased cache entry.
fn build_once(
    manifest_path: &Path,
    package_target: PackageTarget,
    bin_target: Option<BinaryTarget>,
    include_private: bool,
    options: &CrateReadOptions,
    lease: &BuildLease,
) -> result::Result<CrateRead, BuildAttemptFailure> {
    let mut captured_stdout = Vec::new();
    let mut captured_stderr = Vec::new();
    let build_dir = lease.build_dir();

    let build_result = rustdoc_json::Builder::default()
        .toolchain("nightly")
        .manifest_path(manifest_path)
        .package_target(package_target)
        .document_private_items(include_private)
        .no_default_features(options.no_default_features)
        .all_features(options.all_features)
        .features(&options.features)
        .target_dir(build_dir)
        .env("CARGO_BUILD_BUILD_DIR", build_dir)
        .quiet(options.silent)
        .silent(false)
        .build_with_captured_output(&mut captured_stdout, &mut captured_stderr);

    if !options.silent {
        if !captured_stdout.is_empty() && io::stdout().write_all(&captured_stdout).is_err() {
            // Output mirroring is best effort.
        }
        if !captured_stderr.is_empty() && io::stderr().write_all(&captured_stderr).is_err() {
            // Output mirroring is best effort.
        }
    }

    let json_path = match build_result {
        Ok(path) => path,
        Err(rustdoc_json::BuildError::IoError(source)) => {
            return Err(BuildAttemptFailure::Storage(RuskelError::CacheIo {
                action: "generate rustdoc JSON",
                path: build_dir.to_path_buf(),
                source,
            }));
        }
        Err(error) => {
            let mapped = map_rustdoc_build_error(&error, &captured_stderr, options.silent);
            if matches!(error, rustdoc_json::BuildError::BuildRustdocJsonError) {
                return Err(BuildAttemptFailure::Diagnostic(mapped));
            }
            return Err(BuildAttemptFailure::Final(mapped));
        }
    };

    let json_content = fs::read_to_string(&json_path).map_err(|source| {
        BuildAttemptFailure::Storage(RuskelError::CacheIo {
            action: "read generated rustdoc JSON",
            path: json_path.clone(),
            source,
        })
    })?;
    let crate_data: Crate = serde_json::from_str(&json_content).map_err(|error| {
        use serde_json::error::Category;

        match error.classify() {
            Category::Syntax | Category::Eof | Category::Io => {
                BuildAttemptFailure::Storage(RuskelError::CacheLayout {
                    path: json_path.clone(),
                    message: format!("generated rustdoc JSON is incomplete or invalid: {error}"),
                })
            }
            Category::Data => BuildAttemptFailure::Final(RuskelError::Generate(format!(
                "Failed to parse rustdoc JSON, which may indicate an outdated nightly toolchain. Run: rustup update nightly\nError: {error}"
            ))),
        }
    })?;

    Ok(CrateRead {
        crate_data,
        bin_target,
    })
}

/// Maximum number of characters from rustdoc stderr included in failure reports.
const MAX_STDERR_CHARS: usize = 8_192;

/// Translate a `rustdoc_json` build failure into a user-facing [`RuskelError`].
fn map_rustdoc_build_error(
    err: &rustdoc_json::BuildError,
    captured_stderr: &[u8],
    silent: bool,
) -> RuskelError {
    match err {
        rustdoc_json::BuildError::BuildRustdocJsonError => {
            format_rustdoc_failure(captured_stderr, silent)
        }
        other => {
            let err_msg = other.to_string();

            if err_msg.contains("no library targets found in package") {
                return RuskelError::Generate(
                    "error: no library targets found in package".to_string(),
                );
            }

            if err_msg.contains("toolchain") && err_msg.contains("is not installed") {
                return RuskelError::Generate(
                    "ruskel requires the nightly toolchain to be installed - run 'rustup toolchain install nightly'"
                        .to_string(),
                );
            }

            if err_msg.contains("Failed to build rustdoc JSON") {
                return format_rustdoc_failure(captured_stderr, silent);
            }

            RuskelError::Generate(format!("Failed to build rustdoc JSON: {err_msg}"))
        }
    }
}

/// Format a detailed error for rustdoc build failures, optionally embedding diagnostics.
fn format_rustdoc_failure(captured_stderr: &[u8], silent: bool) -> RuskelError {
    let stderr_raw = String::from_utf8_lossy(captured_stderr).into_owned();
    let stderr_trimmed = stderr_raw.trim();
    let summary = extract_primary_diagnostic(stderr_trimmed).unwrap_or_else(|| {
        "rustdoc exited with an error; rerun with --verbose for full diagnostics.".to_string()
    });
    let summary = summary.trim();

    if silent {
        if stderr_trimmed.is_empty() {
            return RuskelError::Generate(
                "Failed to build rustdoc JSON: rustdoc exited with an error but emitted no diagnostics. \
                 Re-run with --verbose or `cargo rustdoc` to inspect the failure.".to_string(),
            );
        }

        let (diagnostics, truncated) = truncate_diagnostics(stderr_trimmed);
        let mut message = format!("Failed to build rustdoc JSON: {summary}");
        message.push_str("\n\nrustdoc stderr:\n");
        message.push_str(&diagnostics);
        if truncated {
            message.push_str("\n… output truncated …");
        }
        return RuskelError::Generate(message);
    }

    RuskelError::Generate(format!("Failed to build rustdoc JSON: {summary}"))
}

/// Extract the first meaningful rustdoc diagnostic from the captured stderr stream.
fn extract_primary_diagnostic(stderr: &str) -> Option<String> {
    let mut lines = stderr.lines().peekable();

    while let Some(line) = lines.next() {
        if !is_primary_error_line(line) {
            continue;
        }

        let mut snippet = vec![line.trim_end().to_string()];

        while let Some(peek) = lines.peek() {
            let trimmed = peek.trim_end();
            if trimmed.is_empty() {
                lines.next();
                break;
            }

            let trimmed_start = trimmed.trim_start_matches(' ');
            let is_line_number_block = trimmed.contains('|')
                && trimmed
                    .split_once('|')
                    .map(|(prefix, _)| prefix.trim().chars().all(|c| c.is_ascii_digit()))
                    .unwrap_or(false);

            let is_context_line = peek.starts_with(' ')
                || peek.starts_with('\t')
                || peek.starts_with('|')
                || trimmed_start.starts_with("-->")
                || trimmed_start.starts_with("note:")
                || trimmed_start.starts_with("help:")
                || trimmed_start.starts_with("warning:")
                || trimmed_start.starts_with("= note:")
                || trimmed_start.starts_with("= help:")
                || trimmed_start.starts_with("= warning:")
                || is_line_number_block;

            if !is_context_line {
                break;
            }

            if let Some(next_line) = lines.next() {
                snippet.push(next_line.trim_end().to_string());
            } else {
                break;
            }
        }

        return Some(snippet.join("\n"));
    }

    None
}

/// Determine whether a line introduces a new primary rustdoc error diagnostic.
fn is_primary_error_line(line: &str) -> bool {
    let trimmed = line.trim();

    if let Some(body) = trimmed.strip_prefix("error[") {
        return body.contains(']');
    }

    if let Some(body) = trimmed.strip_prefix("error:") {
        let body = body.trim_start();
        return !(body.starts_with("Compilation failed")
            || body.starts_with("could not compile")
            || body.starts_with("could not document"));
    }

    false
}

/// Truncate collected diagnostics to a manageable size, returning whether truncation occurred.
fn truncate_diagnostics(stderr: &str) -> (String, bool) {
    let mut buffer = String::new();
    let mut truncated = false;

    for (idx, ch) in stderr.chars().enumerate() {
        if idx >= MAX_STDERR_CHARS {
            truncated = true;
            break;
        }
        buffer.push(ch);
    }

    (buffer, truncated)
}

#[cfg(test)]
mod tests {
    use std::{
        env,
        ffi::OsString,
        fs,
        path::{Path, PathBuf},
        sync::{Mutex, MutexGuard},
    };

    use once_cell::sync::Lazy;
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn retry_budget_keeps_reasons_independent_with_three_attempts_total() {
        let mut budget = RetryBudget::default();

        assert!(budget.begin_attempt());
        assert!(budget.take_storage_retry());
        assert!(!budget.take_storage_retry());

        assert!(budget.begin_attempt());
        assert!(budget.take_toolchain_retry());
        assert!(!budget.take_toolchain_retry());

        assert!(budget.begin_attempt());
        assert!(!budget.take_storage_retry());
        assert!(!budget.take_toolchain_retry());
        assert!(!budget.begin_attempt());
    }

    #[test]
    fn primary_diagnostic_extracts_compiler_error() -> Result<()> {
        let stderr = r#"
error: expected pattern, found `=`
 --> src/lib.rs:3:9
  |
3 |     let = left + right;
  |         ^ expected pattern

error: Compilation failed, aborting rustdoc
"#;

        let diagnostic = extract_primary_diagnostic(stderr).ok_or_else(|| {
            RuskelError::Generate("failed to find primary diagnostic".to_string())
        })?;
        assert!(diagnostic.contains("expected pattern"));
        assert!(diagnostic.contains("src/lib.rs:3:9"));
        assert!(!diagnostic.contains("Compilation failed"));

        Ok(())
    }

    #[test]
    fn format_rustdoc_failure_includes_diagnostics_when_silent() {
        let stderr = b"error: expected pattern, found `=`\n --> src/lib.rs:3:9\n  |\n3 |     let = left + right;\n  |         ^ expected pattern\n";
        let message = format_rustdoc_failure(stderr, true).to_string();

        assert!(message.contains("Failed to build rustdoc JSON"));
        assert!(message.contains("expected pattern"));
        assert!(message.contains("src/lib.rs:3:9"));
        assert!(message.contains("rustdoc stderr"));
    }

    struct DirGuard {
        original: PathBuf,
    }

    impl DirGuard {
        fn change_to(path: &Path) -> Result<Self> {
            let original = env::current_dir()?;
            env::set_current_dir(path)?;
            Ok(Self { original })
        }
    }

    impl Drop for DirGuard {
        fn drop(&mut self) {
            if let Err(err) = env::set_current_dir(&self.original) {
                panic!("failed to restore current directory: {err}");
            }
        }
    }

    static ENV_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    struct EnvVarGuard {
        key: &'static str,
        original: Option<OsString>,
        _guard: MutexGuard<'static, ()>,
    }

    impl EnvVarGuard {
        fn set_path(key: &'static str, value: &Path) -> Self {
            let guard = ENV_LOCK.lock().expect("env mutex poisoned");
            let original = env::var_os(key);
            // SAFETY: the mutex ensures exclusive access while we mutate process environment.
            unsafe { env::set_var(key, value) };
            Self {
                key,
                original,
                _guard: guard,
            }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: still holding the mutex guard so this mutation is synchronized.
            match &self.original {
                Some(value) => unsafe { env::set_var(self.key, value) },
                None => unsafe { env::remove_var(self.key) },
            }
        }
    }

    #[test]
    fn test_to_import_name() {
        assert_eq!(to_import_name("serde"), "serde");
        assert_eq!(to_import_name("serde-json"), "serde_json");
        assert_eq!(to_import_name("tokio-util"), "tokio_util");
        assert_eq!(
            to_import_name("my-hyphenated-package"),
            "my_hyphenated_package"
        );
    }

    #[test]
    fn test_generate_dummy_manifest() {
        // Test without features
        let manifest = generate_dummy_manifest("serde", None, None);
        assert!(manifest.contains("serde = { version = \"*\" }"));
        assert!(!manifest.contains("features"));

        // Test with single feature
        let manifest = generate_dummy_manifest("tokio", Some("1.0".to_string()), Some(&["rt"]));
        assert!(manifest.contains("tokio = { version = \"1.0\", features = [\"rt\"] }"));

        // Test with multiple features
        let manifest = generate_dummy_manifest("tokio", None, Some(&["rt", "macros", "test-util"]));
        assert!(manifest.contains(
            "tokio = { version = \"*\", features = [\"rt\", \"macros\", \"test-util\"] }"
        ));

        // Validate TOML syntax by parsing
        let manifest = generate_dummy_manifest("serde", None, Some(&["derive", "std"]));
        // Just verify the manifest contains the expected strings, since we don't have toml crate in tests
        assert!(manifest.contains("[dependencies]"));
        assert!(manifest.contains("serde = { version = \"*\", features = [\"derive\", \"std\"] }"));
    }

    #[test]
    fn test_generate_dummy_manifest_with_underscores() {
        // Test underscore to hyphen conversion
        let manifest = generate_dummy_manifest("serde_json", None, None);
        assert!(manifest.contains("serde-json = { version = \"*\" }"));
        assert!(!manifest.contains("serde_json"));

        // Test with already hyphenated names (should remain unchanged)
        let manifest = generate_dummy_manifest("async-trait", None, None);
        assert!(manifest.contains("async-trait = { version = \"*\" }"));

        // Test complex name with multiple underscores
        let manifest =
            generate_dummy_manifest("my_complex_crate_name", Some("0.1.0".to_string()), None);
        assert!(manifest.contains("my-complex-crate-name = { version = \"0.1.0\" }"));
    }

    #[test]
    fn test_create_dummy_crate() -> Result<()> {
        let cargo_path = create_dummy_crate("serde", None, None)?;
        let path = cargo_path.as_path()?;

        assert!(path.join("Cargo.toml").exists());

        let manifest_content = fs::read_to_string(path.join("Cargo.toml"))?;
        assert!(manifest_content.contains("[dependencies]"));
        assert!(manifest_content.contains("serde = { version = \"*\""));

        Ok(())
    }

    #[test]
    fn test_create_dummy_crate_with_features() -> Result<()> {
        let cargo_path = create_dummy_crate("serde", Some("1.0".to_string()), Some(&["derive"]))?;
        let path = cargo_path.as_path()?;

        assert!(path.join("Cargo.toml").exists());

        let manifest_content = fs::read_to_string(path.join("Cargo.toml"))?;

        // Validate that the manifest contains the expected content
        assert!(manifest_content.contains("[dependencies]"));
        assert!(
            manifest_content.contains("serde = { version = \"1.0\", features = [\"derive\"] }")
        );

        Ok(())
    }

    #[test]
    fn test_is_workspace() -> Result<()> {
        let temp_dir = tempdir()?;
        let cargo_path = CargoPath::from_path(temp_dir.path().to_path_buf());

        // Create a workspace Cargo.toml
        let manifest = r#"
            [workspace]
            members = ["member1", "member2"]
        "#;
        let manifest_path = cargo_path.manifest_path()?;
        fs::write(&manifest_path, manifest)?;
        assert!(cargo_path.is_workspace()?);

        // Create a regular Cargo.toml
        fs::write(
            &manifest_path,
            r#"
[package]
name = "test-crate"
version = "0.1.0"
"#,
        )?;
        assert!(!cargo_path.is_workspace()?);

        Ok(())
    }

    #[test]
    fn test_find_workspace_package() -> Result<()> {
        let temp_dir = tempdir()?;

        // Create a workspace Cargo.toml
        let manifest = r#"
            [workspace]
            members = ["member1", "member2"]
        "#;
        fs::write(temp_dir.path().join("Cargo.toml"), manifest)?;

        // Create the "member1" package
        let member1_dir = temp_dir.path().join("member1");
        fs::create_dir(&member1_dir)?;
        fs::create_dir(member1_dir.join("src"))?;
        let member1_manifest = r#"
            [package]
            name = "member1"
            version = "0.1.0"

            [features]
            default = []
            feature1 = []
        "#;
        fs::write(member1_dir.join("Cargo.toml"), member1_manifest)?;
        fs::write(member1_dir.join("src").join("lib.rs"), "// member1 lib.rs")?;

        // Create the "member2" package
        let member2_dir = temp_dir.path().join("member2");
        fs::create_dir(&member2_dir)?;
        fs::create_dir(member2_dir.join("src"))?;
        let member2_manifest = r#"
            [package]
            name = "member2"
            version = "0.2.0"
        "#;
        fs::write(member2_dir.join("Cargo.toml"), member2_manifest)?;
        fs::write(member2_dir.join("src").join("lib.rs"), "// member2 lib.rs")?;

        let cargo_path = CargoPath::from_path(temp_dir.path().to_path_buf());

        // Test finding a package in the workspace
        if let Some(resolved) = cargo_path.find_workspace_package("member1")? {
            assert_eq!(resolved.package_path.as_path()?, member1_dir);
            assert_eq!(resolved.filter, "");
        } else {
            panic!("Failed to find package in the workspace");
        }

        // Test finding another package in the workspace
        if let Some(resolved) = cargo_path.find_workspace_package("member2")? {
            assert_eq!(resolved.package_path.as_path()?, member2_dir);
            assert_eq!(resolved.filter, "");
        } else {
            panic!("Failed to find package in the workspace");
        }

        // Test not finding a package in the workspace
        assert!(
            cargo_path
                .find_workspace_package("non-existent-package")?
                .is_none()
        );

        Ok(())
    }

    #[test]
    fn test_resolve_name_prefers_workspace_members() -> Result<()> {
        let temp_dir = tempdir()?;
        let workspace_root = temp_dir.path().join("workspace");
        let localcrate_dir = workspace_root.join("localcrate");

        fs::create_dir_all(localcrate_dir.join("src"))?;
        fs::write(
            workspace_root.join("Cargo.toml"),
            r#"
            [workspace]
            members = ["localcrate"]
            "#,
        )?;
        fs::write(
            localcrate_dir.join("Cargo.toml"),
            r#"
            [package]
            name = "localcrate"
            version = "0.1.0"
            "#,
        )?;
        fs::write(localcrate_dir.join("src/lib.rs"), "// localcrate lib")?;

        let _guard = DirGuard::change_to(&workspace_root)?;
        let resolved = resolve_target("localcrate", true)?;

        let ResolvedTarget {
            package_path,
            filter,
        } = resolved;
        let path = package_path.canonical_path()?;
        let expected = fs::canonicalize(&localcrate_dir)?;

        assert_eq!(path, expected);
        assert!(filter.is_empty());

        Ok(())
    }

    #[test]
    fn test_offline_dummy_crate_error_message() -> Result<()> {
        let temp_dir = tempdir()?;
        let _cargo_home_guard = EnvVarGuard::set_path("CARGO_HOME", temp_dir.path());
        let path: Vec<String> = Vec::new();

        match ResolvedTarget::from_dummy_crate("serde", None, &path, true) {
            Err(err) => {
                let message = err.to_string();
                assert!(
                    message.contains("not cached locally for offline use"),
                    "{message}"
                );
            }
            Ok(_) => panic!("Expected offline resolution to fail"),
        }

        Ok(())
    }

    fn setup_test_structure() -> TempDir {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();

        // Create workspace structure
        fs::create_dir_all(root.join("workspace/pkg1/src")).unwrap();
        fs::create_dir_all(root.join("workspace/pkg2/src")).unwrap();
        fs::write(
            root.join("workspace/Cargo.toml"),
            r#"
            [workspace]
            members = ["pkg1", "pkg2"]
            "#,
        )
        .unwrap();

        // Create pkg1
        fs::write(
            root.join("workspace/pkg1/Cargo.toml"),
            r#"
            [package]
            name = "pkg1"
            version = "0.1.0"
            "#,
        )
        .unwrap();
        fs::write(root.join("workspace/pkg1/src/lib.rs"), "// pkg1 lib").unwrap();
        fs::write(root.join("workspace/pkg1/src/module.rs"), "// pkg1 module").unwrap();

        // Create pkg2
        fs::write(
            root.join("workspace/pkg2/Cargo.toml"),
            r#"
            [package]
            name = "pkg2"
            version = "0.1.0"
            [dependencies]
            "#,
        )
        .unwrap();
        fs::write(root.join("workspace/pkg2/src/lib.rs"), "// pkg2 lib").unwrap();

        // Create standalone package
        fs::create_dir_all(root.join("standalone/src")).unwrap();
        fs::write(
            root.join("standalone/Cargo.toml"),
            r#"
            [package]
            name = "standalone"
            version = "0.1.0"
            "#,
        )
        .unwrap();
        fs::write(root.join("standalone/src/lib.rs"), "// standalone lib").unwrap();
        fs::write(
            root.join("standalone/src/module.rs"),
            "// standalone module",
        )
        .unwrap();

        temp_dir
    }

    enum ExpectedResult {
        Path(PathBuf),
    }

    #[test]
    fn test_is_std_library_crate() {
        assert!(stdlib::is_crate("std"));
        assert!(stdlib::is_crate("core"));
        assert!(stdlib::is_crate("alloc"));
        assert!(stdlib::is_crate("proc_macro"));
        assert!(stdlib::is_crate("test"));

        assert!(!stdlib::is_crate("serde"));
        assert!(!stdlib::is_crate("tokio"));
        assert!(!stdlib::is_crate("random_crate"));
    }

    #[test]
    fn test_resolve_std_reexport() {
        // Test alloc re-exports
        assert_eq!(
            stdlib::resolve_reexport("std::rc"),
            Some("alloc::rc".to_string())
        );
        assert_eq!(
            stdlib::resolve_reexport("std::rc::Rc"),
            Some("alloc::rc::Rc".to_string())
        );
        assert_eq!(
            stdlib::resolve_reexport("std::vec::Vec"),
            Some("alloc::vec::Vec".to_string())
        );
        assert_eq!(
            stdlib::resolve_reexport("std::collections::HashMap"),
            Some("alloc::collections::HashMap".to_string())
        );

        // Test core re-exports
        assert_eq!(
            stdlib::resolve_reexport("std::mem"),
            Some("core::mem".to_string())
        );
        assert_eq!(
            stdlib::resolve_reexport("std::mem::size_of"),
            Some("core::mem::size_of".to_string())
        );
        assert_eq!(
            stdlib::resolve_reexport("std::option::Option"),
            Some("core::option::Option".to_string())
        );

        // Test non-reexports
        assert_eq!(stdlib::resolve_reexport("std::fs"), None);
        assert_eq!(stdlib::resolve_reexport("std::io"), None);
        assert_eq!(stdlib::resolve_reexport("std::net"), None);
        assert_eq!(stdlib::resolve_reexport("alloc::rc"), None);
        assert_eq!(stdlib::resolve_reexport("core::mem"), None);
        assert_eq!(stdlib::resolve_reexport("serde::Deserialize"), None);
    }

    #[test]
    fn test_is_std_library_module() {
        // Common std library modules should be detected
        assert!(stdlib::is_module("rc"));
        assert!(stdlib::is_module("vec"));
        assert!(stdlib::is_module("collections"));
        assert!(stdlib::is_module("sync"));
        assert!(stdlib::is_module("io"));
        assert!(stdlib::is_module("mem"));
        assert!(stdlib::is_module("ptr"));

        // Regular crate names should not be detected
        assert!(!stdlib::is_module("serde"));
        assert!(!stdlib::is_module("tokio"));
        assert!(!stdlib::is_module("reqwest"));
    }

    /// Assert that resolving `target` yields the expected std crate mapping.
    fn assert_std_target(
        target: &str,
        expected_actual: &str,
        expected_display: &str,
        expected_filter: &str,
    ) {
        let result = resolve_target(target, true).unwrap();
        match result.package_path.std_names() {
            Some((actual, display)) => {
                assert_eq!(actual, expected_actual);
                assert_eq!(display, expected_display);
            }
            None => panic!("Expected StdLibrary variant for {target}"),
        }
        assert_eq!(result.filter, expected_filter);
    }

    /// Assert that resolving a bare module fails with the expected error message.
    fn assert_std_module_error(module: &str, suggestion: &str) {
        match resolve_target(module, true) {
            Err(err) => {
                let message = err.to_string();
                assert!(message.contains("appears to be a standard library module"));
                assert!(message.contains(suggestion));
            }
            Ok(_) => panic!(
                "'{module}' should have failed with an error about being a std library module"
            ),
        }
    }

    #[test]
    fn test_std_library_resolve() {
        assert_std_target("std", "std", "std", "");
        assert_std_target("std::vec::Vec", "alloc", "std", "vec::Vec");
        assert_std_target("core::mem", "core", "core", "mem");
        assert_std_module_error("rc", "std::rc");
        assert_std_target("std::rc::Rc", "alloc", "std", "rc::Rc");
        assert_std_target("alloc::rc::Rc", "alloc", "alloc", "rc::Rc");
        assert_std_target("std::mem", "core", "std", "mem");
    }

    #[test]
    fn every_mapped_std_module_resolves_to_its_owning_crate() {
        for &(module, crate_name) in stdlib::mapped_modules() {
            assert_std_target(&format!("std::{module}"), crate_name, "std", module);
            if !stdlib::is_crate(module) {
                assert_std_module_error(module, &format!("std::{module}"));
            }
        }
    }

    #[test]
    fn test_from_target() {
        let temp_dir = setup_test_structure();
        let root = temp_dir.path();

        let test_cases = vec![
            (
                Target {
                    entrypoint: Entrypoint::Path(root.join("workspace/pkg1")),
                    path: vec![],
                },
                ExpectedResult::Path(root.join("workspace/pkg1")),
                vec![],
            ),
            (
                Target {
                    entrypoint: Entrypoint::Path(root.join("workspace/pkg1")),
                    path: vec!["module".to_string()],
                },
                ExpectedResult::Path(root.join("workspace/pkg1")),
                vec!["module".to_string()],
            ),
            (
                Target {
                    entrypoint: Entrypoint::Path(root.join("workspace")),
                    path: vec!["pkg2".to_string()],
                },
                ExpectedResult::Path(root.join("workspace/pkg2")),
                vec![],
            ),
            (
                Target {
                    entrypoint: Entrypoint::Path(root.join("workspace/pkg1/src/module.rs")),
                    path: vec![],
                },
                ExpectedResult::Path(root.join("workspace/pkg1")),
                vec!["module".to_string()],
            ),
            (
                Target {
                    entrypoint: Entrypoint::Path(root.join("standalone")),
                    path: vec!["module".to_string()],
                },
                ExpectedResult::Path(root.join("standalone")),
                vec!["module".to_string()],
            ),
        ];

        for (i, (target, expected_result, expected_filter)) in test_cases.into_iter().enumerate() {
            let result = ResolvedTarget::from_target(target, true);

            match (result, expected_result) {
                (Ok(resolved), ExpectedResult::Path(expected)) => {
                    let resolved_path = resolved
                        .package_path
                        .canonical_path()
                        .unwrap_or_else(|err| panic!("Test case {i} failed: {err}"));
                    let expected_path = fs::canonicalize(expected).unwrap();
                    assert_eq!(
                        resolved_path, expected_path,
                        "Test case {} failed: package_path mismatch",
                        i
                    );
                    assert_eq!(
                        resolved.filter,
                        expected_filter.join("::"),
                        "Test case {} failed: filter mismatch",
                        i
                    );
                }
                (Err(e), _) => {
                    panic!("Test case {i} failed: expected Ok, but got error '{e}'");
                }
            }
        }
    }
}
