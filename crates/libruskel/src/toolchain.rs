use std::{
    path::PathBuf,
    process::{Command, Stdio},
};

use crate::error::{Result, RuskelError};

/// Locate the nightly toolchain sysroot path.
pub fn nightly_sysroot() -> Result<PathBuf> {
    let output = Command::new("rustc")
        .args(["+nightly", "--print", "sysroot"])
        .output()
        .map_err(|e| RuskelError::Generate(format!("Failed to get sysroot: {e}")))?;

    if !output.status.success() {
        return Err(RuskelError::Generate(
            "ruskel requires the nightly toolchain to be installed - run 'rustup toolchain install nightly'".to_string(),
        ));
    }

    let sysroot = String::from_utf8(output.stdout)
        .map_err(|e| RuskelError::Generate(format!("Invalid UTF-8 in sysroot path: {e}")))?
        .trim()
        .to_string();

    Ok(PathBuf::from(sysroot))
}

/// Ensure the nightly toolchain exists and report whether the `rust-docs-json` component is installed.
pub fn ensure_nightly_with_docs() -> Result<bool> {
    let output = Command::new("rustup")
        .args(["run", "nightly", "rustc", "--version"])
        .stderr(Stdio::null())
        .output()
        .map_err(|e| RuskelError::Generate(format!("Failed to run rustup: {e}")))?;

    if !output.status.success() {
        return Err(RuskelError::Generate(
            "ruskel requires the nightly toolchain to be installed. \
            Run: rustup toolchain install nightly"
                .to_string(),
        ));
    }

    let components_output = Command::new("rustup")
        .args(["component", "list", "--toolchain", "nightly"])
        .stderr(Stdio::null())
        .output()
        .map_err(|e| RuskelError::Generate(format!("Failed to check nightly components: {e}")))?;

    if !components_output.status.success() {
        return Ok(false);
    }

    let has_rust_docs_json = String::from_utf8_lossy(&components_output.stdout)
        .lines()
        .any(|line| line.starts_with("rust-docs-json") && line.contains("(installed)"));

    Ok(has_rust_docs_json)
}
