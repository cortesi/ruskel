use std::{io, result};

use thiserror::Error;

/// Convenience alias for results returned by libruskel operations.
pub type Result<T> = result::Result<T, RuskelError>;

/// Errors surfaced while generating rustdoc skeletons.
#[derive(Error, Debug)]
pub enum RuskelError {
    /// Indicates that a specified module could not be found.
    #[error("Module not found: {0}")]
    ModuleNotFound(String),

    /// Indicates a failure in reading a file, wrapping the underlying IO error.
    #[error("Failed to read file: {0}")]
    FileRead(#[from] io::Error),

    /// Indicates a failure in the code generation process.
    #[error("{0}")]
    Generate(String),

    /// Indicates an item referenced by rustdoc output could not be found.
    #[error("Item not found in rustdoc index: {0}")]
    ItemNotFound(String),

    /// Indicates an error occurred while executing a Cargo command.
    #[error("Cargo error: {0}")]
    Cargo(String),

    /// Indicates an error occurred during code formatting.
    #[error("Formatting error: {0}")]
    Format(String),

    /// Indicates an error occurred during syntax highlighting.
    #[error("Highlighting error: {0}")]
    Highlight(String),

    /// The specified filter did not match any items.
    #[error("Filter '{0}' did not match any items")]
    FilterNotMatched(String),

    /// Failed to parse a Cargo.toml manifest
    #[error("Failed to parse Cargo.toml manifest: {0}")]
    ManifestParse(String),

    /// Indicates that the Cargo.toml manifest file could not be found in the current directory or any parent directories.
    #[error("Failed to find Cargo.toml in the current directory or any parent directories")]
    ManifestNotFound,

    /// Indicates an invalid version string was provided.
    #[error("Invalid version: {0}")]
    InvalidVersion(String),

    /// Indicates an invalid target specification was provided.
    #[error("Invalid target: {0}")]
    InvalidTarget(String),

    /// Indicates a dependency was not found in the registry.
    #[error("No matching package")]
    DependencyNotFound,

    /// A catch-all for other Cargo-related errors.
    #[error("Cargo error: {0}")]
    CargoError(String),
}

impl From<syntect::Error> for RuskelError {
    fn from(err: syntect::Error) -> Self {
        Self::Highlight(err.to_string())
    }
}

impl From<serde_json::Error> for RuskelError {
    fn from(err: serde_json::Error) -> Self {
        Self::Generate(err.to_string())
    }
}

impl From<rust_format::Error> for RuskelError {
    fn from(err: rust_format::Error) -> Self {
        Self::Format(err.to_string())
    }
}

/// Convert an `anyhow::Error` into the corresponding `RuskelError` variant.
pub fn convert_cargo_error(error: &anyhow::Error) -> RuskelError {
    let err_msg = error.to_string();
    if err_msg.contains("no matching package") {
        RuskelError::DependencyNotFound
    } else {
        RuskelError::CargoError(err_msg)
    }
}
