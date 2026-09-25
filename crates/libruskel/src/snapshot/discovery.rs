use std::{
    collections::{BTreeMap, BTreeSet},
    fs, mem,
    path::{Path, PathBuf},
};

use cargo::core::{Package, Workspace, dependency::DepKind};

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
    pub(crate) features: BTreeSet<String>,
    /// Publication policy from the Cargo manifest.
    pub(crate) publish: String,
    /// Binary targets of this package.
    pub(crate) binaries: Vec<String>,
    /// Direct normal dependencies on workspace packages.
    pub(crate) workspace_dependencies: Vec<WorkspaceDependency>,
    /// Workspace packages that depend on this package.
    pub(crate) workspace_dependents: Vec<WorkspaceDependent>,
    /// Normal dependency aliases mapped to package names.
    pub(crate) dependency_aliases: BTreeMap<String, String>,
}

/// One normal dependency on a selected workspace package.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct WorkspaceDependency {
    /// Depended-on workspace package.
    pub(crate) package: String,
    /// Whether every normal edge to this package is optional.
    pub(crate) optional: bool,
}

/// One selected workspace package that depends on another package.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct WorkspaceDependent {
    /// Dependent workspace package.
    pub(crate) package: String,
    /// Dependency kind used by the dependent.
    pub(crate) kind: DepKind,
}

/// A selected package with no library target.
#[derive(Debug, Clone)]
pub struct BinaryPackage {
    /// Cargo package name.
    pub(crate) name: String,
    /// Direct normal dependencies on workspace packages.
    pub(crate) workspace_dependencies: Vec<WorkspaceDependency>,
}

/// Canonically ordered discovery result.
#[derive(Debug)]
pub struct Discovery {
    /// Selected library-like packages.
    pub(crate) packages: Vec<DiscoveredPackage>,
    /// Binary-only packages that cannot be captured.
    pub(crate) skipped_packages: Vec<String>,
    /// Binary-only package metadata for the index.
    pub(crate) binary_packages: Vec<BinaryPackage>,
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
    let mut skipped = BTreeMap::<PathBuf, BinaryPackage>::new();

    for input in inputs {
        let manifest_path = resolve_manifest(input)?;
        let manifest = cargo_toml::Manifest::from_path(&manifest_path).map_err(|error| {
            discovery_error(input, format!("failed to parse manifest: {error}"))
        })?;
        let workspace = Workspace::new(&manifest_path, &config)
            .map_err(|error| discovery_error(input, convert_cargo_error(&error).to_string()))?;
        let members = workspace.members().collect::<Vec<_>>();

        if manifest.workspace.is_some() {
            for package in &members {
                collect_package(package, &members, &mut selected, &mut skipped)?;
            }
        } else {
            let package = workspace.current_opt().ok_or_else(|| {
                discovery_error(
                    input,
                    "manifest does not select a Cargo package".to_string(),
                )
            })?;
            collect_package(package, &members, &mut selected, &mut skipped)?;
        }
    }

    let mut packages = selected.into_values().collect::<Vec<_>>();
    sort_by_dependencies(&mut packages);
    validate_artifact_names(&packages)?;

    let mut binary_packages = skipped.into_values().collect::<Vec<_>>();
    binary_packages.sort_by(|left, right| left.name.cmp(&right.name));
    let skipped_packages = binary_packages
        .iter()
        .map(|package| package.name.clone())
        .collect();
    if packages.is_empty() {
        return Err(RuskelError::SnapshotDiscovery {
            input: inputs.first().cloned().unwrap_or_default(),
            message: "no library or procedural-macro target remains after discovery".to_string(),
        });
    }
    Ok(Discovery {
        packages,
        skipped_packages,
        binary_packages,
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
    members: &[&Package],
    selected: &mut BTreeMap<PathBuf, DiscoveredPackage>,
    skipped: &mut BTreeMap<PathBuf, BinaryPackage>,
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
    let member_roots = members
        .iter()
        .filter_map(|member| {
            fs::canonicalize(member.root())
                .ok()
                .map(|root| (root, member.name().to_string()))
        })
        .collect::<BTreeMap<_, _>>();
    let workspace_dependencies = workspace_dependencies(package, &member_roots);
    let Some(target) = package.library() else {
        skipped.insert(
            manifest_path,
            BinaryPackage {
                name: package_name,
                workspace_dependencies,
            },
        );
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
            publish: match package.publish() {
                None => "crates.io".to_string(),
                Some(registries) if registries.is_empty() => "no".to_string(),
                Some(registries) => {
                    let mut registries = registries.clone();
                    registries.sort();
                    registries.join(", ")
                }
            },
            binaries: {
                let mut names = package
                    .targets()
                    .iter()
                    .filter(|target| target.is_bin())
                    .map(|target| target.name().to_string())
                    .collect::<Vec<_>>();
                names.sort();
                names
            },
            workspace_dependencies,
            workspace_dependents: workspace_dependents(package, members),
            dependency_aliases: package
                .dependencies()
                .iter()
                .filter(|dependency| dependency.kind() == DepKind::Normal)
                .map(|dependency| {
                    (
                        dependency.name_in_toml().replace('-', "_"),
                        dependency.package_name().to_string(),
                    )
                })
                .collect(),
        },
    );
    Ok(())
}

/// Find direct normal edges from one package to selected workspace members.
fn workspace_dependencies(
    package: &Package,
    member_roots: &BTreeMap<PathBuf, String>,
) -> Vec<WorkspaceDependency> {
    let mut by_package = BTreeMap::<String, bool>::new();
    for dependency in package
        .dependencies()
        .iter()
        .filter(|dependency| dependency.kind() == DepKind::Normal)
    {
        let Some(root) = dependency
            .source_id()
            .local_path()
            .and_then(|root| fs::canonicalize(root).ok())
        else {
            continue;
        };
        let Some(name) = member_roots.get(&root) else {
            continue;
        };
        by_package
            .entry(name.clone())
            .and_modify(|optional| *optional &= dependency.is_optional())
            .or_insert(dependency.is_optional());
    }
    by_package
        .into_iter()
        .map(|(package, optional)| WorkspaceDependency { package, optional })
        .collect()
}

/// Find selected workspace packages that depend on this package.
fn workspace_dependents(package: &Package, members: &[&Package]) -> Vec<WorkspaceDependent> {
    let package_root = fs::canonicalize(package.root()).ok();
    let mut dependents = members
        .iter()
        .flat_map(|member| {
            member
                .dependencies()
                .iter()
                .filter(|&dependency| {
                    dependency.package_name().as_str() == package.name().as_str()
                        && package_root.as_ref().is_some_and(|package_root| {
                            dependency
                                .source_id()
                                .local_path()
                                .and_then(|root| fs::canonicalize(root).ok())
                                .as_ref()
                                == Some(package_root)
                        })
                })
                .map(|dependency| WorkspaceDependent {
                    package: member.name().to_string(),
                    kind: dependency.kind(),
                })
        })
        .collect::<Vec<_>>();
    dependents.sort();
    dependents.dedup();
    let normal = dependents
        .iter()
        .filter(|dependent| dependent.kind == DepKind::Normal)
        .map(|dependent| dependent.package.clone())
        .collect::<BTreeSet<_>>();
    dependents.retain(|dependent| {
        dependent.kind == DepKind::Normal || !normal.contains(&dependent.package)
    });
    dependents
}

/// Sort packages so each normal workspace dependency precedes its consumer.
fn sort_by_dependencies(packages: &mut Vec<DiscoveredPackage>) {
    let mut ordered = Vec::with_capacity(packages.len());
    let mut pending = mem::take(packages);
    pending.sort_by(|left, right| left.package_name.cmp(&right.package_name));
    while !pending.is_empty() {
        let next = pending
            .iter()
            .position(|package| {
                !package.workspace_dependencies.iter().any(|dependency| {
                    pending
                        .iter()
                        .any(|other| other.package_name == dependency.package)
                })
            })
            .unwrap_or(0);
        ordered.push(pending.remove(next));
    }
    *packages = ordered;
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
    use crate::snapshot::card;

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
    fn workspace_edges_and_cards_use_only_member_packages() -> Result<()> {
        let root = tempdir()?;
        let workspace = root.path().join("workspace");
        fs::create_dir(&workspace)?;
        write_package(&root.path().join("external"), "external", "");
        write_package(&workspace.join("core"), "core", "");
        write_package(
            &workspace.join("app"),
            "app",
            "publish = false\n[lib]\nname = \"app_library\"\n[dependencies]\ncore_alias = { package = \"core\", path = \"../core\", optional = true }\nexternal = { path = \"../../external\" }",
        );
        write_package(
            &workspace.join("dev"),
            "dev",
            "[dev-dependencies]\ncore = { path = \"../core\" }",
        );
        let binary = workspace.join("tool");
        fs::create_dir_all(binary.join("src"))?;
        fs::write(binary.join("src/main.rs"), "fn main() {}\n")?;
        fs::write(
            binary.join("Cargo.toml"),
            "[package]\nname = \"tool\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[dependencies]\napp = { path = \"../app\" }\n",
        )?;
        fs::write(
            workspace.join("Cargo.toml"),
            "[workspace]\nmembers = [\"core\", \"app\", \"dev\", \"tool\"]\nresolver = \"3\"\n",
        )?;

        let discovery = discover(&[workspace], true)?;
        assert_eq!(
            discovery
                .packages
                .iter()
                .map(|package| package.package_name.as_str())
                .collect::<Vec<_>>(),
            ["core", "app", "dev"]
        );
        let core = &discovery.packages[0];
        assert_eq!(core.publish, "crates.io");
        assert_eq!(
            core.workspace_dependents,
            [
                WorkspaceDependent {
                    package: "app".into(),
                    kind: DepKind::Normal
                },
                WorkspaceDependent {
                    package: "dev".into(),
                    kind: DepKind::Development
                },
            ]
        );
        let app = &discovery.packages[1];
        assert_eq!(app.publish, "no");
        assert_eq!(app.crate_name, "app_library");
        assert_eq!(
            app.workspace_dependencies,
            [WorkspaceDependency {
                package: "core".into(),
                optional: true
            }]
        );
        assert_eq!(app.dependency_aliases["core_alias"], "core");
        assert_eq!(app.dependency_aliases["external"], "external");
        let card = card::crate_card(app, &[]);
        assert!(card.contains("// Workspace dependencies: core (optional)\n"));
        assert!(!card.contains("Workspace dependencies: external"));
        assert_eq!(discovery.binary_packages[0].name, "tool");
        assert_eq!(
            discovery.binary_packages[0].workspace_dependencies[0].package,
            "app"
        );
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
            publish: "crates.io".into(),
            binaries: Vec::new(),
            workspace_dependencies: Vec::new(),
            workspace_dependents: Vec::new(),
            dependency_aliases: BTreeMap::new(),
        };
        let discovery = Discovery {
            packages: vec![package("alpha"), package("beta")],
            skipped_packages: Vec::new(),
            binary_packages: Vec::new(),
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
            publish: "crates.io".into(),
            binaries: Vec::new(),
            workspace_dependencies: Vec::new(),
            workspace_dependents: Vec::new(),
            dependency_aliases: BTreeMap::new(),
        };
        assert!(validate_artifact_names(&[package("Api"), package("api")]).is_err());
        assert!(validate_artifact_names(&[package("CON")]).is_err());
        assert!(validate_artifact_names(&[package("lpt9")]).is_err());
    }
}
