use thiserror::Error;

#[derive(Error, Debug)]
pub enum RuskelError {
    /// Indicates that a specified module could not be found.
    #[error("Module not found: {0}")]
    ModuleNotFound(String),

    /// Indicates a failure in reading a file, wrapping the underlying IO error.
    #[error("Failed to read file: {0}")]
    FileRead(#[from] std::io::Error),

    /// Indicates a failure in the code generation process.
    #[error("Failed to generate: {0}")]
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

pub type Result<T> = std::result::Result<T, RuskelError>;
