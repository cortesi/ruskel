use std::fs;
use std::io::Write;
use std::path::{absolute, Path, PathBuf};

use cargo::{core::Workspace, ops, util::context::GlobalContext};
use rustdoc_types::Crate;
use semver::Version;
use tempfile::TempDir;

use super::target::{Entrypoint, Target};
use crate::error::{convert_cargo_error, Result, RuskelError};

/// A path to a crate. This can be a directory on the filesystem or a temporary directory.
#[derive(Debug)]
enum CargoPath {
    Path(PathBuf),
    TempDir(TempDir),
}

impl CargoPath {
    pub fn as_path(&self) -> &Path {
        match self {
            CargoPath::Path(path) => path.as_path(),
            CargoPath::TempDir(temp_dir) => temp_dir.path(),
        }
    }

    pub fn read_crate(
        &self,
        no_default_features: bool,
        all_features: bool,
        features: Vec<String>,
    ) -> Result<Crate> {
        let json_path = rustdoc_json::Builder::default()
            .toolchain("nightly")
            .manifest_path(self.manifest_path())
            .document_private_items(true)
            .no_default_features(no_default_features)
            .all_features(all_features)
            .features(&features)
            .build()
            .map_err(|e| RuskelError::Generate(e.to_string()))?;
        let json_content = fs::read_to_string(&json_path)?;
        let crate_data: Crate = serde_json::from_str(&json_content)?;
        Ok(crate_data)
    }

    pub fn manifest_path(&self) -> PathBuf {
        absolute(self.as_path().join("Cargo.toml")).unwrap()
    }

    pub fn has_manifest(&self) -> bool {
        self.manifest_path().exists()
    }

    pub fn is_package(&self) -> bool {
        self.has_manifest() && !self.is_workspace()
    }

    pub fn is_workspace(&self) -> bool {
        if !self.has_manifest() {
            return false;
        }
        let manifest = cargo_toml::Manifest::from_path(self.manifest_path())
            .map_err(|_| ())
            .ok();
        manifest
            .as_ref()
            .map_or(false, |m| m.workspace.is_some() && m.package.is_none())
    }

    pub fn find_dependency(&self, dependency: &str, offline: bool) -> Result<Option<CargoPath>> {
        let mut config = GlobalContext::default().map_err(convert_cargo_error)?;
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
            .map_err(convert_cargo_error)?;

        let workspace =
            Workspace::new(&self.manifest_path(), &config).map_err(convert_cargo_error)?;

        let (_, ps) = ops::fetch(
            &workspace,
            &ops::FetchOptions {
                gctx: &config,
                targets: vec![],
            },
        )
        .map_err(convert_cargo_error)?;

        for package in ps.packages() {
            if package.name().as_str() == dependency {
                return Ok(Some(CargoPath::Path(
                    package.manifest_path().parent().unwrap().to_path_buf(),
                )));
            }
        }
        Ok(None)
    }

    pub fn nearest_manifest(start_dir: &Path) -> Option<CargoPath> {
        let mut current_dir = start_dir.to_path_buf();

        loop {
            let manifest_path = current_dir.join("Cargo.toml");
            if manifest_path.exists() {
                return Some(CargoPath::Path(current_dir));
            }
            if !current_dir.pop() {
                break;
            }
        }
        None
    }

    /// Find a package in the current workspace by name.
    fn find_workspace_package(&self, module_name: &str) -> Result<Option<ResolvedTarget>> {
        let workspace_manifest_path = self.manifest_path();
        let original_name = module_name.replace('-', "_");
        let normalized_name = module_name.to_string();

        let config = GlobalContext::default().map_err(convert_cargo_error)?;

        let workspace =
            Workspace::new(&workspace_manifest_path, &config).map_err(convert_cargo_error)?;

        for package in workspace.members() {
            if package.name().as_str() == normalized_name
                || package.name().as_str() == original_name
            {
                let package_path = package.manifest_path().parent().unwrap().to_path_buf();
                return Ok(Some(ResolvedTarget::new(
                    CargoPath::Path(package_path),
                    &[],
                )));
            }
        }
        Ok(None)
    }
}

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

    let version_str = version.map_or("*".to_string(), |v| v.to_string());
    let features_str = features.map_or(String::new(), |f| format!(", features = {:?}", f));
    let manifest = format!(
        r#"[package]
        name = "dummy-crate"
        version = "0.1.0"

        [dependencies]
        {} = {{ version = "{}"{}}}
        "#,
        dependency, version_str, features_str
    );
    fs::write(manifest_path, manifest)?;

    Ok(CargoPath::TempDir(temp_dir))
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
    fn new(path: CargoPath, components: &[String]) -> Self {
        let filter = if components.is_empty() {
            String::new()
        } else {
            let mut normalized_components = components.to_vec();
            normalized_components[0] = to_import_name(&normalized_components[0]);
            normalized_components.join("::")
        };

        ResolvedTarget {
            package_path: path,
            filter,
        }
    }

    pub fn read_crate(
        &self,
        no_default_features: bool,
        all_features: bool,
        features: Vec<String>,
    ) -> Result<Crate> {
        self.package_path
            .read_crate(no_default_features, all_features, features)
    }

    pub fn from_target(target: Target, offline: bool) -> Result<Self> {
        match target.entrypoint {
            Entrypoint::Path(path) => {
                if path.is_file() && path.extension().map_or(false, |ext| ext == "rs") {
                    Self::from_rust_file(path, &target.path)
                } else {
                    let cargo_path = CargoPath::Path(path.clone());
                    if cargo_path.is_package() {
                        Ok(ResolvedTarget::new(cargo_path, &target.path))
                    } else if cargo_path.is_workspace() {
                        if target.path.is_empty() {
                            Err(RuskelError::InvalidTarget(
                                "No package specified in workspace".to_string(),
                            ))
                        } else {
                            let package_name = &target.path[0];
                            if let Some(package) =
                                cargo_path.find_workspace_package(package_name)?
                            {
                                Ok(ResolvedTarget::new(package.package_path, &target.path[1..]))
                            } else {
                                Err(RuskelError::ModuleNotFound(format!(
                                    "Package '{}' not found in workspace",
                                    package_name
                                )))
                            }
                        }
                    } else {
                        Err(RuskelError::InvalidTarget(format!(
                            "Path '{}' is neither a package nor a workspace",
                            path.display()
                        )))
                    }
                }
            }
            Entrypoint::Name { name, version } => {
                let current_dir = std::env::current_dir()?;
                match CargoPath::nearest_manifest(&current_dir) {
                    Some(root) => {
                        if let Some(dependency) = root.find_dependency(&name, offline)? {
                            Ok(ResolvedTarget::new(dependency, &target.path))
                        } else {
                            Self::from_dummy_crate(&name, version, &target.path)
                        }
                    }
                    None => Self::from_dummy_crate(&name, version, &target.path),
                }
            }
        }
    }

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

        let cargo_path = CargoPath::Path(current_dir.clone());
        let relative_path = file_path.strip_prefix(&current_dir).map_err(|_| {
            RuskelError::InvalidTarget("Failed to determine relative path".to_string())
        })?;

        // Convert the relative path to a module path
        let mut components: Vec<_> = relative_path
            .components()
            .filter_map(|c| {
                if let std::path::Component::Normal(os_str) = c {
                    os_str.to_str().map(String::from)
                } else {
                    None
                }
            })
            .collect();

        // Remove "src" if it's the first component
        if components.first().map_or(false, |c| c == "src") {
            components.remove(0);
        }

        // Remove the last component (file name) and add it back without the extension
        if let Some(file_name) = components.pop() {
            if let Some(stem) = Path::new(&file_name).file_stem().and_then(|s| s.to_str()) {
                components.push(stem.to_string());
            }
        }

        // Combine the module path with the additional path
        components.extend_from_slice(additional_path);

        Ok(ResolvedTarget::new(cargo_path, &components))
    }

    fn from_dummy_crate(name: &str, version: Option<Version>, path: &[String]) -> Result<Self> {
        let version_str = version.map(|v| v.to_string());
        let dummy = create_dummy_crate(name, version_str, None)?;

        // Find the dependency within the dummy crate
        if let Some(dependency_path) = dummy.find_dependency(name, true)? {
            Ok(ResolvedTarget::new(dependency_path, path))
        } else {
            Err(RuskelError::ModuleNotFound(format!(
                "Dependency '{}' not found in dummy crate",
                name
            )))
        }
    }
}

/// Resovles a target specification and returns a ResolvedTarget, pointing to the package
/// directory. If necessary, construct temporary dummy crate to download packages from cargo.io.
pub fn resolve_target(target_str: &str, offline: bool) -> Result<ResolvedTarget> {
    let target = Target::parse(target_str)?;

    match &target.entrypoint {
        Entrypoint::Path(_) => ResolvedTarget::from_target(target, offline),
        Entrypoint::Name { name, version } => {
            if version.is_some() {
                // If a version is specified, always create a dummy package
                ResolvedTarget::from_dummy_crate(name, version.clone(), &target.path)
            } else {
                let resolved = ResolvedTarget::from_target(target.clone(), offline)?;
                if !resolved.filter.is_empty() {
                    let first_component = resolved.filter.split("::").next().unwrap().to_string();
                    if let Some(cp) = resolved
                        .package_path
                        .find_dependency(&first_component, offline)?
                    {
                        Ok(ResolvedTarget::new(cp, &target.path))
                    } else {
                        Ok(resolved)
                    }
                } else {
                    Ok(resolved)
                }
            }
        }
    }
}

fn to_import_name(package_name: &str) -> String {
    package_name.replace('-', "_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::fs;
    use tempfile::tempdir;

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
    fn test_create_dummy_crate() -> Result<()> {
        let cargo_path = create_dummy_crate("serde", None, None)?;
        let path = cargo_path.as_path();

        assert!(path.join("Cargo.toml").exists());

        let manifest_content = fs::read_to_string(path.join("Cargo.toml"))?;
        assert!(manifest_content.contains("[dependencies]"));
        assert!(manifest_content.contains("serde = { version = \"*\""));

        Ok(())
    }

    #[test]
    fn test_is_workspace() -> Result<()> {
        let temp_dir = tempdir()?;
        let cargo_path = CargoPath::Path(temp_dir.path().to_path_buf());

        // Create a workspace Cargo.toml
        let manifest = r#"
            [workspace]
            members = ["member1", "member2"]
        "#;
        fs::write(cargo_path.manifest_path(), manifest)?;
        assert!(cargo_path.is_workspace());

        // Create a regular Cargo.toml
        fs::write(
            cargo_path.manifest_path(),
            r#"[package] name = "test-crate""#,
        )?;
        assert!(!cargo_path.is_workspace());

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

        let cargo_path = CargoPath::Path(temp_dir.path().to_path_buf());

        // Test finding a package in the workspace
        if let Some(resolved) = cargo_path.find_workspace_package("member1")? {
            assert_eq!(resolved.package_path.as_path(), member1_dir);
            assert_eq!(resolved.filter, "");
        } else {
            panic!("Failed to find package in the workspace");
        }

        // Test finding another package in the workspace
        if let Some(resolved) = cargo_path.find_workspace_package("member2")? {
            assert_eq!(resolved.package_path.as_path(), member2_dir);
            assert_eq!(resolved.filter, "");
        } else {
            panic!("Failed to find package in the workspace");
        }

        // Test not finding a package in the workspace
        assert!(cargo_path
            .find_workspace_package("non-existent-package")?
            .is_none());

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
            serde = "1.0"
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
        Error(String),
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
            (
                Target {
                    entrypoint: Entrypoint::Name {
                        name: "nonexistent".to_string(),
                        version: None,
                    },
                    path: vec![],
                },
                ExpectedResult::Error("No matching".to_string()),
                vec![],
            ),
        ];

        for (i, (target, expected_result, expected_filter)) in test_cases.into_iter().enumerate() {
            let result = ResolvedTarget::from_target(target, true);

            match (result, expected_result) {
                (Ok(resolved), ExpectedResult::Path(expected)) => {
                    match &resolved.package_path {
                        CargoPath::Path(path) => {
                            let resolved_path = fs::canonicalize(path).unwrap();
                            let expected_path = fs::canonicalize(expected).unwrap();
                            assert_eq!(
                                resolved_path, expected_path,
                                "Test case {} failed: package_path mismatch",
                                i
                            );
                        }
                        CargoPath::TempDir(_) => {
                            panic!("Test case {} failed: expected CargoPath::Path, got CargoPath::TempDir", i);
                        }
                    }
                    assert_eq!(
                        resolved.filter,
                        expected_filter.join("::"),
                        "Test case {} failed: filter mismatch",
                        i
                    );
                }
                (Err(e), ExpectedResult::Error(expected_err)) => {
                    assert!(
                        e.to_string().contains(&expected_err),
                        "Test case {} failed: error message mismatch. Expected '{}', got '{}'",
                        i,
                        expected_err,
                        e
                    );
                }
                (Ok(_), ExpectedResult::Error(expected_err)) => {
                    panic!(
                        "Test case {} failed: expected error '{}', but got Ok",
                        i, expected_err
                    );
                }
                (Err(e), _) => {
                    panic!("Test case {} failed: expected Ok, but got error '{}'", i, e);
                }
            }
        }
    }
}
