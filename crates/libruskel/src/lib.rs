mod error;

pub use crate::error::{Result, RuskelError};
use rustdoc_types::Crate;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub fn generate_json(target: Option<&str>) -> Result<Crate> {
    let manifest_path = find_manifest(target)?;
    let json_path = rustdoc_json::Builder::default()
        .toolchain("nightly")
        .manifest_path(&manifest_path)
        .build()
        .map_err(|e| RuskelError::RustdocJsonError(e.to_string()))?;
    let json_content = fs::read_to_string(&json_path)?;
    let crate_data: Crate = serde_json::from_str(&json_content)?;
    Ok(crate_data)
}

pub fn find_manifest(target: Option<&str>) -> Result<PathBuf> {
    if let Some(target_path) = target {
        let path = Path::new(target_path);
        if path.is_dir() {
            let manifest_path = path.join("Cargo.toml");
            if manifest_path.exists() {
                return Ok(manifest_path);
            }
        } else {
            return Err(RuskelError::InvalidTargetPath(path.to_path_buf()));
        }
    }

    let mut current_dir = env::current_dir()?;
    loop {
        let manifest_path = current_dir.join("Cargo.toml");
        if manifest_path.exists() {
            return Ok(manifest_path);
        }
        if !current_dir.pop() {
            break;
        }
    }

    Err(RuskelError::ManifestNotFound)
}

pub fn pretty_print_json(crate_data: &Crate) -> Result<String> {
    serde_json::to_string_pretty(&crate_data).map_err(RuskelError::JsonParseError)
}
