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
        .document_private_items(true)
        .manifest_path(&manifest_path)
        .build()
        .map_err(|e| RuskelError::RustdocJsonError(e.to_string()))?;
    let json_content = fs::read_to_string(&json_path)?;
    let crate_data: Crate = serde_json::from_str(&json_content)?;
    Ok(crate_data)
}

pub fn find_manifest(target: Option<&str>) -> Result<PathBuf> {
    match target {
        Some(target_path) => {
            let path = Path::new(target_path);
            if path.is_dir() {
                find_manifest_in_dir(path)
            } else if path.is_file() && path.extension().map_or(false, |ext| ext == "rs") {
                find_manifest_from_file(path)
            } else {
                Err(RuskelError::InvalidTargetPath(path.to_path_buf()))
            }
        }
        None => find_manifest_in_dir(&env::current_dir()?),
    }
}

fn find_manifest_in_dir(dir: &Path) -> Result<PathBuf> {
    let manifest_path = dir.join("Cargo.toml");
    if manifest_path.exists() {
        Ok(manifest_path)
    } else {
        find_manifest_in_parent_dirs(dir)
    }
}

fn find_manifest_from_file(file: &Path) -> Result<PathBuf> {
    if let Some(parent) = file.parent() {
        find_manifest_in_parent_dirs(parent)
    } else {
        Err(RuskelError::ManifestNotFound)
    }
}

fn find_manifest_in_parent_dirs(start_dir: &Path) -> Result<PathBuf> {
    let mut current_dir = start_dir.to_path_buf();
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use tempfile::TempDir;

    #[test]
    fn test_find_manifest() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let cargo_toml_path = temp_dir.path().join("Cargo.toml");
        File::create(&cargo_toml_path)?;

        let src_dir = temp_dir.path().join("src");
        fs::create_dir(&src_dir)?;
        File::create(src_dir.join("lib.rs"))?;

        let sub_dir = src_dir.join("sub");
        fs::create_dir(&sub_dir)?;

        // Test finding Cargo.toml in current directory
        assert_eq!(
            find_manifest(Some(temp_dir.path().to_str().unwrap()))?,
            cargo_toml_path
        );

        // Test finding Cargo.toml in parent directory
        assert_eq!(
            find_manifest(Some(src_dir.to_str().unwrap()))?,
            cargo_toml_path
        );

        // Test finding Cargo.toml from Rust file
        assert_eq!(
            find_manifest(Some(src_dir.join("lib.rs").to_str().unwrap()))?,
            cargo_toml_path
        );

        // Test finding Cargo.toml from subdirectory
        assert_eq!(
            find_manifest(Some(sub_dir.to_str().unwrap()))?,
            cargo_toml_path
        );

        // Test Cargo.toml not found
        let another_temp_dir = TempDir::new()?;
        assert!(matches!(
            find_manifest(Some(another_temp_dir.path().to_str().unwrap())),
            Err(RuskelError::ManifestNotFound)
        ));

        // Test invalid path
        assert!(matches!(
            find_manifest(Some("/non/existent/path")),
            Err(RuskelError::InvalidTargetPath(_))
        ));

        Ok(())
    }
}
