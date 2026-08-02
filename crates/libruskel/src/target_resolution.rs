use std::{
    collections::BTreeMap,
    env, fs,
    io::Write,
    path::{Component, Path, PathBuf, absolute},
};

use cargo::{core::Workspace, ops, util::context::GlobalContext};
use semver::Version;
use serde::Serialize;
use tempfile::TempDir;

use super::{
    stdlib,
    target::{Entrypoint, Target},
};
use crate::error::{Result, RuskelError, convert_cargo_error};

/// A path to a crate. This can be a directory on the filesystem or the virtual std library.
#[derive(Debug)]
pub struct CargoPath {
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
    pub fn std_names(&self) -> Option<(&str, &str)> {
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
pub fn create_quiet_cargo_config(offline: bool) -> Result<GlobalContext> {
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

/// Package metadata for the temporary dependency resolver.
#[derive(Serialize)]
struct DummyPackage {
    /// Package name.
    name: &'static str,
    /// Package version.
    version: &'static str,
}

/// One dependency in the temporary resolver manifest.
#[derive(Serialize)]
struct DummyDependency {
    /// Requested dependency version.
    version: String,
    /// Requested Cargo features.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    features: Vec<String>,
}

/// Complete temporary resolver manifest.
#[derive(Serialize)]
struct DummyManifest {
    /// Temporary package metadata.
    package: DummyPackage,
    /// The one dependency that Cargo must resolve.
    dependencies: BTreeMap<String, DummyDependency>,
}

/// Construct a minimal manifest for a temporary crate that depends on `dependency`.
fn generate_dummy_manifest(
    dependency: &str,
    version: Option<String>,
    features: Option<&[&str]>,
) -> Result<String> {
    let cargo_dependency = dependency.replace('_', "-");
    let dependency = DummyDependency {
        version: version.unwrap_or_else(|| "*".to_string()),
        features: features
            .unwrap_or_default()
            .iter()
            .map(|feature| (*feature).to_string())
            .collect(),
    };
    let manifest = DummyManifest {
        package: DummyPackage {
            name: "dummy-crate",
            version: "0.1.0",
        },
        dependencies: BTreeMap::from([(cargo_dependency, dependency)]),
    };

    toml::to_string(&manifest).map_err(|error| {
        RuskelError::Generate(format!("Failed to serialize dummy manifest: {error}"))
    })
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

    let manifest = generate_dummy_manifest(dependency, version, features)?;
    fs::write(manifest_path, manifest)?;

    Ok(CargoPath::from_temp_dir(temp_dir))
}

/// A resolved Rust package or module target.
#[derive(Debug)]
pub struct ResolvedTarget {
    /// Package directory path (filesystem or temporary).
    pub package_path: CargoPath,

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
    fn test_generate_dummy_manifest() -> Result<()> {
        let manifest = generate_dummy_manifest(
            "tokio",
            Some("1.0".to_string()),
            Some(&["rt", "macros", "test-util"]),
        )?;
        let document: toml::Value = toml::from_str(&manifest).expect("valid manifest TOML");

        assert_eq!(document["package"]["name"].as_str(), Some("dummy-crate"));
        assert_eq!(document["package"]["version"].as_str(), Some("0.1.0"));
        let dependencies = document["dependencies"]
            .as_table()
            .expect("dependencies table");
        assert_eq!(dependencies.len(), 1);
        let dependency = &dependencies["tokio"];
        assert_eq!(dependency["version"].as_str(), Some("1.0"));
        assert_eq!(
            dependency["features"]
                .as_array()
                .expect("features array")
                .iter()
                .map(|feature| feature.as_str().expect("string feature"))
                .collect::<Vec<_>>(),
            ["rt", "macros", "test-util"]
        );
        assert!(document.get("workspace").is_none());
        Ok(())
    }

    #[test]
    fn test_generate_dummy_manifest_with_underscores() -> Result<()> {
        let manifest =
            generate_dummy_manifest("my_complex_crate_name", Some("0.1.0".to_string()), None)?;
        let document: toml::Value = toml::from_str(&manifest).expect("valid manifest TOML");
        let dependencies = document["dependencies"]
            .as_table()
            .expect("dependencies table");

        assert_eq!(dependencies.len(), 1);
        assert!(dependencies.contains_key("my-complex-crate-name"));
        assert!(!dependencies.contains_key("my_complex_crate_name"));
        assert_eq!(
            dependencies["my-complex-crate-name"]["version"].as_str(),
            Some("0.1.0")
        );
        assert!(
            dependencies["my-complex-crate-name"]
                .get("features")
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn test_create_dummy_crate() -> Result<()> {
        let cargo_path = create_dummy_crate("serde", None, None)?;
        let path = cargo_path.as_path()?;

        assert!(path.join("Cargo.toml").exists());

        let manifest_content = fs::read_to_string(path.join("Cargo.toml"))?;
        assert!(manifest_content.contains("[dependencies]"));
        let document: toml::Value = toml::from_str(&manifest_content).expect("valid manifest TOML");
        assert_eq!(
            document["dependencies"]["serde"]["version"].as_str(),
            Some("*")
        );

        Ok(())
    }

    #[test]
    fn test_create_dummy_crate_with_features() -> Result<()> {
        let cargo_path = create_dummy_crate("serde", Some("1.0".to_string()), Some(&["derive"]))?;
        let path = cargo_path.as_path()?;

        assert!(path.join("Cargo.toml").exists());

        let manifest_content = fs::read_to_string(path.join("Cargo.toml"))?;

        let document: toml::Value = toml::from_str(&manifest_content).expect("valid manifest TOML");
        assert_eq!(
            document["dependencies"]["serde"]["version"].as_str(),
            Some("1.0")
        );
        assert_eq!(
            document["dependencies"]["serde"]["features"][0].as_str(),
            Some("derive")
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
