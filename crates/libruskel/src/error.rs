use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RuskelError {
    #[error("Failed to find Cargo.toml in the current directory or any parent directories")]
    ManifestNotFound,

    #[error("Module not found: {0}")]
    ModuleNotFound(String),

    #[error("Failed to read file: {0}")]
    FileReadError(#[from] std::io::Error),

    #[error("Failed to parse JSON: {0}")]
    JsonParseError(#[from] serde_json::Error),

    #[error("Failed to generate rustdoc JSON: {0}")]
    RustdocJsonError(String),

    #[error("Invalid target path: {0}")]
    InvalidTargetPath(PathBuf),

    #[error("Cargo error: {0}")]
    CargoError(#[from] anyhow::Error),

    #[error("Formatting error: {0}")]
    FormatError(#[from] rust_format::Error),
}

pub type Result<T> = std::result::Result<T, RuskelError>;
