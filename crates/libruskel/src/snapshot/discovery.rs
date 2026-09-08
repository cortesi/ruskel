use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use cargo::core::{Package, Workspace};

use crate::{
    error::{Result, RuskelError, convert_cargo_error},
    snapshot::SnapshotFeatures,
    target_resolution::create_quiet_cargo_config,
};

/// Owned metadata for one selected library-like Cargo package.
#[derive(Debug, Clone)]
pub struct DiscoveredPackage {
    /// Canonical package manifest path.
    pub(crate) manifest_path: PathBuf,
    /// Cargo package name.
    pub(crate) package_name: String,
    /// Normalized Rust crate name.
    pub(crate) crate_name: String,
    /// Generated snapshot filename.
    pub(crate) filename: String,
    /// Cargo features declared by this package.
    features: BTreeSet<String>,
}

/// Canonically ordered discovery result.
#[derive(Debug)]
pub struct Discovery {
    /// Selected library-like packages.
    pub(crate) packages: Vec<DiscoveredPackage>,
    /// Binary-only packages that cannot be captured.
    pub(crate) skipped_packages: Vec<String>,
}

/// Canonical shared feature policy and per-package Cargo arguments.
#[derive(Debug)]
pub struct RoutedFeatures {
    /// Sorted package-qualified feature policy.
    pub(crate) canonical: SnapshotFeatures,
    /// Local Cargo feature names keyed by package.
    pub(crate) by_package: BTreeMap<String, Vec<String>>,
}

impl Discovery {
    /// Validate shared selectors and route local feature names to packages.
    pub(crate) fn route_features(&self, policy: &SnapshotFeatures) -> Result<RoutedFeatures> {
        let mut by_package = self
            .packages
            .iter()
            .map(|package| (package.package_name.clone(), Vec::new()))
            .collect::<BTreeMap<_, _>>();
        let mut canonical = Vec::new();

        for selector in policy.features() {
            let (package_name, feature) = match selector.split_once('/') {
                Some(parts) => parts,
                None if self.packages.len() == 1 => {
                    (self.packages[0].package_name.as_str(), selector.as_str())
                }
                None => {
                    return Err(RuskelError::SnapshotProfile(format!(
                        "feature '{selector}' is ambiguous; use package/feature for multi-package captures"
                    )));
                }
            };
            let package = self
                .packages
                .iter()
                .find(|package| package.package_name == package_name)
                .ok_or_else(|| {
                    RuskelError::SnapshotProfile(format!(
                        "feature selector '{selector}' names unknown package '{package_name}'"
                    ))
                })?;
            if !package.features.contains(feature) {
                return Err(RuskelError::SnapshotProfile(format!(
                    "package '{package_name}' does not declare feature '{feature}'"
                )));
            }
            canonical.push(format!("{package_name}/{feature}"));
            by_package
                .get_mut(package_name)
                .expect("selected package has a feature bucket")
                .push(feature.to_string());
        }

        for features in by_package.values_mut() {
            features.sort();
            features.dedup();
        }
        let canonical =
            SnapshotFeatures::new(policy.default_features(), policy.all_features(), canonical)?;
        Ok(RoutedFeatures {
            canonical,
            by_package,
        })
    }
}

/// Discover all library-like packages selected by local manifest inputs.
pub fn discover(inputs: &[PathBuf], offline: bool) -> Result<Discovery> {
    let config = create_quiet_cargo_config(offline)?;
    let mut selected = BTreeMap::<PathBuf, DiscoveredPackage>::new();
    let mut skipped = BTreeMap::<PathBuf, String>::new();

    for input in inputs {
        let manifest_path = resolve_manifest(input)?;
        let manifest = cargo_toml::Manifest::from_path(&manifest_path).map_err(|error| {
            discovery_error(input, format!("failed to parse manifest: {error}"))
        })?;
        let workspace = Workspace::new(&manifest_path, &config)
            .map_err(|error| discovery_error(input, convert_cargo_error(&error).to_string()))?;

        if manifest.workspace.is_some() {
            for package in workspace.members() {
                collect_package(package, &mut selected, &mut skipped)?;
            }
        } else {
            let package = workspace.current_opt().ok_or_else(|| {
                discovery_error(
                    input,
                    "manifest does not select a Cargo package".to_string(),
                )
            })?;
            collect_package(package, &mut selected, &mut skipped)?;
        }
    }

    let mut packages = selected.into_values().collect::<Vec<_>>();
    packages.sort_by(|left, right| {
        (&left.package_name, &left.crate_name).cmp(&(&right.package_name, &right.crate_name))
    });
    validate_artifact_names(&packages)?;

    let mut skipped_packages = skipped.into_values().collect::<Vec<_>>();
    skipped_packages.sort();
    if packages.is_empty() {
        return Err(RuskelError::SnapshotDiscovery {
            input: inputs.first().cloned().unwrap_or_default(),
            message: "no library or procedural-macro target remains after discovery".to_string(),
        });
    }
    Ok(Discovery {
        packages,
        skipped_packages,
    })
}

/// Resolve one input into a canonical Cargo manifest path.
fn resolve_manifest(input: &Path) -> Result<PathBuf> {
    let candidate = if input.is_dir() {
        input.join("Cargo.toml")
    } else {
        input.to_path_buf()
    };
    if candidate.file_name().and_then(|name| name.to_str()) != Some("Cargo.toml") {
        return Err(discovery_error(
            input,
            "input must be a Cargo.toml or a directory that contains one".to_string(),
        ));
    }
    fs::canonicalize(&candidate).map_err(|error| {
        discovery_error(
            input,
            format!("cannot resolve '{}': {error}", candidate.display()),
        )
    })
}

/// Add one Cargo package to the deduplicated selected or skipped set.
fn collect_package(
    package: &Package,
    selected: &mut BTreeMap<PathBuf, DiscoveredPackage>,
    skipped: &mut BTreeMap<PathBuf, String>,
) -> Result<()> {
    let manifest_path = fs::canonicalize(package.manifest_path()).map_err(|error| {
        discovery_error(
            package.manifest_path(),
            format!("cannot canonicalize selected manifest: {error}"),
        )
    })?;
    if selected.contains_key(&manifest_path) || skipped.contains_key(&manifest_path) {
        return Ok(());
    }
    let package_name = package.name().to_string();
    let Some(target) = package.library() else {
        skipped.insert(manifest_path, package_name);
        return Ok(());
    };
    let features = package
        .summary()
        .features()
        .keys()
        .map(ToString::to_string)
        .collect();
    selected.insert(
        manifest_path.clone(),
        DiscoveredPackage {
            manifest_path,
            filename: format!("{package_name}.rs"),
            package_name,
            crate_name: target.crate_name(),
            features,
        },
    );
    Ok(())
}

/// Validate all artifact paths before the first rustdoc build.
fn validate_artifact_names(packages: &[DiscoveredPackage]) -> Result<()> {
    let mut exact = BTreeMap::<&str, &str>::new();
    let mut folded = BTreeMap::<String, &str>::new();
    for package in packages {
        let filename = package.filename.as_str();
        if Path::new(filename).components().count() != 1 || filename.contains(['/', '\\']) {
            return Err(RuskelError::SnapshotDiscovery {
                input: package.manifest_path.clone(),
                message: format!(
                    "package '{}' maps to reserved snapshot path '{filename}'",
                    package.package_name
                ),
            });
        }
        if is_platform_reserved(&package.package_name) {
            return Err(RuskelError::SnapshotDiscovery {
                input: package.manifest_path.clone(),
                message: format!(
                    "package '{}' maps to a platform-reserved filename",
                    package.package_name
                ),
            });
        }
        if let Some(existing) = exact.insert(filename, &package.package_name) {
            return Err(collision_error(existing, &package.package_name, filename));
        }
        let key = filename.to_ascii_lowercase();
        if let Some(existing) = folded.insert(key, &package.package_name) {
            return Err(collision_error(existing, &package.package_name, filename));
        }
    }
    Ok(())
}

/// Return whether a package stem is reserved on Windows filesystems.
fn is_platform_reserved(package_name: &str) -> bool {
    let name = package_name.to_ascii_uppercase();
    matches!(name.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            name.strip_prefix(prefix)
                .and_then(|suffix| suffix.parse::<u8>().ok())
                .is_some_and(|number| (1..=9).contains(&number))
        })
}

/// Build one contextual artifact collision error.
fn collision_error(first: &str, second: &str, filename: &str) -> RuskelError {
    RuskelError::SnapshotDiscovery {
        input: PathBuf::from(filename),
        message: format!("packages '{first}' and '{second}' map to colliding snapshot filenames"),
    }
}

/// Build one contextual input discovery error.
fn discovery_error(input: &Path, message: String) -> RuskelError {
    RuskelError::SnapshotDiscovery {
        input: input.to_path_buf(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn write_package(root: &Path, name: &str, extra: &str) {
        fs::create_dir_all(root.join("src")).expect("create fixture source");
        fs::write(root.join("src/lib.rs"), "pub struct Public;\n").expect("write fixture source");
        fs::write(
            root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n{extra}\n"
            ),
        )
        .expect("write fixture manifest");
    }

    #[test]
    fn workspace_discovery_deduplicates_and_sorts() -> Result<()> {
        let root = tempdir()?;
        write_package(&root.path().join("zeta"), "zeta", "");
        write_package(
            &root.path().join("alpha"),
            "alpha",
            "[lib]\nname = \"renamed_alpha\"",
        );
        fs::write(
            root.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"zeta\", \"alpha\"]\nresolver = \"2\"\n",
        )?;

        let discovery = discover(
            &[
                root.path().to_path_buf(),
                root.path().join("alpha/Cargo.toml"),
            ],
            true,
        )?;
        assert_eq!(
            discovery
                .packages
                .iter()
                .map(|package| (&*package.package_name, &*package.crate_name))
                .collect::<Vec<_>>(),
            [("alpha", "renamed_alpha"), ("zeta", "zeta")]
        );
        Ok(())
    }

    #[test]
    fn standalone_manifest_selects_its_package() -> Result<()> {
        let root = tempdir()?;
        write_package(root.path(), "standalone", "publish = false");
        let discovery = discover(&[root.path().join("Cargo.toml")], true)?;
        assert_eq!(discovery.packages.len(), 1);
        assert_eq!(discovery.packages[0].package_name, "standalone");
        Ok(())
    }

    #[test]
    fn binary_members_are_skipped_but_do_not_hide_libraries() -> Result<()> {
        let root = tempdir()?;
        write_package(&root.path().join("library"), "library", "");
        let binary = root.path().join("binary");
        fs::create_dir_all(binary.join("src"))?;
        fs::write(binary.join("src/main.rs"), "fn main() {}\n")?;
        fs::write(
            binary.join("Cargo.toml"),
            "[package]\nname = \"binary\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(
            root.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"library\", \"binary\"]\nresolver = \"2\"\n",
        )?;

        let discovery = discover(&[root.path().to_path_buf()], true)?;
        assert_eq!(discovery.packages[0].package_name, "library");
        assert_eq!(discovery.skipped_packages, ["binary"]);
        assert!(discover(&[binary], true).is_err());
        Ok(())
    }

    #[test]
    fn feature_routing_requires_qualification_for_workspaces() -> Result<()> {
        let package = |name: &str| DiscoveredPackage {
            manifest_path: PathBuf::from(format!("/{name}/Cargo.toml")),
            package_name: name.to_string(),
            crate_name: name.to_string(),
            filename: format!("{name}.rs"),
            features: BTreeSet::from(["serde".to_string()]),
        };
        let discovery = Discovery {
            packages: vec![package("alpha"), package("beta")],
            skipped_packages: Vec::new(),
        };
        assert!(
            discovery
                .route_features(&SnapshotFeatures::new(true, false, vec!["serde".into()])?)
                .is_err()
        );
        let routed = discovery.route_features(&SnapshotFeatures::new(
            true,
            false,
            vec!["beta/serde".into(), "alpha/serde".into()],
        )?)?;
        assert_eq!(routed.canonical.features(), ["alpha/serde", "beta/serde"]);
        assert_eq!(routed.by_package["alpha"], ["serde"]);
        assert!(
            discovery
                .route_features(&SnapshotFeatures::new(
                    true,
                    false,
                    vec!["unknown/serde".into()]
                )?)
                .is_err()
        );
        assert!(
            discovery
                .route_features(&SnapshotFeatures::new(
                    true,
                    false,
                    vec!["alpha/missing".into()]
                )?)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn artifact_names_reject_case_collisions_and_reserved_names() {
        let package = |name: &str| DiscoveredPackage {
            manifest_path: PathBuf::from(format!("/{name}/Cargo.toml")),
            package_name: name.to_string(),
            crate_name: name.replace('-', "_"),
            filename: format!("{name}.rs"),
            features: BTreeSet::new(),
        };
        assert!(validate_artifact_names(&[package("Api"), package("api")]).is_err());
        assert!(validate_artifact_names(&[package("CON")]).is_err());
        assert!(validate_artifact_names(&[package("lpt9")]).is_err());
    }
}
