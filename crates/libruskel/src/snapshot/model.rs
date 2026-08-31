use std::{path::PathBuf, process::Command};

use crate::{
    error::{Result, RuskelError},
    toolchain::{active_dated_nightly, host_target, normalize_dated_nightly, toolchain_binary},
};

/// Cargo feature policy shared by every crate in one snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotFeatures {
    /// Whether Cargo enables default features.
    default_features: bool,
    /// Whether Cargo enables all features.
    all_features: bool,
    /// Sorted selected feature names.
    features: Vec<String>,
}

impl SnapshotFeatures {
    /// Validate and canonicalize one snapshot feature policy.
    pub fn new(
        default_features: bool,
        all_features: bool,
        mut features: Vec<String>,
    ) -> Result<Self> {
        if all_features && (!default_features || !features.is_empty()) {
            return Err(RuskelError::SnapshotProfile(
                "all features cannot be combined with disabled default features or selected features"
                    .to_string(),
            ));
        }
        if features.iter().any(|feature| {
            feature.is_empty()
                || feature.split_once('/').is_some_and(|(package, local)| {
                    package.is_empty() || local.is_empty() || local.contains('/')
                })
        }) {
            return Err(RuskelError::SnapshotProfile(
                "feature selectors must be FEATURE or PACKAGE/FEATURE".to_string(),
            ));
        }
        features.sort();
        features.dedup();
        Ok(Self {
            default_features: all_features || default_features,
            all_features,
            features,
        })
    }

    /// Return whether Cargo enables each package's default features.
    pub fn default_features(&self) -> bool {
        self.default_features
    }

    /// Return whether Cargo enables every feature.
    pub fn all_features(&self) -> bool {
        self.all_features
    }

    /// Return the sorted selected feature list.
    pub fn features(&self) -> &[String] {
        &self.features
    }
}

impl Default for SnapshotFeatures {
    fn default() -> Self {
        Self {
            default_features: true,
            all_features: false,
            features: Vec::new(),
        }
    }
}

/// Optional first-capture or profile-migration overrides.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapshotProfileOptions {
    /// Explicit portable toolchain override.
    toolchain: Option<String>,
    /// Explicit target override.
    target: Option<String>,
    /// Explicit shared feature policy override.
    features: Option<SnapshotFeatures>,
}

impl SnapshotProfileOptions {
    /// Create options with no explicit overrides.
    pub fn new() -> Self {
        Self::default()
    }

    /// Select a portable dated-nightly toolchain.
    pub fn with_toolchain(mut self, toolchain: impl Into<String>) -> Self {
        self.toolchain = Some(toolchain.into());
        self
    }

    /// Select one Cargo target triple.
    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    /// Select the shared Cargo feature policy.
    pub fn with_features(mut self, features: SnapshotFeatures) -> Self {
        self.features = Some(features);
        self
    }

    /// Return the explicit toolchain override.
    pub fn toolchain(&self) -> Option<&str> {
        self.toolchain.as_deref()
    }

    /// Return the explicit target override.
    pub fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }

    /// Return the explicit feature-policy override.
    pub fn features(&self) -> Option<&SnapshotFeatures> {
        self.features.as_ref()
    }
}

/// Fully resolved capture profile stored with one snapshot tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotProfile {
    /// Snapshot format version.
    format: u32,
    /// Portable dated-nightly toolchain.
    toolchain: String,
    /// Cargo target triple.
    target: String,
    /// Shared Cargo feature policy.
    features: SnapshotFeatures,
}

impl SnapshotProfile {
    /// Snapshot format implemented by this release.
    pub const FORMAT: u32 = 1;

    /// Validate a complete format 1 profile against the local Rust
    /// installation.
    pub fn new(
        toolchain: impl Into<String>,
        target: impl Into<String>,
        features: SnapshotFeatures,
    ) -> Result<Self> {
        let profile = Self {
            format: Self::FORMAT,
            toolchain: toolchain.into(),
            target: target.into(),
            features,
        };
        profile.validate()?;
        Ok(profile)
    }

    /// Resolve stored values and explicit overrides as one atomic profile.
    pub fn resolve(stored: Option<&Self>, options: SnapshotProfileOptions) -> Result<Self> {
        let values = resolve_profile_values(stored, options, active_dated_nightly, host_target)?;
        Self::new(values.0, values.1, values.2)
    }

    /// Return the snapshot format version.
    pub fn format(&self) -> u32 {
        self.format
    }

    /// Return the portable dated-nightly toolchain.
    pub fn toolchain(&self) -> &str {
        &self.toolchain
    }

    /// Return the selected Cargo target triple.
    pub fn target(&self) -> &str {
        &self.target
    }

    /// Return the shared Cargo feature policy.
    pub fn features(&self) -> &SnapshotFeatures {
        &self.features
    }

    /// Replace caller selectors with their package-qualified canonical form.
    pub(crate) fn with_features(&self, features: SnapshotFeatures) -> Self {
        Self {
            features,
            ..self.clone()
        }
    }

    /// Build a profile from a validated format marker without inspecting the
    /// local Rust installation.
    pub(crate) fn from_marker(
        format: u32,
        toolchain: String,
        target: String,
        features: SnapshotFeatures,
    ) -> Result<Self> {
        if format != Self::FORMAT {
            return Err(RuskelError::SnapshotProfile(format!(
                "snapshot format {format} is not supported"
            )));
        }
        if normalize_dated_nightly(&toolchain).as_deref() != Some(toolchain.as_str()) {
            return Err(RuskelError::SnapshotProfile(format!(
                "stored toolchain '{toolchain}' is not a portable dated nightly"
            )));
        }
        if target.trim().is_empty() {
            return Err(RuskelError::SnapshotProfile(
                "stored target triple cannot be empty".to_string(),
            ));
        }
        Ok(Self {
            format,
            toolchain,
            target,
            features,
        })
    }

    /// Validate all selected profile values after resolution is complete.
    fn validate(&self) -> Result<()> {
        if normalize_dated_nightly(&self.toolchain).as_deref() != Some(self.toolchain.as_str()) {
            return Err(RuskelError::SnapshotProfile(format!(
                "toolchain '{}' is not portable; use nightly-YYYY-MM-DD",
                self.toolchain
            )));
        }
        if self.target.trim().is_empty() {
            return Err(RuskelError::SnapshotProfile(
                "target triple cannot be empty".to_string(),
            ));
        }
        toolchain_binary(&self.toolchain, "rustfmt")
            .map_err(|error| RuskelError::SnapshotProfile(format!("{}", error)))?;
        ensure_target_installed(&self.toolchain, &self.target)?;
        Ok(())
    }
}

/// Profile fields selected before local installation validation.
type ProfileValues = (String, String, SnapshotFeatures);

/// Select all stored, explicit, and environment-derived profile values.
fn resolve_profile_values<A, H>(
    stored: Option<&SnapshotProfile>,
    options: SnapshotProfileOptions,
    active_toolchain: A,
    compiler_host: H,
) -> Result<ProfileValues>
where
    A: FnOnce() -> Result<String>,
    H: FnOnce(&str) -> Result<String>,
{
    let toolchain = match options.toolchain {
        Some(toolchain) => toolchain,
        None => stored
            .map(|profile| profile.toolchain.clone())
            .map(Ok)
            .unwrap_or_else(active_toolchain)?,
    };
    let target = match options.target {
        Some(target) => target,
        None => match stored {
            Some(profile) => profile.target.clone(),
            None => compiler_host(&toolchain)?,
        },
    };
    let features = options
        .features
        .or_else(|| stored.map(|profile| profile.features.clone()))
        .unwrap_or_default();
    Ok((toolchain, target, features))
}

/// Require the selected target in the selected rustup toolchain.
fn ensure_target_installed(toolchain: &str, target: &str) -> Result<()> {
    let output = Command::new("rustup")
        .args(["target", "list", "--installed", "--toolchain", toolchain])
        .output()
        .map_err(|error| {
            RuskelError::SnapshotProfile(format!(
                "failed to inspect targets for toolchain '{toolchain}': {error}"
            ))
        })?;
    if !output.status.success() {
        return Err(RuskelError::SnapshotProfile(format!(
            "toolchain '{toolchain}' is not available. Run: rustup toolchain install {toolchain}"
        )));
    }
    let installed = String::from_utf8_lossy(&output.stdout);
    if !installed.lines().any(|line| line.trim() == target) {
        return Err(RuskelError::SnapshotProfile(format!(
            "target '{target}' is not installed for toolchain '{toolchain}'. Run: rustup target add --toolchain {toolchain} {target}"
        )));
    }
    Ok(())
}

/// Inputs and resolved profile for one in-memory capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotRequest {
    /// Caller-provided local input paths.
    inputs: Vec<PathBuf>,
    /// Complete capture profile.
    profile: SnapshotProfile,
}

impl SnapshotRequest {
    /// Create a request for one or more local Cargo manifests or directories.
    pub fn new(inputs: Vec<PathBuf>, profile: SnapshotProfile) -> Result<Self> {
        if inputs.is_empty() {
            return Err(RuskelError::SnapshotDiscovery {
                input: PathBuf::new(),
                message: "at least one input is required".to_string(),
            });
        }
        Ok(Self { inputs, profile })
    }

    /// Return the input paths in caller order.
    pub fn inputs(&self) -> &[PathBuf] {
        &self.inputs
    }

    /// Return the resolved capture profile.
    pub fn profile(&self) -> &SnapshotProfile {
        &self.profile
    }
}

/// One canonical generated crate file held in memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrateSnapshot {
    /// Cargo package name.
    pub(crate) package: String,
    /// Normalized Rust crate name.
    pub(crate) crate_name: String,
    /// Relative generated filename.
    pub(crate) filename: String,
    /// Complete canonical crate source.
    pub(crate) contents: String,
}

impl CrateSnapshot {
    /// Return the Cargo package name.
    pub fn package(&self) -> &str {
        &self.package
    }

    /// Return the Rust library crate name.
    pub fn crate_name(&self) -> &str {
        &self.crate_name
    }

    /// Return the generated filename relative to the snapshot root.
    pub fn filename(&self) -> &str {
        &self.filename
    }

    /// Return the complete generated Rust source.
    pub fn contents(&self) -> &str {
        &self.contents
    }
}

/// Complete ordered snapshot captured without destination I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiSnapshot {
    /// Canonical profile used by every crate.
    pub(crate) profile: SnapshotProfile,
    /// Canonically ordered generated crates.
    pub(crate) crates: Vec<CrateSnapshot>,
    /// Canonically ordered binary-only package names.
    pub(crate) skipped_packages: Vec<String>,
}

impl ApiSnapshot {
    /// Return the canonical profile used for this capture.
    pub fn profile(&self) -> &SnapshotProfile {
        &self.profile
    }

    /// Return generated crate files in canonical package order.
    pub fn crates(&self) -> &[CrateSnapshot] {
        &self.crates
    }

    /// Return binary-only package names in canonical order.
    pub fn skipped_packages(&self) -> &[String] {
        &self.skipped_packages
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn unvalidated_profile(toolchain: &str, target: &str) -> SnapshotProfile {
        SnapshotProfile {
            format: SnapshotProfile::FORMAT,
            toolchain: toolchain.to_string(),
            target: target.to_string(),
            features: SnapshotFeatures::default(),
        }
    }

    #[test]
    fn feature_forms_are_canonical() -> Result<()> {
        assert_eq!(SnapshotFeatures::default().features(), &[] as &[String]);
        let selected =
            SnapshotFeatures::new(false, false, vec!["z".into(), "a".into(), "a".into()])?;
        assert_eq!(selected.features(), &["a", "z"]);
        assert!(!selected.default_features());
        let all = SnapshotFeatures::new(true, true, Vec::new())?;
        assert!(all.default_features());
        assert!(all.all_features());
        assert!(SnapshotFeatures::new(false, true, Vec::new()).is_err());
        assert!(SnapshotFeatures::new(true, true, vec!["one".into()]).is_err());
        Ok(())
    }

    #[test]
    fn explicit_toolchain_does_not_inspect_active_toolchain() -> Result<()> {
        for active_name in ["stable-aarch64-apple-darwin", "beta-aarch64-apple-darwin"] {
            let active_called = Cell::new(false);
            let values = resolve_profile_values(
                None,
                SnapshotProfileOptions::new()
                    .with_toolchain("nightly-2026-07-01")
                    .with_target("aarch64-apple-darwin"),
                || {
                    active_called.set(true);
                    Err(RuskelError::SnapshotProfile(format!(
                        "active {active_name}"
                    )))
                },
                |_| Err(RuskelError::SnapshotProfile("host queried".into())),
            )?;
            assert_eq!(values.0, "nightly-2026-07-01");
            assert_eq!(values.1, "aarch64-apple-darwin");
            assert!(!active_called.get());
        }
        Ok(())
    }

    #[test]
    fn migration_preserves_stored_target() -> Result<()> {
        let stored = unvalidated_profile("nightly-2026-06-01", "x86_64-unknown-linux-gnu");
        let values = resolve_profile_values(
            Some(&stored),
            SnapshotProfileOptions::new().with_toolchain("nightly-2026-07-01"),
            || unreachable!(),
            |_| unreachable!(),
        )?;
        assert_eq!(values.0, "nightly-2026-07-01");
        assert_eq!(values.1, "x86_64-unknown-linux-gnu");
        Ok(())
    }
}
