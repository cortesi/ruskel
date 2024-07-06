use rustdoc_types::Crate;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub fn generate_json(target: Option<&str>) -> Result<Crate, Box<dyn std::error::Error>> {
    let manifest_path = find_manifest(target)?;
    let json_path = rustdoc_json::Builder::default()
        .toolchain("nightly")
        .manifest_path(&manifest_path)
        .build()?;
    let json_content = fs::read_to_string(&json_path)?;
    let crate_data: Crate = serde_json::from_str(&json_content)?;
    Ok(crate_data)
}

pub fn find_manifest(target: Option<&str>) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(target_path) = target {
        let path = Path::new(target_path);
        if path.is_dir() {
            let manifest_path = path.join("Cargo.toml");
            if manifest_path.exists() {
                return Ok(manifest_path);
            }
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

    Err("Could not find Cargo.toml in the current directory or any parent directories".into())
}
