//! Shared nightly and `rustup` helpers.

use std::{
    path::PathBuf,
    process::{Command, Output, Stdio},
};

use crate::error::{Result, RuskelError};

/// User-facing installation hint reused across nightly toolchain checks.
const NIGHTLY_INSTALL_HINT: &str =
    "ruskel requires the nightly toolchain to be installed. Run: rustup toolchain install nightly";
/// Component name for the rustdoc JSON support required for stdlib rendering.
const RUST_DOCS_JSON_COMPONENT: &str = "rust-docs-json";

/// Locate the nightly toolchain sysroot path.
pub fn nightly_sysroot() -> Result<PathBuf> {
    let output = run_command(
        "rustc",
        &["+nightly", "--print", "sysroot"],
        false,
        "Failed to get sysroot",
    )?;
    if !output.status.success() {
        return Err(RuskelError::Generate(
            "ruskel requires the nightly toolchain to be installed - run 'rustup toolchain install nightly'"
                .to_string(),
        ));
    }

    parse_sysroot_path(&output.stdout)
}

/// Ensure the nightly toolchain exists and report whether the `rust-docs-json` component is installed.
pub fn ensure_nightly_with_docs() -> Result<bool> {
    let output = run_command(
        "rustup",
        &["run", "nightly", "rustc", "--version"],
        true,
        "Failed to run rustup",
    )?;
    ensure_success(&output, NIGHTLY_INSTALL_HINT)?;

    let components = run_command(
        "rustup",
        &["component", "list", "--toolchain", "nightly"],
        true,
        "Failed to check nightly components",
    )?;
    if !components.status.success() {
        return Ok(false);
    }

    Ok(has_installed_component(
        &components.stdout,
        RUST_DOCS_JSON_COMPONENT,
    ))
}

/// Execute a subprocess and convert spawn failures into `RuskelError::Generate`.
fn run_command(
    program: &str,
    args: &[&str],
    quiet_stderr: bool,
    failure_context: &str,
) -> Result<Output> {
    let mut command = Command::new(program);
    command.args(args);
    if quiet_stderr {
        command.stderr(Stdio::null());
    }

    command
        .output()
        .map_err(|error| RuskelError::Generate(format!("{failure_context}: {error}")))
}

/// Convert a non-zero subprocess exit into a generated user-facing error.
fn ensure_success(output: &Output, failure_message: &str) -> Result<()> {
    if output.status.success() {
        Ok(())
    } else {
        Err(RuskelError::Generate(failure_message.to_string()))
    }
}

/// Parse a `rustc --print sysroot` response into a trimmed filesystem path.
fn parse_sysroot_path(stdout: &[u8]) -> Result<PathBuf> {
    let sysroot = String::from_utf8(stdout.to_vec()).map_err(|error| {
        RuskelError::Generate(format!("Invalid UTF-8 in sysroot path: {error}"))
    })?;
    Ok(PathBuf::from(sysroot.trim()))
}

/// Check whether `rustup component list` reports the named component as installed.
fn has_installed_component(stdout: &[u8], component: &str) -> bool {
    String::from_utf8_lossy(stdout)
        .lines()
        .any(|line| line.starts_with(component) && line.contains("(installed)"))
}

#[cfg(test)]
mod tests {
    use super::{has_installed_component, parse_sysroot_path};
    use crate::error::Result;

    #[test]
    fn parse_sysroot_path_trims_trailing_newlines() -> Result<()> {
        let path = parse_sysroot_path(b"/tmp/nightly-sysroot\n")?;
        assert_eq!(path.to_string_lossy(), "/tmp/nightly-sysroot");
        Ok(())
    }

    #[test]
    fn parse_sysroot_path_rejects_invalid_utf8() {
        let error = parse_sysroot_path(&[0xff]).expect_err("invalid utf8 should fail");
        assert_eq!(
            error.to_string(),
            "Invalid UTF-8 in sysroot path: invalid utf-8 sequence of 1 bytes from index 0"
        );
    }

    #[test]
    fn component_parser_detects_installed_component() {
        let stdout = b"rust-docs-json-x86_64-apple-darwin (installed)\nrust-src (installed)\n";
        assert!(has_installed_component(stdout, "rust-docs-json"));
        assert!(!has_installed_component(stdout, "clippy"));
    }

    #[test]
    fn component_parser_ignores_available_but_uninstalled_component() {
        let stdout = b"rust-docs-json-x86_64-apple-darwin\n";
        assert!(!has_installed_component(stdout, "rust-docs-json"));
    }
}
