//! Shared nightly and `rustup` helpers.

use std::{
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use sha2::{Digest, Sha256};

use crate::error::{Result, RuskelError};

/// User-facing installation hint reused across nightly toolchain checks.
const NIGHTLY_INSTALL_HINT: &str =
    "ruskel requires the nightly toolchain to be installed. Run: rustup toolchain install nightly";
/// Component name for the rustdoc JSON support required for stdlib rendering.
const RUST_DOCS_JSON_COMPONENT: &str = "rust-docs-json";

/// Prefix that identifies a portable dated nightly toolchain.
const DATED_NIGHTLY_PREFIX: &str = "nightly-";

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

/// Ensure the nightly toolchain exists and report whether the `rust-docs-json`
/// component is installed.
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

/// Return the SHA-256 identity of the current nightly compiler description.
pub(crate) fn nightly_identity() -> Result<String> {
    toolchain_identity("nightly")
}

/// Return the SHA-256 identity of one compiler description.
pub fn toolchain_identity(toolchain: &str) -> Result<String> {
    let output = run_command(
        "rustup",
        &["run", toolchain, "rustc", "-vV"],
        true,
        &format!("Failed to identify toolchain '{toolchain}'"),
    )?;
    ensure_success(
        &output,
        &format!(
            "toolchain '{toolchain}' is not available. Run: rustup toolchain install {toolchain}"
        ),
    )?;
    Ok(identity_from_stdout(&output.stdout))
}

/// Return the active portable dated-nightly toolchain name.
pub fn active_dated_nightly() -> Result<String> {
    let output = run_command(
        "rustup",
        &["show", "active-toolchain"],
        true,
        "Failed to inspect the active Rust toolchain",
    )?;
    ensure_success(
        &output,
        "Failed to inspect the active Rust toolchain. Run: rustup show active-toolchain",
    )?;
    parse_active_dated_nightly(&output.stdout)
}

/// Return the host target reported by the selected compiler.
pub fn host_target(toolchain: &str) -> Result<String> {
    let output = run_command(
        "rustup",
        &["run", toolchain, "rustc", "-vV"],
        true,
        &format!("Failed to inspect toolchain '{toolchain}'"),
    )?;
    ensure_success(
        &output,
        &format!(
            "toolchain '{toolchain}' is not available. Run: rustup toolchain install {toolchain}"
        ),
    )?;
    parse_host_target(&output.stdout)
}

/// Locate a binary installed for the selected toolchain.
pub fn toolchain_binary(toolchain: &str, binary: &str) -> Result<PathBuf> {
    let output = run_command(
        "rustup",
        &["which", "--toolchain", toolchain, binary],
        true,
        &format!("Failed to locate '{binary}' for toolchain '{toolchain}'"),
    )?;
    ensure_success(
        &output,
        &format!(
            "component for '{binary}' is missing from toolchain '{toolchain}'. Run: rustup component add --toolchain {toolchain} {binary}"
        ),
    )?;
    parse_binary_path(&output.stdout, binary)
}

/// Hash exact command output bytes into a stable full identity.
fn identity_from_stdout(stdout: &[u8]) -> String {
    hex::encode(Sha256::digest(stdout))
}

/// Parse and normalize the active dated-nightly name from rustup output.
fn parse_active_dated_nightly(stdout: &[u8]) -> Result<String> {
    let value = String::from_utf8(stdout.to_vec()).map_err(|error| {
        RuskelError::Generate(format!("Invalid UTF-8 in active toolchain output: {error}"))
    })?;
    let name = value.split_whitespace().next().ok_or_else(|| {
        RuskelError::Generate("rustup did not report an active toolchain".to_string())
    })?;
    normalize_dated_nightly(name).ok_or_else(|| {
        RuskelError::Generate(format!(
            "active toolchain '{name}' is not a dated nightly. Use --toolchain nightly-YYYY-MM-DD"
        ))
    })
}

/// Accept a dated nightly with an optional rustup host suffix.
pub(crate) fn normalize_dated_nightly(name: &str) -> Option<String> {
    let date = name.strip_prefix(DATED_NIGHTLY_PREFIX)?.get(..10)?;
    let bytes = date.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
    {
        return None;
    }
    let month = date[5..7].parse::<u8>().ok()?;
    let day = date[8..10].parse::<u8>().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let portable_len = DATED_NIGHTLY_PREFIX.len() + 10;
    let suffix = &name[portable_len..];
    if !suffix.is_empty() {
        let host = suffix.strip_prefix('-')?;
        let components: Vec<_> = host.split('-').collect();
        if components.len() < 3
            || components.iter().any(|component| {
                component.is_empty()
                    || !component
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.'))
            })
        {
            return None;
        }
    }
    Some(name[..portable_len].to_string())
}

/// Parse the `host:` field from exact compiler version output.
fn parse_host_target(stdout: &[u8]) -> Result<String> {
    let value = String::from_utf8(stdout.to_vec()).map_err(|error| {
        RuskelError::Generate(format!("Invalid UTF-8 in compiler identity: {error}"))
    })?;
    value
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .filter(|host| !host.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            RuskelError::Generate("compiler identity does not contain a host target".to_string())
        })
}

/// Parse a single absolute binary path from `rustup which` output.
fn parse_binary_path(stdout: &[u8], binary: &str) -> Result<PathBuf> {
    let value = String::from_utf8(stdout.to_vec()).map_err(|error| {
        RuskelError::Generate(format!("Invalid UTF-8 in '{binary}' path: {error}"))
    })?;
    let path = Path::new(value.trim());
    if path.as_os_str().is_empty() || !path.is_absolute() {
        return Err(RuskelError::Generate(format!(
            "rustup returned an invalid path for '{binary}'"
        )));
    }
    Ok(path.to_path_buf())
}

/// Execute a subprocess and convert spawn failures into
/// `RuskelError::Generate`.
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

/// Check whether `rustup component list` reports the named component as
/// installed.
fn has_installed_component(stdout: &[u8], component: &str) -> bool {
    String::from_utf8_lossy(stdout)
        .lines()
        .any(|line| line.starts_with(component) && line.contains("(installed)"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        has_installed_component, identity_from_stdout, normalize_dated_nightly,
        parse_active_dated_nightly, parse_binary_path, parse_host_target, parse_sysroot_path,
        toolchain_binary, toolchain_identity,
    };
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

    #[test]
    fn nightly_identity_hashes_exact_stdout_bytes() {
        assert_eq!(identity_from_stdout(b"nightly\n").len(), 64);
        assert_ne!(
            identity_from_stdout(b"nightly\n"),
            identity_from_stdout(b"nightly")
        );
    }

    #[test]
    fn dated_nightly_parser_accepts_portable_and_host_qualified_names() -> Result<()> {
        assert_eq!(
            parse_active_dated_nightly(b"nightly-2026-08-31-aarch64-apple-darwin (default)\n")?,
            "nightly-2026-08-31"
        );
        assert_eq!(
            parse_active_dated_nightly(b"nightly-2026-08-31 (overridden by '/tmp')\n")?,
            "nightly-2026-08-31"
        );
        Ok(())
    }

    #[test]
    fn dated_nightly_parser_rejects_nonportable_names() {
        for name in [
            "nightly",
            "beta-aarch64-apple-darwin",
            "stable-x86_64-unknown-linux-gnu",
            "custom",
            "nightly-2026-8-31",
            "nightly-2026-99-99",
            "nightly-2026-08-31-custom",
        ] {
            assert!(
                parse_active_dated_nightly(format!("{name}\n").as_bytes()).is_err(),
                "{name} must be rejected"
            );
        }
        assert!(normalize_dated_nightly("nightly-2026-08-31x").is_none());
    }

    #[test]
    fn host_parser_reads_only_the_host_field() -> Result<()> {
        let output = b"rustc 1.92.0-nightly\ncommit-date: 2026-08-31\nhost: aarch64-apple-darwin\n";
        assert_eq!(parse_host_target(output)?, "aarch64-apple-darwin");
        assert!(parse_host_target(b"rustc 1.92.0-nightly\n").is_err());
        Ok(())
    }

    #[test]
    fn binary_path_parser_requires_an_absolute_path() -> Result<()> {
        assert_eq!(
            parse_binary_path(b"/tmp/toolchains/nightly/bin/rustfmt\n", "rustfmt")?,
            PathBuf::from("/tmp/toolchains/nightly/bin/rustfmt")
        );
        assert!(parse_binary_path(b"rustfmt\n", "rustfmt").is_err());
        Ok(())
    }

    #[test]
    fn missing_toolchain_error_has_an_install_command() {
        let error = toolchain_identity("nightly-ruskel-missing")
            .expect_err("unknown toolchain must fail")
            .to_string();
        assert!(
            error.contains("rustup toolchain install nightly-ruskel-missing"),
            "{error}"
        );
    }

    #[test]
    fn missing_component_error_has_an_install_command() {
        let error = toolchain_binary("nightly", "ruskel-missing-component")
            .expect_err("unknown component binary must fail")
            .to_string();
        assert!(
            error.contains("rustup component add --toolchain nightly ruskel-missing-component"),
            "{error}"
        );
    }
}
