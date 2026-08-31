use std::{
    collections::{BTreeMap, BTreeSet},
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

/// Final source selected for rustdoc loading or generation.
#[derive(Debug)]
pub enum ResolvedSource {
    /// Canonical manifest for a package.
    Package {
        /// Canonical `Cargo.toml` path.
        manifest_path: PathBuf,
    },
    /// Standard-library crate mapping.
    StdLibrary {
        /// Crate whose rustdoc JSON contains the item.
        actual: String,
        /// Crate name requested by the user.
        display: String,
    },
}

impl ResolvedSource {
    /// Create a package source from a manifest and canonicalize its identity.
    fn package(manifest_path: &Path) -> Result<Self> {
        let manifest_path = fs::canonicalize(manifest_path).map_err(|error| {
            RuskelError::Generate(format!(
                "Failed to canonicalize manifest '{}': {error}",
                manifest_path.display()
            ))
        })?;
        Ok(Self::Package { manifest_path })
    }
}

/// Manifest used while Ruskel discovers a final package source.
#[derive(Debug)]
struct ManifestContext {
    /// Absolute manifest path used by Cargo workspace operations.
    manifest_path: PathBuf,
}

impl ManifestContext {
    /// Create a discovery context from a directory that contains `Cargo.toml`.
    fn from_directory(directory: &Path) -> Result<Self> {
        let manifest_path = directory.join("Cargo.toml");
        let manifest_path = absolute(&manifest_path).map_err(|error| {
            RuskelError::Generate(format!(
                "Failed to resolve manifest path for '{}': {error}",
                manifest_path.display()
            ))
        })?;
        Ok(Self { manifest_path })
    }

    /// Return whether this context points to an existing manifest.
    fn has_manifest(&self) -> bool {
        self.manifest_path.exists()
    }

    /// Identify a standalone package manifest.
    fn is_package(&self) -> Result<bool> {
        Ok(self.has_manifest() && !self.is_workspace()?)
    }

    /// Identify a virtual workspace manifest.
    fn is_workspace(&self) -> Result<bool> {
        if !self.has_manifest() {
            return Ok(false);
        }
        let manifest = cargo_toml::Manifest::from_path(&self.manifest_path)
            .map_err(|error| RuskelError::ManifestParse(error.to_string()))?;
        Ok(manifest.workspace.is_some() && manifest.package.is_none())
    }

    /// Find one direct dependency from the current Cargo package.
    fn find_dependency(&self, dependency: &str, offline: bool) -> Result<Option<ResolvedSource>> {
        let config = create_quiet_cargo_config(offline)?;
        let workspace = Workspace::new(&self.manifest_path, &config)
            .map_err(|error| convert_cargo_error(&error))?;
        let (resolve, packages) = ops::fetch(
            &workspace,
            &ops::FetchOptions {
                gctx: &config,
                targets: vec![],
            },
        )
        .map_err(|error| convert_cargo_error(&error))?;
        let Some(current_package) = workspace.current_opt() else {
            return Ok(None);
        };
        let current_id = current_package.package_id();
        let matching_ids = |name: &str| {
            resolve
                .deps(current_id)
                .filter_map(|(package_id, dependencies)| {
                    dependencies
                        .iter()
                        .any(|edge| edge.name_in_toml().as_str() == name)
                        .then_some(package_id)
                })
                .collect::<BTreeSet<_>>()
        };
        let mut matches = matching_ids(dependency);
        if matches.is_empty() {
            matches = matching_ids(&alternate_package_spelling(dependency));
        }

        let Some(package_id) = matches.pop_first() else {
            return Ok(None);
        };
        if !matches.is_empty() {
            let mut package_ids = vec![package_id];
            package_ids.extend(matches);
            let choices = package_ids
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(RuskelError::InvalidTarget(format!(
                "Dependency '{dependency}' resolves to multiple direct packages: {choices}"
            )));
        }

        let package = packages
            .get_one(package_id)
            .map_err(|error| convert_cargo_error(&error))?;
        ResolvedSource::package(package.manifest_path()).map(Some)
    }

    /// Walk upwards from `start_dir` to find the closest `Cargo.toml`.
    fn nearest(start_dir: &Path) -> Option<Self> {
        let mut current_dir = start_dir.to_path_buf();

        loop {
            let manifest_path = current_dir.join("Cargo.toml");
            if manifest_path.exists() {
                return Some(Self { manifest_path });
            }
            if !current_dir.pop() {
                return None;
            }
        }
    }

    /// Find a package in the current workspace by name.
    fn find_workspace_package(&self, module_name: &str) -> Result<Option<ResolvedSource>> {
        let alternate = alternate_package_spelling(module_name);
        let config = create_quiet_cargo_config(false)?;
        let workspace = Workspace::new(&self.manifest_path, &config)
            .map_err(|error| convert_cargo_error(&error))?;

        for package in workspace.members() {
            let package_name = package.name().as_str();
            if package_name == module_name || package_name == alternate {
                return ResolvedSource::package(package.manifest_path()).map(Some);
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

/// Construct a minimal manifest for a temporary crate that depends on
/// `dependency`.
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
) -> Result<TempDir> {
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

    Ok(temp_dir)
}

/// A resolved Rust package or module target.
#[derive(Debug)]
pub struct ResolvedTarget {
    /// Package manifest or standard-library mapping.
    pub(super) source: ResolvedSource,

    /// Module path within the package, excluding the package name. E.g.,
    /// "module::submodule::item". Empty string for package root. This might not
    /// necessarily match the user's input.
    pub(super) filter: String,
}

impl ResolvedTarget {
    /// Build a `ResolvedTarget` with a normalised module filter path.
    fn new(source: ResolvedSource, components: &[String]) -> Self {
        let filter = if components.is_empty() {
            String::new()
        } else {
            let mut normalized_components = components.to_vec();
            normalized_components[0] = to_import_name(&normalized_components[0]);
            normalized_components.join("::")
        };

        Self { source, filter }
    }

    /// Resolve a standard library crate name, optionally overriding the display
    /// name.
    fn resolve_std_crate(name: &str, display_name: Option<&str>, path: &[String]) -> Option<Self> {
        stdlib::is_crate(name).then(|| {
            let display = display_name.unwrap_or(name);
            Self::new(
                ResolvedSource::StdLibrary {
                    actual: name.to_string(),
                    display: display.to_string(),
                },
                path,
            )
        })
    }

    /// Reject bare standard library module names that require an explicit
    /// `std::` prefix.
    fn reject_std_module_name(name: &str) -> Result<()> {
        if stdlib::is_module(name) {
            return Err(RuskelError::InvalidTarget(format!(
                "'{name}' appears to be a standard library module. Use the full path like 'std::{name}'"
            )));
        }

        Ok(())
    }

    /// Resolve a `Target` into a fully-qualified location and filter path.
    fn from_target(target: Target, offline: bool) -> Result<Self> {
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

        let canonical_path = fs::canonicalize(&path).map_err(|error| {
            RuskelError::Generate(format!(
                "Failed to canonicalize path '{}': {error}",
                path.display()
            ))
        })?;
        let context = ManifestContext::from_directory(&canonical_path)?;
        if context.is_package()? {
            return Ok(Self::new(
                ResolvedSource::package(&context.manifest_path)?,
                target_path,
            ));
        }
        if context.is_workspace()? {
            return Self::from_workspace_path(&context, target_path);
        }

        Err(RuskelError::InvalidTarget(format!(
            "Path '{}' is neither a package nor a workspace",
            canonical_path.display()
        )))
    }

    /// Resolve a workspace root plus package path to a concrete package target.
    fn from_workspace_path(context: &ManifestContext, target_path: &[String]) -> Result<Self> {
        let Some(package_name) = target_path.first() else {
            return Err(RuskelError::InvalidTarget(
                "No package specified in workspace".to_string(),
            ));
        };

        if let Some(source) = context.find_workspace_package(package_name)? {
            return Ok(Self::new(source, &target_path[1..]));
        }

        Err(RuskelError::ModuleNotFound(format!(
            "Package '{package_name}' not found in workspace"
        )))
    }

    /// Resolve a named entrypoint against std, workspace, dependencies, or
    /// crates.io.
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
        match ManifestContext::nearest(&current_dir) {
            Some(context) => {
                Self::from_manifest_root(&context, name, version, target_path, offline)
            }
            None => Self::from_dummy_crate(name, version, target_path, offline),
        }
    }

    /// Resolve a named target using the nearest manifest as the root context.
    fn from_manifest_root(
        context: &ManifestContext,
        name: &str,
        version: Option<Version>,
        target_path: &[String],
        offline: bool,
    ) -> Result<Self> {
        if let Some(source) = context.find_workspace_package(name)? {
            return Ok(Self::new(source, target_path));
        }

        if let Some(source) = context.find_dependency(name, offline)? {
            return Ok(Self::new(source, target_path));
        }

        Self::from_dummy_crate(name, version, target_path, offline)
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

        let context = ManifestContext::from_directory(&current_dir)?;
        Ok(Self::new(
            ResolvedSource::package(&context.manifest_path)?,
            &components,
        ))
    }

    /// Create a resolved target backed by a temporary crate for registry
    /// dependencies.
    fn from_dummy_crate(
        name: &str,
        version: Option<Version>,
        path: &[String],
        offline: bool,
    ) -> Result<Self> {
        let version_str = version.map(|v| v.to_string());
        let dummy = create_dummy_crate(name, version_str, None)?;
        let context = ManifestContext::from_directory(dummy.path())?;

        match context.find_dependency(name, offline) {
            Ok(Some(source)) => Ok(Self::new(source, path)),
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

            ResolvedTarget::from_target(target, offline)
        }
    }
}

/// Convert a package name into its canonical import form by replacing hyphens.
fn to_import_name(package_name: &str) -> String {
    package_name.replace('-', "_")
}

/// Return the equivalent Cargo or Rust import spelling for a package name.
fn alternate_package_spelling(package_name: &str) -> String {
    if package_name.contains('_') {
        package_name.replace('_', "-")
    } else {
        package_name.replace('-', "_")
    }
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
    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    use super::*;

    struct DirGuard {
        original: PathBuf,
        _guard: MutexGuard<'static, ()>,
    }

    impl DirGuard {
        fn change_to(path: &Path) -> Result<Self> {
            let guard = ENV_LOCK.lock().expect("environment mutex poisoned");
            let original = env::current_dir()?;
            env::set_current_dir(path)?;
            Ok(Self {
                original,
                _guard: guard,
            })
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
            // SAFETY: the mutex ensures exclusive access while we mutate process
            // environment.
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

    /// Create one minimal package with optional additional manifest sections.
    fn write_package(path: &Path, name: &str, manifest_tail: &str) -> Result<()> {
        write_package_version(path, name, "0.1.0", manifest_tail)
    }

    /// Create one minimal package at an explicit version.
    fn write_package_version(
        path: &Path,
        name: &str,
        version: &str,
        manifest_tail: &str,
    ) -> Result<()> {
        fs::create_dir_all(path.join("src"))?;
        fs::write(
            path.join("Cargo.toml"),
            format!(
                "[package]\nname = {name:?}\nversion = {version:?}\nedition = \"2024\"\n{manifest_tail}"
            ),
        )?;
        fs::write(path.join("src/lib.rs"), "")?;
        Ok(())
    }

    /// Return the package manifest from a resolved source.
    fn package_manifest(source: &ResolvedSource) -> &Path {
        match source {
            ResolvedSource::Package { manifest_path } => manifest_path,
            ResolvedSource::StdLibrary { .. } => panic!("expected package source"),
        }
    }

    /// Return the canonical package directory from a resolved source.
    fn package_directory(source: &ResolvedSource) -> &Path {
        package_manifest(source)
            .parent()
            .expect("package manifest must have a parent")
    }

    /// Add one package to a Cargo directory source.
    fn write_directory_source_package(
        source_root: &Path,
        name: &str,
        version: &str,
    ) -> Result<PathBuf> {
        let package = source_root.join(format!("{name}-{version}"));
        let manifest =
            format!("[package]\nname = {name:?}\nversion = {version:?}\nedition = \"2024\"\n");
        let source = "pub struct RegistryFixture;\n";
        fs::create_dir_all(package.join("src"))?;
        fs::write(package.join("Cargo.toml"), &manifest)?;
        fs::write(package.join("src/lib.rs"), source)?;
        let checksum = serde_json::json!({
            "files": {
                "Cargo.toml": hex::encode(Sha256::digest(manifest.as_bytes())),
                "src/lib.rs": hex::encode(Sha256::digest(source.as_bytes())),
            },
            "package": null,
        });
        fs::write(
            package.join(".cargo-checksum.json"),
            serde_json::to_vec(&checksum)?,
        )?;
        Ok(package)
    }

    /// Resolve one direct dependency and return its package directory.
    fn direct_dependency_directory(context: &ManifestContext, name: &str) -> Result<PathBuf> {
        let source = context
            .find_dependency(name, true)?
            .unwrap_or_else(|| panic!("direct dependency {name:?} should resolve"));
        Ok(package_directory(&source).to_path_buf())
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
        let temp_dir = create_dummy_crate("serde", None, None)?;
        let path = temp_dir.path();

        assert!(path.join("Cargo.toml").exists());

        let manifest_content = fs::read_to_string(path.join("Cargo.toml"))?;
        let document: toml::Value = toml::from_str(&manifest_content).expect("valid manifest TOML");
        assert_eq!(
            document["dependencies"]["serde"]["version"].as_str(),
            Some("*")
        );

        Ok(())
    }

    #[test]
    fn test_create_dummy_crate_with_features() -> Result<()> {
        let temp_dir = create_dummy_crate("serde", Some("1.0".to_string()), Some(&["derive"]))?;
        let path = temp_dir.path();

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
        let context = ManifestContext::from_directory(temp_dir.path())?;

        // Create a workspace Cargo.toml
        let manifest = r#"
            [workspace]
            members = ["member1", "member2"]
        "#;
        let manifest_path = &context.manifest_path;
        fs::write(manifest_path, manifest)?;
        assert!(context.is_workspace()?);

        // Create a regular Cargo.toml
        fs::write(
            manifest_path,
            r#"
[package]
name = "test-crate"
version = "0.1.0"
"#,
        )?;
        assert!(!context.is_workspace()?);

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

        let context = ManifestContext::from_directory(temp_dir.path())?;

        // Test finding a package in the workspace
        if let Some(source) = context.find_workspace_package("member1")? {
            assert_eq!(package_directory(&source), fs::canonicalize(member1_dir)?);
        } else {
            panic!("Failed to find package in the workspace");
        }

        // Test finding another package in the workspace
        if let Some(source) = context.find_workspace_package("member2")? {
            assert_eq!(package_directory(&source), fs::canonicalize(member2_dir)?);
        } else {
            panic!("Failed to find package in the workspace");
        }

        // Test not finding a package in the workspace
        assert!(
            context
                .find_workspace_package("non-existent-package")?
                .is_none()
        );

        Ok(())
    }

    #[test]
    fn resolves_direct_dependency_keys_across_cargo_edge_kinds() -> Result<()> {
        let temp_dir = tempdir()?;
        let host = temp_dir.path().join("host");
        let dependencies = temp_dir.path().join("dependencies");
        let cases = [
            ("normal", "normal"),
            ("renamed_alias", "renamed-package"),
            ("hyphen_key", "hyphen-key"),
            ("dev_only", "dev-package"),
            ("build_only", "build-package"),
            ("target_only", "target-package"),
        ];
        for (_, package_name) in cases {
            write_package(&dependencies.join(package_name), package_name, "")?;
        }
        write_package(
            &host,
            "host",
            r#"[dependencies]
normal = { path = "../dependencies/normal" }
renamed_alias = { package = "renamed-package", path = "../dependencies/renamed-package" }
hyphen-key = { path = "../dependencies/hyphen-key" }

[dev-dependencies]
dev_only = { package = "dev-package", path = "../dependencies/dev-package" }

[build-dependencies]
build_only = { package = "build-package", path = "../dependencies/build-package" }

[target.'cfg(target_os = "none")'.dependencies]
target_only = { package = "target-package", path = "../dependencies/target-package" }
"#,
        )?;
        let context = ManifestContext::from_directory(&host)?;

        for (entrypoint, package_name) in cases {
            assert_eq!(
                direct_dependency_directory(&context, entrypoint)?,
                fs::canonicalize(dependencies.join(package_name))?
            );
        }
        let resolved = ResolvedTarget::from_manifest_root(
            &context,
            "renamed_alias",
            None,
            &["RenamedItem".to_string()],
            true,
        )?;
        assert_eq!(
            package_directory(&resolved.source),
            fs::canonicalize(dependencies.join("renamed-package"))?
        );
        assert_eq!(resolved.filter, "RenamedItem");
        Ok(())
    }

    #[test]
    fn exact_dependency_key_precedes_alternate_spelling() -> Result<()> {
        let temp_dir = tempdir()?;
        let host = temp_dir.path().join("host");
        let exact = temp_dir.path().join("dependencies/exact-package");
        let alternate = temp_dir.path().join("dependencies/alternate-package");
        write_package(&exact, "exact-package", "")?;
        write_package(&alternate, "alternate-package", "")?;
        write_package(
            &host,
            "host",
            r#"[dependencies]
foo_bar = { package = "exact-package", path = "../dependencies/exact-package" }
foo-bar = { package = "alternate-package", path = "../dependencies/alternate-package" }
"#,
        )?;
        let context = ManifestContext::from_directory(&host)?;

        assert_eq!(
            direct_dependency_directory(&context, "foo_bar")?,
            fs::canonicalize(exact)?
        );
        Ok(())
    }

    #[test]
    fn direct_alias_selects_one_package_version() -> Result<()> {
        let temp_dir = tempdir()?;
        let host = temp_dir.path().join("host");
        let version_one = temp_dir.path().join("dependencies/multi-v1");
        let version_two = temp_dir.path().join("dependencies/multi-v2");
        write_package_version(&version_one, "multi-package", "1.0.0", "")?;
        write_package_version(&version_two, "multi-package", "2.0.0", "")?;
        write_package(
            &host,
            "host",
            r#"[dependencies]
version_one = { package = "multi-package", path = "../dependencies/multi-v1" }
version_two = { package = "multi-package", path = "../dependencies/multi-v2" }
"#,
        )?;
        let context = ManifestContext::from_directory(&host)?;

        assert_eq!(
            direct_dependency_directory(&context, "version_two")?,
            fs::canonicalize(version_two)?
        );
        Ok(())
    }

    #[test]
    fn transitive_dependency_is_not_an_entrypoint() -> Result<()> {
        let temp_dir = tempdir()?;
        let host = temp_dir.path().join("host");
        let middle = temp_dir.path().join("dependencies/middle");
        let leaf = temp_dir.path().join("dependencies/leaf");
        write_package(
            &host,
            "host",
            "[dependencies]\nmiddle = { path = \"../dependencies/middle\" }\n",
        )?;
        write_package(
            &middle,
            "middle",
            "[dependencies]\nleaf = { path = \"../leaf\" }\n",
        )?;
        write_package(&leaf, "leaf", "")?;
        let context = ManifestContext::from_directory(&host)?;

        assert!(context.find_dependency("leaf", true)?.is_none());
        Ok(())
    }

    #[test]
    fn virtual_workspace_has_no_direct_dependency_entrypoint() -> Result<()> {
        let temp_dir = tempdir()?;
        let member = temp_dir.path().join("member");
        let dependency = temp_dir.path().join("dependency");
        fs::write(
            temp_dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"member\"]\nresolver = \"2\"\n",
        )?;
        write_package(
            &member,
            "member",
            "[dependencies]\ndependency = { path = \"../dependency\" }\n",
        )?;
        write_package(&dependency, "dependency", "")?;
        let context = ManifestContext::from_directory(temp_dir.path())?;

        assert!(context.find_dependency("dependency", true)?.is_none());
        Ok(())
    }

    #[test]
    fn ambiguous_target_specific_edges_return_stable_error() -> Result<()> {
        let temp_dir = tempdir()?;
        let host = temp_dir.path().join("host");
        let cargo_home = temp_dir.path().join("cargo-home");
        let source_root = temp_dir.path().join("registry-source");
        fs::create_dir_all(&cargo_home)?;
        write_directory_source_package(&source_root, "shared-package", "1.0.0")?;
        write_directory_source_package(&source_root, "shared-package", "2.0.0")?;
        fs::write(
            cargo_home.join("config.toml"),
            format!(
                "[source.crates-io]\nreplace-with = \"fixture\"\n\n[source.fixture]\ndirectory = {:?}\n",
                source_root.to_string_lossy()
            ),
        )?;
        write_package(
            &host,
            "host",
            r#"[target.'cfg(unix)'.dependencies]
shared = { package = "shared-package", version = "=1.0.0" }

[target.'cfg(windows)'.dependencies]
shared = { package = "shared-package", version = "=2.0.0" }
"#,
        )?;
        let _cargo_home_guard = EnvVarGuard::set_path("CARGO_HOME", &cargo_home);
        let context = ManifestContext::from_directory(&host)?;

        let error = context
            .find_dependency("shared", true)
            .expect_err("distinct direct package IDs should be ambiguous");
        let message = error.to_string();
        assert!(
            matches!(error, RuskelError::InvalidTarget(_)),
            "unexpected ambiguity error: {error:?}"
        );
        assert!(message.contains("shared-package v1.0.0"));
        assert!(message.contains("shared-package v2.0.0"));
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

        let ResolvedTarget { source, filter } = resolved;
        let expected = fs::canonicalize(&localcrate_dir)?;

        assert_eq!(package_directory(&source), expected);
        assert!(filter.is_empty());

        Ok(())
    }

    #[test]
    fn intra_package_path_does_not_retarget_to_dependency() -> Result<()> {
        let temp_dir = tempdir()?;
        let workspace = temp_dir.path().join("workspace");
        let app = workspace.join("app");
        let dependency = temp_dir.path().join("dependencies/shadow");

        fs::create_dir_all(&workspace)?;
        fs::write(
            workspace.join("Cargo.toml"),
            "[workspace]\nmembers = [\"app\"]\nresolver = \"2\"\n",
        )?;
        write_package(
            &app,
            "app",
            "[dependencies]\nshadow = { path = \"../../dependencies/shadow\" }\n",
        )?;
        write_package(&dependency, "shadow", "")?;

        let _guard = DirGuard::change_to(&workspace)?;
        let resolved = resolve_target("app::shadow", true)?;

        assert_eq!(package_directory(&resolved.source), fs::canonicalize(app)?);
        assert_eq!(resolved.filter, "shadow");
        Ok(())
    }

    #[test]
    fn dependency_path_does_not_retarget_to_transitive_dependency() -> Result<()> {
        let temp_dir = tempdir()?;
        let host = temp_dir.path().join("host");
        let middle = temp_dir.path().join("dependencies/middle");
        let leaf = temp_dir.path().join("dependencies/leaf");

        write_package(
            &host,
            "host",
            "[dependencies]\nmiddle = { path = \"../dependencies/middle\" }\n",
        )?;
        write_package(
            &middle,
            "middle",
            "[dependencies]\nleaf = { path = \"../leaf\" }\n",
        )?;
        write_package(&leaf, "leaf", "")?;

        let _guard = DirGuard::change_to(&host)?;
        let resolved = resolve_target("middle::leaf", true)?;

        assert_eq!(
            package_directory(&resolved.source),
            fs::canonicalize(middle)?
        );
        assert_eq!(resolved.filter, "leaf");
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

    #[test]
    fn missing_direct_dependency_falls_back_to_local_registry_source() -> Result<()> {
        let temp_dir = tempdir()?;
        let cargo_home = temp_dir.path().join("cargo-home");
        let source_root = temp_dir.path().join("registry-source");
        let host = temp_dir.path().join("host");
        fs::create_dir_all(&cargo_home)?;
        write_package(&host, "host", "")?;
        let package =
            write_directory_source_package(&source_root, "local-registry-crate", "1.2.3")?;
        fs::write(
            cargo_home.join("config.toml"),
            format!(
                "[source.crates-io]\nreplace-with = \"fixture\"\n\n[source.fixture]\ndirectory = {:?}\n",
                source_root.to_string_lossy()
            ),
        )?;
        let _cargo_home_guard = EnvVarGuard::set_path("CARGO_HOME", &cargo_home);
        let context = ManifestContext::from_directory(&host)?;

        let resolved = ResolvedTarget::from_manifest_root(
            &context,
            "local_registry_crate",
            None,
            &["RegistryFixture".to_string()],
            true,
        )?;

        assert_eq!(
            package_directory(&resolved.source),
            fs::canonicalize(package)?
        );
        assert_eq!(resolved.filter, "RegistryFixture");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn package_source_canonicalizes_symlinked_entrypoint() -> Result<()> {
        use std::os::unix::fs::symlink;

        let temp_dir = tempdir()?;
        let package = temp_dir.path().join("package");
        let alias = temp_dir.path().join("alias");
        write_package(&package, "canonical-package", "")?;
        symlink(&package, &alias)?;

        let resolved = ResolvedTarget::from_target(
            Target {
                entrypoint: Entrypoint::Path(alias),
                path: Vec::new(),
            },
            true,
        )?;

        assert_eq!(
            package_manifest(&resolved.source),
            fs::canonicalize(package.join("Cargo.toml"))?
        );
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
        match &result.source {
            ResolvedSource::StdLibrary { actual, display } => {
                assert_eq!(actual, expected_actual);
                assert_eq!(display, expected_display);
            }
            ResolvedSource::Package { .. } => panic!("Expected StdLibrary variant for {target}"),
        }
        assert_eq!(result.filter, expected_filter);
    }

    /// Assert that resolving a bare module fails with the expected error
    /// message.
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
                    let expected_path = fs::canonicalize(expected).unwrap();
                    assert_eq!(
                        package_directory(&resolved.source),
                        expected_path,
                        "Test case {} failed: package source mismatch",
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
