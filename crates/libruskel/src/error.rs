use thiserror::Error;

pub type Result<T> = std::result::Result<T, RuskelError>;

#[derive(Error, Debug)]
pub enum RuskelError {
    /// Indicates that a specified module could not be found.
    #[error("Module not found: {0}")]
    ModuleNotFound(String),

    /// Indicates a failure in reading a file, wrapping the underlying IO error.
    #[error("Failed to read file: {0}")]
    FileRead(#[from] std::io::Error),

    /// Indicates a failure in the code generation process.
    #[error("{0}")]
    Generate(String),

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
        RuskelError::Highlight(err.to_string())
    }
}

impl From<serde_json::Error> for RuskelError {
    fn from(err: serde_json::Error) -> Self {
        RuskelError::Generate(err.to_string())
    }
}

impl From<rust_format::Error> for RuskelError {
    fn from(err: rust_format::Error) -> Self {
        RuskelError::Format(err.to_string())
    }
}

/// Converts anyhow::Error to our custom RuskelError type.
pub fn convert_cargo_error(error: anyhow::Error) -> RuskelError {
    let err_msg = error.to_string();
    if err_msg.contains("no matching package") {
        RuskelError::DependencyNotFound
    } else {
        RuskelError::CargoError(err_msg)
    }
}

/// Generate a consistent error message for missing nightly toolchain
pub fn nightly_install_error(context: &str, target_arch: Option<&str>) -> String {
    let install_cmd = if let Some(target) = target_arch {
        format!("rustup toolchain install nightly --target {target}")
    } else {
        "rustup toolchain install nightly".to_string()
    };
    format!("{context} - run '{install_cmd}'")
}
