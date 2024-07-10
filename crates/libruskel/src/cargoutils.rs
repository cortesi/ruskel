use std::fs;
use std::io::Write;
use std::path::{absolute, Path, PathBuf};

use cargo::{core::Workspace, ops, util::context::GlobalContext};
use tempfile::TempDir;

use crate::error::{Result, RuskelError};

fn is_path(s: &str) -> bool {
    s.contains('.') || s.contains('/') || s.contains('\\') || s.contains(':')
}

fn join_components(components: &[String]) -> String {
    components.join("::")
}

#[derive(Debug)]
pub enum CargoPath {
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

    pub fn copy(&self) -> Result<Self> {
        match self {
            CargoPath::Path(path) => Ok(CargoPath::Path(path.clone())),
            CargoPath::TempDir(_) => Err(RuskelError::Cargo(
                "Cannot copy a TempDir CargoPath".to_string(),
            )),
        }
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

    pub fn create_dummy_crate(
        &self,
        dependency: &str,
        version: Option<&str>,
        features: Option<&[&str]>,
    ) -> Result<()> {
        if self.has_manifest() {
            return Err(RuskelError::Cargo("manifest already exists".to_string()));
        }
        let src_dir = self.as_path().join("src");
        fs::create_dir_all(&src_dir)?;

        let lib_rs = src_dir.join("lib.rs");
        let mut file = fs::File::create(lib_rs)?;
        writeln!(file, "// Dummy crate")?;

        let manifest_path = self.manifest_path();
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
        Ok(())
    }

    pub fn find_dependency(&self, dependency: &str, offline: bool) -> Result<Option<CargoPath>> {
        let mut config = GlobalContext::default().map_err(|e| RuskelError::Cargo(e.to_string()))?;
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
            .map_err(|e| RuskelError::Cargo(e.to_string()))?;
        let workspace = Workspace::new(&self.manifest_path(), &config)
            .map_err(|e| RuskelError::Cargo(e.to_string()))?;

        let (_, ps) = ops::fetch(
            &workspace,
            &ops::FetchOptions {
                gctx: &config,
                targets: vec![],
            },
        )
        .map_err(|e| RuskelError::Cargo(e.to_string()))?;

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

    fn find_workspace_package(&self, module_name: &str) -> Result<Option<ResolvedTarget>> {
        let workspace_manifest_path = self.manifest_path();
        let original_name = module_name.replace('-', "_");
        let normalized_name = module_name.to_string();

        let config = GlobalContext::default().map_err(|e| RuskelError::Cargo(e.to_string()))?;
        let workspace = Workspace::new(&workspace_manifest_path, &config)
            .map_err(|e| RuskelError::Cargo(e.to_string()))?;

        for package in workspace.members() {
            if package.name().as_str() == normalized_name
                || package.name().as_str() == original_name
            {
                let feats = package.summary().features();
                let package_path = package.manifest_path().parent().unwrap().to_path_buf();
                let version = Some(package.version().to_string());
                let features = feats.keys().map(|x| x.as_str().to_string()).collect();
                return Ok(Some(ResolvedTarget {
                    package_path: CargoPath::Path(package_path),
                    filter: String::new(),
                    version,
                    features,
                }));
            }
        }
        Ok(None)
    }

    fn search_spec(&self, components: &[String]) -> Result<Option<ResolvedTarget>> {
        if self.is_package() {
            return Ok(Some(ResolvedTarget {
                package_path: self.copy()?,
                filter: components.join("::"),
                version: None,
                features: vec![],
            }));
        } else if self.is_workspace() {
            if components.is_empty() {
                return Ok(None);
            }
            if let Some(mut resolved) = self.find_workspace_package(&components[0])? {
                resolved.filter = components[1..].join("::");
                return Ok(Some(resolved));
            }
        };
        Ok(None)
    }

    /// Splits a target specification into the CargoPath directory and the filter components. When
    /// this returns, we know that there's always a valid package in the CargoPath directory.
    /// Here are the valid ways to specify a target:
    ///
    /// - "/package/path"  matches a whole package directly
    /// - "/package/path::module::path" matches a module and subpath in a package
    /// - "/workspace/path::package" matches a whole package in the workspace
    /// - "/workspace/path::package::modue::subpath" matches a subpath in a package in the workspace
    /// - If the current directory is inside a workspace:
    ///     - "package" matches a package in the workspace
    ///     - "package::module::path" matches a module and subpath in a package in the workspace
    /// - If the current directory is inside a package:
    ///     - "module::path" matches a module and subpath in the package
    ///     - "package" matches a dependency in the workspace
    /// - Otherwise, the first component is retreived from cargo.io
    pub fn from_target(target: &str) -> Result<ResolvedTarget> {
        let components: Vec<String> = target.split("::").map(|x| x.into()).collect();
        if components.is_empty() {
            return Err(RuskelError::ModuleNotFound("empty target".to_string()));
        }

        if is_path(&components[0]) {
            let root = CargoPath::Path(components[0].clone().into());
            let subpath = components[1..].to_vec();
            if let Some(resolved) = root.search_spec(&subpath)? {
                return Ok(resolved);
            } else if components.len() == 1 {
                return Err(RuskelError::ModuleNotFound(format!(
                    "no submodule specified, but {:?} is not a package",
                    root.as_path().display(),
                )));
            } else {
                return Err(RuskelError::ModuleNotFound(format!(
                    "can't find path {} in directory {}",
                    join_components(&components[1..]),
                    root.as_path().display(),
                )));
            }
        }

        if let Some(root) = CargoPath::nearest_manifest(&PathBuf::from(".")) {
            if let Some(resolved) = root.search_spec(&components)? {
                return Ok(resolved);
            }
        }

        let dummy = CargoPath::TempDir(TempDir::new()?);
        dummy.create_dummy_crate(&components[0], None, None)?;
        Ok(ResolvedTarget {
            package_path: dummy,
            filter: components.join("::"),
            version: None,
            features: vec![],
        })
    }
}

#[derive(Debug)]
pub struct ResolvedTarget {
    pub package_path: CargoPath,
    pub filter: String,
    pub version: Option<String>,
    pub features: Vec<String>,
}

pub fn resolve_target(target: &str, offline: bool) -> Result<ResolvedTarget> {
    let mut resolved = CargoPath::from_target(target)?;
    if !resolved.filter.is_empty() {
        let first_component = resolved.filter.split("::").next().unwrap().to_string();
        if let Some(cp) = resolved
            .package_path
            .find_dependency(&first_component, offline)?
        {
            resolved.package_path = cp;
            resolved.filter = resolved
                .filter
                .split_once("::")
                .map(|x| x.1)
                .unwrap_or("")
                .to_string();
        }
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_create_dummy_crate() -> Result<()> {
        let temp_dir = tempdir()?;
        let cargo_path = CargoPath::Path(temp_dir.path().to_path_buf());

        cargo_path.create_dummy_crate("serde", None, None)?;
        assert!(cargo_path.has_manifest());

        let manifest_content = fs::read_to_string(cargo_path.manifest_path())?;
        println!("{}", manifest_content);
        assert!(manifest_content.contains("[dependencies]"));
        assert!(manifest_content.contains("serde = { version = \"*\""));

        // Ensure creating a second crate fails
        assert!(cargo_path.create_dummy_crate("rand", None, None).is_err());

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
            assert_eq!(resolved.version, Some("0.1.0".to_string()));
            assert_eq!(
                resolved.features,
                vec!["default".to_string(), "feature1".to_string()]
            );
            assert_eq!(resolved.filter, "");
        } else {
            panic!("Failed to find package in the workspace");
        }

        // Test finding another package in the workspace
        if let Some(resolved) = cargo_path.find_workspace_package("member2")? {
            assert_eq!(resolved.package_path.as_path(), member2_dir);
            assert_eq!(resolved.version, Some("0.2.0".to_string()));
            assert!(resolved.features.is_empty());
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

    #[test]
    fn test_from_target() -> Result<()> {
        let temp_dir = tempdir()?;
        let workspace_path = temp_dir.path().to_path_buf();

        // Create a workspace with two packages
        fs::create_dir_all(workspace_path.join("pkg1").join("src"))?;
        fs::create_dir_all(workspace_path.join("pkg2").join("src"))?;

        let workspace_manifest = r#"
            [workspace]
            members = ["pkg1", "pkg2"]
        "#;
        fs::write(workspace_path.join("Cargo.toml"), workspace_manifest)?;

        let pkg1_manifest = r#"
            [package]
            name = "pkg1"
            version = "0.1.0"

            [features]
            feature1 = []
        "#;
        fs::write(
            workspace_path.join("pkg1").join("Cargo.toml"),
            pkg1_manifest,
        )?;
        fs::write(
            workspace_path.join("pkg1").join("src").join("lib.rs"),
            "// pkg1 lib",
        )?;

        let pkg2_manifest = r#"
            [package]
            name = "pkg2"
            version = "0.2.0"
        "#;
        fs::write(
            workspace_path.join("pkg2").join("Cargo.toml"),
            pkg2_manifest,
        )?;
        fs::write(
            workspace_path.join("pkg2").join("src").join("lib.rs"),
            "// pkg2 lib",
        )?;

        // Test resolving a package in the workspace
        let resolved =
            CargoPath::from_target(&format!("{}::pkg1::module", workspace_path.display()))?;
        assert_eq!(resolved.package_path.as_path(), workspace_path.join("pkg1"));
        assert_eq!(resolved.filter, "module");
        assert_eq!(resolved.version, Some("0.1.0".to_string()));
        assert_eq!(resolved.features, vec!["feature1".to_string()]);

        // Test resolving another package in the workspace
        let resolved = CargoPath::from_target(&format!("{}::pkg2", workspace_path.display()))?;
        assert_eq!(resolved.package_path.as_path(), workspace_path.join("pkg2"));
        assert_eq!(resolved.filter, "");
        assert_eq!(resolved.version, Some("0.2.0".to_string()));
        assert!(resolved.features.is_empty());

        // Test resolving a non-existent package
        let result = CargoPath::from_target(&format!("{}::non_existent", workspace_path.display()));
        assert!(result.is_err());

        Ok(())
    }
}
