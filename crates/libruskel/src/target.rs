//! Target parsing helpers for user-provided specifications.

use std::path::PathBuf;

use semver::Version;

use crate::error::{Result, RuskelError};

/// Entry point for resolving a target specification.
#[derive(Debug, Clone, PartialEq)]
pub enum Entrypoint {
    /// A path to a Rust file or directory.
    Path(PathBuf),
    /// A module or package name, optionally with a version.
    Name {
        /// Package or module name provided by the user.
        name: String,
        /// Optional package version requested with the target.
        version: Option<Version>,
    },
}

/// A parsed target specification for the ruskel tool.
///
/// A target specification consists of an entrypoint and an optional path, separated by `::`.
///
/// # Format
///
/// ```text
/// entrypoint[::path]
/// ```
///
/// Where:
/// - `entrypoint` can be a file path, directory path, module name, or package name.
/// - `path` is an optional fully qualified path within the entrypoint.
///
/// Package names may include an `@version` suffix.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    /// Entry point describing where to start resolving the target.
    pub entrypoint: Entrypoint,
    /// Optional module path components within the entrypoint.
    pub path: Vec<String>,
}

impl Target {
    /// Parse a target specification string into a structured `Target`.
    pub fn parse(spec: &str) -> Result<Self> {
        let (entrypoint, path) = split_target_spec(spec)?;
        Ok(Self {
            entrypoint: parse_entrypoint(entrypoint)?,
            path: collect_path_components(path),
        })
    }
}

/// Split a target string into its entrypoint and remaining path components.
fn split_target_spec(spec: &str) -> Result<(&str, Vec<&str>)> {
    if spec.is_empty() {
        return Err(invalid_target("Invalid target specification: empty string"));
    }

    let mut parts = spec.split("::");
    let entrypoint = parts
        .next()
        .ok_or_else(|| invalid_target("Invalid target specification: empty string"))?;
    if entrypoint.is_empty() {
        return Err(invalid_target("Invalid name specification: empty name"));
    }

    let path: Vec<&str> = parts.collect();
    validate_path_components(&path)?;
    Ok((entrypoint, path))
}

/// Reject empty path segments so downstream resolution can assume valid components.
fn validate_path_components(path: &[&str]) -> Result<()> {
    for (index, component) in path.iter().enumerate() {
        if component.is_empty() {
            return Err(invalid_target(format!(
                "Invalid target specification: empty path component at position {}",
                index + 1
            )));
        }
    }

    Ok(())
}

/// Materialize borrowed path components into owned strings for the parsed target.
fn collect_path_components(path: Vec<&str>) -> Vec<String> {
    path.into_iter().map(str::to_owned).collect()
}

/// Parse the first component of a target as either a path or a named crate/module.
fn parse_entrypoint(entrypoint: &str) -> Result<Entrypoint> {
    if is_path_entrypoint(entrypoint) {
        return Ok(Entrypoint::Path(PathBuf::from(entrypoint)));
    }

    parse_name_entrypoint(entrypoint)
}

/// Determine whether the target entrypoint should be treated as a filesystem path.
fn is_path_entrypoint(entrypoint: &str) -> bool {
    entrypoint.contains('/') || entrypoint.contains('\\') || matches!(entrypoint, "." | "..")
}

/// Parse a non-path entrypoint, including optional `@version` suffixes.
fn parse_name_entrypoint(entrypoint: &str) -> Result<Entrypoint> {
    let Some((name, version)) = entrypoint.split_once('@') else {
        return Ok(Entrypoint::Name {
            name: entrypoint.to_string(),
            version: None,
        });
    };

    if name.is_empty() || version.is_empty() || version.contains('@') {
        return Err(invalid_target(format!(
            "Invalid name specification: {entrypoint}"
        )));
    }

    let version = Version::parse(version)
        .map_err(|error| invalid_target(format!("Invalid version: {error}")))?;

    Ok(Entrypoint::Name {
        name: name.to_string(),
        version: Some(version),
    })
}

/// Construct a target-parsing error with the standard variant used by this module.
fn invalid_target(message: impl Into<String>) -> RuskelError {
    RuskelError::InvalidTarget(message.into())
}

#[cfg(test)]
mod tests {
    use super::{Entrypoint, Target};
    use crate::error::{Result, RuskelError};

    fn name_target(name: &str, version: Option<&str>, path: &[&str]) -> Target {
        Target {
            entrypoint: Entrypoint::Name {
                name: name.to_string(),
                version: version.map(|value| value.parse().expect("valid test version")),
            },
            path: path
                .iter()
                .map(|component| (*component).to_string())
                .collect(),
        }
    }

    fn path_target(path: &str, components: &[&str]) -> Target {
        Target {
            entrypoint: Entrypoint::Path(path.into()),
            path: components
                .iter()
                .map(|component| (*component).to_string())
                .collect(),
        }
    }

    fn assert_invalid_target(input: &str, expected: &str) {
        let error = Target::parse(input).expect_err("target should be rejected");
        assert_eq!(
            error.to_string(),
            RuskelError::InvalidTarget(expected.to_string()).to_string()
        );
    }

    #[test]
    fn rejects_empty_spec() {
        assert_invalid_target("", "Invalid target specification: empty string");
    }

    #[test]
    fn rejects_empty_name() {
        assert_invalid_target("::", "Invalid name specification: empty name");
    }

    #[test]
    fn parses_relative_path_entrypoint() -> Result<()> {
        let target = Target::parse("src/main.rs::my_module::MyStruct")?;
        assert_eq!(
            target,
            path_target("src/main.rs", &["my_module", "MyStruct"])
        );
        Ok(())
    }

    #[test]
    fn parses_absolute_path_entrypoint() -> Result<()> {
        let target = Target::parse("/path/to/my_project::some_module::function")?;
        assert_eq!(
            target,
            path_target("/path/to/my_project", &["some_module", "function"])
        );
        Ok(())
    }

    #[test]
    fn treats_current_and_parent_directory_as_paths() -> Result<()> {
        assert_eq!(Target::parse(".")?, path_target(".", &[]));
        assert_eq!(Target::parse("..")?, path_target("..", &[]));
        Ok(())
    }

    #[test]
    fn parses_plain_package_name() -> Result<()> {
        let target = Target::parse("serde::Deserialize")?;
        assert_eq!(target, name_target("serde", None, &["Deserialize"]));
        Ok(())
    }

    #[test]
    fn parses_versioned_package_name() -> Result<()> {
        let target = Target::parse("serde@1.0.104::Serialize")?;
        assert_eq!(
            target,
            name_target("serde", Some("1.0.104"), &["Serialize"])
        );
        Ok(())
    }

    #[test]
    fn preserves_hyphenated_package_names() -> Result<()> {
        let target = Target::parse("tracing-test")?;
        assert_eq!(target, name_target("tracing-test", None, &[]));
        Ok(())
    }

    #[test]
    fn rejects_trailing_separator() {
        assert_invalid_target(
            "foo::",
            "Invalid target specification: empty path component at position 1",
        );
    }

    #[test]
    fn rejects_empty_path_component_in_the_middle() {
        assert_invalid_target(
            "foo::::bar",
            "Invalid target specification: empty path component at position 1",
        );
    }

    #[test]
    fn rejects_missing_version_after_at_sign() {
        assert_invalid_target("serde@", "Invalid name specification: serde@");
    }

    #[test]
    fn rejects_multiple_at_signs_in_name_entrypoint() {
        assert_invalid_target(
            "serde@1.0.0@beta",
            "Invalid name specification: serde@1.0.0@beta",
        );
    }

    #[test]
    fn rejects_invalid_semver_versions() {
        let error = Target::parse("serde@invalid").expect_err("version should be rejected");
        assert!(
            error
                .to_string()
                .starts_with("Invalid target: Invalid version: "),
            "unexpected error message: {error}"
        );
    }
}
