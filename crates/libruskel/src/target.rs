use semver::Version;
use std::path::PathBuf;

use crate::error::{Result, RuskelError};

#[derive(Debug, Clone, PartialEq)]
pub enum Entrypoint {
    /// A path to a Rust file or directory.
    Path(PathBuf),
    /// A module or package name, optionally with a version.
    Name {
        name: String,
        version: Option<Version>,
    },
}

/// A parsed target specification for the ruskel tool.
///
/// A target specification consists of an entrypoint and an optional path, separated by '::'.
///
/// # Format
///
/// The general format is:
///
/// ```text
/// entrypoint[::path]
/// ```
///
/// Where:
/// - `entrypoint` can be a file path, directory path, module name, or package name (optionally with a version).
/// - `path` is an optional fully qualified path within the entrypoint, with components separated by '::'.
///
/// # Entrypoint Types
///
/// - **File Path**: A path to a Rust file
/// - **Directory Path**: A path to a directory containing a Cargo.toml file
/// - **Module**: A module name, typically starting with an uppercase letter
/// - **Package**: A package name, optionally followed by '@' and a version number
///
/// # Examples of valid target specifications:
///
/// - File paths:
///   - `src/lib.rs`
///   - `src/main.rs::my_module::MyStruct`
///
/// - Directory paths:
///   - `/path/to/my_project`
///   - `/path/to/my_project::some_module::function`
///
/// - Modules:
///   - `MyModule`
///   - `MyModule::SubModule::function`
///
/// - Packages:
///   - `serde`
///   - `serde::Deserialize`
///   - `serde@1.0.104`
///   - `serde@1.0.104::Serialize`
///
/// - Other examples:
///   - `tokio::sync::Mutex`
///   - `std::collections::HashMap`
///   - `my_crate::utils::helper_function`
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub entrypoint: Entrypoint,
    pub path: Vec<String>,
}

impl Target {
    pub fn parse(spec: &str) -> Result<Self> {
        if spec.is_empty() {
            return Err(RuskelError::InvalidTarget(
                "Invalid target specification: empty string".to_string(),
            ));
        }

        let parts: Vec<&str> = spec.split("::").collect();

        if parts[0].is_empty() {
            return Err(RuskelError::InvalidTarget(
                "Invalid name specification: empty name".to_string(),
            ));
        }

        let (entrypoint, path) = parts.split_first().unwrap();

        // Check for empty path components
        for (i, component) in path.iter().enumerate() {
            if component.is_empty() {
                return Err(RuskelError::InvalidTarget(format!(
                    "Invalid target specification: empty path component at position {}",
                    i + 1
                )));
            }
        }

        let entrypoint = if entrypoint.contains('/')
            || entrypoint.contains('\\')
            || *entrypoint == "."
            || *entrypoint == ".."
        {
            // It's a file or directory path
            Entrypoint::Path(PathBuf::from(entrypoint))
        } else if entrypoint.contains('@') {
            // It's a name with version
            let name_parts: Vec<&str> = entrypoint.split('@').collect();
            if name_parts.len() != 2 {
                return Err(RuskelError::InvalidTarget(format!(
                    "Invalid name specification: {entrypoint}"
                )));
            }
            let name = name_parts[0].to_string();
            let version = Version::parse(name_parts[1])
                .map_err(|e| RuskelError::InvalidTarget(format!("Invalid version: {e}")))?;
            Entrypoint::Name {
                name,
                version: Some(version),
            }
        } else {
            // It's a name without version
            Entrypoint::Name {
                name: entrypoint.to_string(),
                version: None,
            }
        };

        Ok(Target {
            entrypoint,
            path: path.iter().map(|&s| s.to_string()).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_targets() {
        let test_cases = vec![
            // Empty target (invalid)
            (
                "",
                Err(RuskelError::InvalidTarget(
                    "Invalid target specification: empty string".to_string(),
                )),
            ),
            // Double colon (::) should be treated as an error
            (
                "::",
                Err(RuskelError::InvalidTarget(
                    "Invalid name specification: empty name".to_string(),
                )),
            ),
            // Paths
            (
                "src/lib.rs",
                Ok(Target {
                    entrypoint: Entrypoint::Path(PathBuf::from("src/lib.rs")),
                    path: vec![],
                }),
            ),
            (
                "src/main.rs::my_module::MyStruct",
                Ok(Target {
                    entrypoint: Entrypoint::Path(PathBuf::from("src/main.rs")),
                    path: vec!["my_module".to_string(), "MyStruct".to_string()],
                }),
            ),
            (
                "/path/to/my_project",
                Ok(Target {
                    entrypoint: Entrypoint::Path(PathBuf::from("/path/to/my_project")),
                    path: vec![],
                }),
            ),
            (
                "/path/to/my_project::some_module::function",
                Ok(Target {
                    entrypoint: Entrypoint::Path(PathBuf::from("/path/to/my_project")),
                    path: vec!["some_module".to_string(), "function".to_string()],
                }),
            ),
            // Names (Modules or Packages)
            (
                "MyModule",
                Ok(Target {
                    entrypoint: Entrypoint::Name {
                        name: "MyModule".to_string(),
                        version: None,
                    },
                    path: vec![],
                }),
            ),
            (
                "MyModule::SubModule::function",
                Ok(Target {
                    entrypoint: Entrypoint::Name {
                        name: "MyModule".to_string(),
                        version: None,
                    },
                    path: vec!["SubModule".to_string(), "function".to_string()],
                }),
            ),
            (
                "serde",
                Ok(Target {
                    entrypoint: Entrypoint::Name {
                        name: "serde".to_string(),
                        version: None,
                    },
                    path: vec![],
                }),
            ),
            (
                "serde::Deserialize",
                Ok(Target {
                    entrypoint: Entrypoint::Name {
                        name: "serde".to_string(),
                        version: None,
                    },
                    path: vec!["Deserialize".to_string()],
                }),
            ),
            (
                "serde@1.0.104",
                Ok(Target {
                    entrypoint: Entrypoint::Name {
                        name: "serde".to_string(),
                        version: Some(Version::parse("1.0.104").unwrap()),
                    },
                    path: vec![],
                }),
            ),
            (
                "serde@1.0.104::Serialize",
                Ok(Target {
                    entrypoint: Entrypoint::Name {
                        name: "serde".to_string(),
                        version: Some(Version::parse("1.0.104").unwrap()),
                    },
                    path: vec!["Serialize".to_string()],
                }),
            ),
            // Complex paths
            (
                "tokio::sync::Mutex",
                Ok(Target {
                    entrypoint: Entrypoint::Name {
                        name: "tokio".to_string(),
                        version: None,
                    },
                    path: vec!["sync".to_string(), "Mutex".to_string()],
                }),
            ),
            (
                "std::collections::HashMap",
                Ok(Target {
                    entrypoint: Entrypoint::Name {
                        name: "std".to_string(),
                        version: None,
                    },
                    path: vec!["collections".to_string(), "HashMap".to_string()],
                }),
            ),
            (
                "my_crate::utils::helper_function",
                Ok(Target {
                    entrypoint: Entrypoint::Name {
                        name: "my_crate".to_string(),
                        version: None,
                    },
                    path: vec!["utils".to_string(), "helper_function".to_string()],
                }),
            ),
            (
                "tracing-test",
                Ok(Target {
                    entrypoint: Entrypoint::Name {
                        name: "tracing-test".to_string(),
                        version: None,
                    },
                    path: vec![],
                }),
            ),
            // Invalid targets
            (
                "serde@",
                Err(RuskelError::InvalidTarget("Invalid version: ".to_string())),
            ),
            (
                "serde@invalid",
                Err(RuskelError::InvalidTarget("Invalid version: ".to_string())),
            ),
            // Trailing :: should be an error
            (
                "foo::",
                Err(RuskelError::InvalidTarget(
                    "Invalid target specification: empty path component at position 1".to_string(),
                )),
            ),
            (
                "foo::bar::",
                Err(RuskelError::InvalidTarget(
                    "Invalid target specification: empty path component at position 2".to_string(),
                )),
            ),
            // Multiple consecutive :: should also be errors
            (
                "foo::::bar",
                Err(RuskelError::InvalidTarget(
                    "Invalid target specification: empty path component at position 1".to_string(),
                )),
            ),
            // Current directory and parent directory
            (
                ".",
                Ok(Target {
                    entrypoint: Entrypoint::Path(PathBuf::from(".")),
                    path: vec![],
                }),
            ),
            (
                "..",
                Ok(Target {
                    entrypoint: Entrypoint::Path(PathBuf::from("..")),
                    path: vec![],
                }),
            ),
        ];

        for (input, expected_output) in test_cases {
            let result = Target::parse(input);
            match (&result, &expected_output) {
                (Ok(target), Ok(expected_target)) => {
                    assert_eq!(
                        target, expected_target,
                        "Mismatch for input '{input}'. \nGot: {target:?}\nExpected: {expected_target:?}"
                    );
                }
                (Err(error), Err(expected_error)) => {
                    assert!(
                        error.to_string().starts_with(&expected_error.to_string()),
                        "Error mismatch for input '{input}'. \nGot: {error}\nExpected error starting with: {expected_error}"
                    );
                }
                (Ok(target), Err(expected_error)) => {
                    panic!(
                        "Expected error but got success for input '{input}'. \nGot: {target:?}\nExpected error: {expected_error}"
                    );
                }
                (Err(error), Ok(expected_target)) => {
                    panic!(
                        "Expected success but got error for input '{input}'. \nGot error: {error}\nExpected: {expected_target:?}"
                    );
                }
            }
        }
    }
}
