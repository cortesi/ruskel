use thiserror::Error;

#[derive(Error, Debug)]
pub enum RuskelError {
    #[error("Failed to find Cargo.toml in the current directory or any parent directories")]
    ManifestNotFound,

    #[error("Module not found: {0}")]
    ModuleNotFound(String),

    #[error("Failed to read file: {0}")]
    FileRead(#[from] std::io::Error),

    #[error("Failed to generate: {0}")]
    Generate(String),

    #[error("Cargo error: {0}")]
    Cargo(String),

    #[error("Formatting error: {0}")]
    Format(String),

    #[error("Highlighting error: {0}")]
    Highlight(String),
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
