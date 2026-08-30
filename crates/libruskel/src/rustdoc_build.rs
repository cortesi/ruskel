use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    result,
};

use cargo::core::Workspace;
use rustdoc_json::PackageTarget;
use rustdoc_types::Crate;

use super::{
    stdlib,
    target_resolution::{ResolvedSource, ResolvedTarget, create_quiet_cargo_config},
};
use crate::{
    cache::{BuildLease, CacheHandle},
    error::{Result, RuskelError, convert_cargo_error},
    toolchain::nightly_identity,
};

/// Build rustdoc JSON for one resolved target.
pub fn build(resolved: &ResolvedTarget, options: &CrateReadOptions) -> Result<CrateRead> {
    let manifest_path = match &resolved.source {
        ResolvedSource::Package { manifest_path } => manifest_path,
        ResolvedSource::StdLibrary { actual, display } => {
            let display_name = (actual != display).then_some(display.as_str());
            return Ok(CrateRead {
                crate_data: stdlib::load_json(actual, display_name)?,
                bin_target: None,
            });
        }
    };
    let PackageTargetSelection {
        package_target,
        bin_target,
        workspace_root,
        package_name,
        package_version,
    } = select_package_target(
        manifest_path,
        options.offline,
        options.bin_override.as_deref(),
    )?;
    let include_private =
        options.private_items || bin_target.as_ref().is_some_and(|target| target.is_bin_only);
    let owner = options.cache.owner()?;
    let mut storage_retry: Option<(String, String)> = None;
    let mut retry_budget = RetryBudget::default();

    while retry_budget.begin_attempt() {
        let toolchain_before =
            nightly_identity().map_err(|error| with_recovery_context(&mut storage_retry, error))?;
        let lease = match owner.begin_build(
            &toolchain_before,
            &workspace_root,
            &package_name,
            &package_version,
        ) {
            Ok(lease) => lease,
            Err(error) if owner.is_entry_error(&error) && retry_budget.take_storage_retry() => {
                let original = error.to_string();
                let quarantine =
                    match owner.quarantine_workspace(&toolchain_before, &workspace_root) {
                        Ok(Some(path)) => {
                            format!("moved the damaged cache entry to '{}'", path.display())
                        }
                        Ok(None) => "the damaged cache entry did not exist".to_string(),
                        Err(quarantine_error) => {
                            format!("could not move the damaged cache entry: {quarantine_error}")
                        }
                    };
                let maintenance = match owner.recover_storage(&toolchain_before) {
                    Ok(action) => action,
                    Err(maintenance_error) => {
                        format!("synchronous maintenance failed: {maintenance_error}")
                    }
                };
                owner.signal_maintenance(&toolchain_before, false);
                storage_retry = Some((original, format!("{quarantine}; {maintenance}")));
                continue;
            }
            Err(error) => return Err(with_recovery_context(&mut storage_retry, error)),
        };
        let attempt_result = build_once(
            manifest_path,
            package_target.clone(),
            bin_target.clone(),
            include_private,
            options,
            &lease,
        );
        let attempt_result = match attempt_result {
            Ok(crate_read) => lease
                .touch_success()
                .map(|()| crate_read)
                .map_err(BuildAttemptFailure::Storage),
            error => error,
        };
        let low_space = owner.is_low_space();
        let attempt_result = match attempt_result {
            Err(BuildAttemptFailure::Diagnostic(error)) if low_space => {
                Err(BuildAttemptFailure::Storage(error))
            }
            other => other,
        };

        match attempt_result {
            Ok(crate_read) => {
                owner.signal_maintenance(&toolchain_before, low_space);
                let toolchain_after = nightly_identity()
                    .map_err(|error| with_recovery_context(&mut storage_retry, error))?;
                if toolchain_after != toolchain_before {
                    drop(lease);
                    if !retry_budget.take_toolchain_retry() {
                        return Err(RuskelError::Generate(
                            "The nightly toolchain changed repeatedly while Ruskel generated rustdoc JSON. Retry the request after the update completes."
                                .to_string(),
                        ));
                    }
                    continue;
                }
                return Ok(crate_read);
            }
            Err(BuildAttemptFailure::Storage(error)) if retry_budget.take_storage_retry() => {
                let original = error.to_string();
                let recovery = match lease.move_to_trash() {
                    Ok(path) => {
                        format!("moved the damaged cache entry to '{}'", path.display())
                    }
                    Err(recovery_error) => {
                        format!("could not move the damaged cache entry: {recovery_error}")
                    }
                };
                drop(lease);
                let maintenance = match owner.recover_storage(&toolchain_before) {
                    Ok(action) => action,
                    Err(error) => format!("synchronous maintenance failed: {error}"),
                };
                owner.signal_maintenance(&toolchain_before, false);
                storage_retry = Some((original, format!("{recovery}; {maintenance}")));
            }
            Err(failure) => {
                let retry_error = failure.into_error();
                owner.signal_maintenance(&toolchain_before, low_space);
                return Err(with_recovery_context(&mut storage_retry, retry_error));
            }
        }
    }

    Err(RuskelError::Generate(
        "Ruskel exhausted the rustdoc build retry budget".to_string(),
    ))
}

/// Determine which Cargo target should be used for rustdoc JSON generation.
fn select_package_target(
    manifest_path: &Path,
    offline: bool,
    bin_override: Option<&str>,
) -> Result<PackageTargetSelection> {
    let config = create_quiet_cargo_config(offline)?;
    let workspace =
        Workspace::new(manifest_path, &config).map_err(|err| convert_cargo_error(&err))?;
    let workspace_root =
        fs::canonicalize(workspace.root()).map_err(|source| RuskelError::CacheIo {
            action: "canonicalize Cargo workspace root",
            path: workspace.root().to_path_buf(),
            source,
        })?;
    let package = workspace
        .current()
        .map_err(|err| convert_cargo_error(&err))?;
    let package_name = package.name().to_string();
    let package_version = package.version().to_string();

    let has_lib = package.targets().iter().any(|target| target.is_lib());
    let bin_targets: Vec<_> = package
        .targets()
        .iter()
        .filter(|target| target.is_bin())
        .collect();
    let bin_names: Vec<&str> = bin_targets.iter().map(|target| target.name()).collect();

    if let Some(bin_name) = bin_override {
        if bin_names.contains(&bin_name) {
            return Ok(PackageTargetSelection {
                package_target: PackageTarget::Bin(bin_name.to_string()),
                bin_target: Some(BinaryTarget {
                    name: bin_name.to_string(),
                    is_bin_only: !has_lib,
                }),
                workspace_root,
                package_name,
                package_version,
            });
        }

        let available = if bin_names.is_empty() {
            "no binary targets found".to_string()
        } else {
            format!("available: {}", bin_names.join(", "))
        };

        return Err(RuskelError::Generate(format!(
            "error: binary target '{bin_name}' not found in package ({available})"
        )));
    }

    if has_lib {
        return Ok(PackageTargetSelection {
            package_target: PackageTarget::Lib,
            bin_target: None,
            workspace_root,
            package_name,
            package_version,
        });
    }

    if bin_names.is_empty() {
        return Err(RuskelError::Generate(
            "error: no library targets found in package".to_string(),
        ));
    }

    if let Some(default_run) = package.manifest().default_run()
        && bin_names.contains(&default_run)
    {
        return Ok(PackageTargetSelection {
            package_target: PackageTarget::Bin(default_run.to_string()),
            bin_target: Some(BinaryTarget {
                name: default_run.to_string(),
                is_bin_only: true,
            }),
            workspace_root,
            package_name,
            package_version,
        });
    }

    if bin_names.len() == 1 {
        let name = bin_names[0];
        return Ok(PackageTargetSelection {
            package_target: PackageTarget::Bin(name.to_string()),
            bin_target: Some(BinaryTarget {
                name: name.to_string(),
                is_bin_only: true,
            }),
            workspace_root,
            package_name,
            package_version,
        });
    }

    Err(RuskelError::Generate(format!(
        "error: multiple binary targets found in package ({})",
        bin_names.join(", ")
    )))
}

/// Metadata describing a selected binary target.
#[derive(Debug, Clone)]
pub struct BinaryTarget {
    /// Name of the selected binary target.
    pub(crate) name: String,
    /// Whether the package has no library target.
    pub(crate) is_bin_only: bool,
}

/// Container for rustdoc JSON and target metadata.
#[derive(Debug)]
pub struct CrateRead {
    /// Parsed rustdoc JSON for the selected target.
    pub(crate) crate_data: Crate,
    /// Binary target metadata when a bin target was selected.
    pub(crate) bin_target: Option<BinaryTarget>,
}

/// Options controlling how rustdoc JSON is generated.
#[derive(Debug, Clone)]
pub struct CrateReadOptions {
    /// Whether to disable default features.
    pub(crate) no_default_features: bool,
    /// Whether to enable all features.
    pub(crate) all_features: bool,
    /// Specific feature list to enable.
    pub(crate) features: Vec<String>,
    /// Whether to include private items in rustdoc output.
    pub(crate) private_items: bool,
    /// Whether to suppress cargo output during rustdoc generation.
    pub(crate) silent: bool,
    /// Whether to force offline mode for cargo operations.
    pub(crate) offline: bool,
    /// Optional override of the binary target name.
    pub(crate) bin_override: Option<String>,
    /// Dedicated cache handle for non-standard-library builds.
    pub(crate) cache: CacheHandle,
}

/// Internal package target selection details for rustdoc JSON.
#[derive(Debug)]
struct PackageTargetSelection {
    /// Cargo package target used for rustdoc JSON.
    package_target: PackageTarget,
    /// Binary target metadata for frontmatter output.
    bin_target: Option<BinaryTarget>,
    /// Canonical root shared by all members of the selected workspace.
    workspace_root: PathBuf,
    /// Selected package name for cache display metadata.
    package_name: String,
    /// Selected package version for cache display metadata.
    package_version: String,
}

/// Failure category retained until the build path makes its retry decision.
#[derive(Debug)]
enum BuildAttemptFailure {
    /// Probable cache storage damage that permits one cold retry.
    Storage(RuskelError),
    /// Compiler or rustdoc diagnostics that low space can reclassify.
    Diagnostic(RuskelError),
    /// A compatibility, manifest, metadata, or tool invocation failure.
    Final(RuskelError),
}

/// Independent retry reasons within the shared three-attempt limit.
#[derive(Debug, Default)]
struct RetryBudget {
    /// Attempts already started.
    attempts: u8,
    /// Whether the storage retry was consumed.
    storage_used: bool,
    /// Whether the toolchain-change retry was consumed.
    toolchain_used: bool,
}

impl RetryBudget {
    /// Start an attempt when the total limit has not been reached.
    fn begin_attempt(&mut self) -> bool {
        if self.attempts >= 3 {
            return false;
        }
        self.attempts += 1;
        true
    }

    /// Consume the one storage retry when another attempt remains.
    fn take_storage_retry(&mut self) -> bool {
        if self.storage_used || self.attempts >= 3 {
            return false;
        }
        self.storage_used = true;
        true
    }

    /// Consume the one toolchain-change retry when another attempt remains.
    fn take_toolchain_retry(&mut self) -> bool {
        if self.toolchain_used || self.attempts >= 3 {
            return false;
        }
        self.toolchain_used = true;
        true
    }
}

/// Attach the original storage failure and recovery action to a retry failure.
fn with_recovery_context(
    context: &mut Option<(String, String)>,
    retry_error: RuskelError,
) -> RuskelError {
    let Some((original, recovery)) = context.take() else {
        return retry_error;
    };
    RuskelError::Generate(format!(
        "Rustdoc cache recovery failed.\nOriginal failure: {original}\nRecovery: {recovery}\nRetry failure: {retry_error}"
    ))
}

impl BuildAttemptFailure {
    /// Discard the internal retry category after the retry decision.
    fn into_error(self) -> RuskelError {
        match self {
            Self::Storage(error) | Self::Diagnostic(error) | Self::Final(error) => error,
        }
    }
}

/// Run one rustdoc build and read its JSON from a leased cache entry.
fn build_once(
    manifest_path: &Path,
    package_target: PackageTarget,
    bin_target: Option<BinaryTarget>,
    include_private: bool,
    options: &CrateReadOptions,
    lease: &BuildLease,
) -> result::Result<CrateRead, BuildAttemptFailure> {
    let mut captured_stdout = Vec::new();
    let mut captured_stderr = Vec::new();
    let build_dir = lease.build_dir();

    let build_result = rustdoc_json::Builder::default()
        .toolchain("nightly")
        .manifest_path(manifest_path)
        .package_target(package_target)
        .document_private_items(include_private)
        .no_default_features(options.no_default_features)
        .all_features(options.all_features)
        .features(&options.features)
        .target_dir(build_dir)
        .env("CARGO_BUILD_BUILD_DIR", build_dir)
        .quiet(options.silent)
        .silent(false)
        .build_with_captured_output(&mut captured_stdout, &mut captured_stderr);

    if !options.silent {
        if !captured_stdout.is_empty() && io::stdout().write_all(&captured_stdout).is_err() {
            // Output mirroring is best effort.
        }
        if !captured_stderr.is_empty() && io::stderr().write_all(&captured_stderr).is_err() {
            // Output mirroring is best effort.
        }
    }

    let json_path = match build_result {
        Ok(path) => path,
        Err(rustdoc_json::BuildError::IoError(source)) => {
            return Err(BuildAttemptFailure::Storage(RuskelError::CacheIo {
                action: "generate rustdoc JSON",
                path: build_dir.to_path_buf(),
                source,
            }));
        }
        Err(error) => {
            let mapped = map_rustdoc_build_error(&error, &captured_stderr, options.silent);
            if matches!(error, rustdoc_json::BuildError::BuildRustdocJsonError) {
                return Err(BuildAttemptFailure::Diagnostic(mapped));
            }
            return Err(BuildAttemptFailure::Final(mapped));
        }
    };

    let json_content = fs::read_to_string(&json_path).map_err(|source| {
        BuildAttemptFailure::Storage(RuskelError::CacheIo {
            action: "read generated rustdoc JSON",
            path: json_path.clone(),
            source,
        })
    })?;
    let crate_data: Crate = serde_json::from_str(&json_content).map_err(|error| {
        use serde_json::error::Category;

        match error.classify() {
            Category::Syntax | Category::Eof | Category::Io => {
                BuildAttemptFailure::Storage(RuskelError::CacheLayout {
                    path: json_path.clone(),
                    message: format!("generated rustdoc JSON is incomplete or invalid: {error}"),
                })
            }
            Category::Data => BuildAttemptFailure::Final(RuskelError::Generate(format!(
                "Failed to parse rustdoc JSON, which may indicate an outdated nightly toolchain. Run: rustup update nightly\nError: {error}"
            ))),
        }
    })?;

    Ok(CrateRead {
        crate_data,
        bin_target,
    })
}

/// Maximum number of characters from rustdoc stderr included in failure
/// reports.
const MAX_STDERR_CHARS: usize = 8_192;

/// Translate a `rustdoc_json` build failure into a user-facing [`RuskelError`].
fn map_rustdoc_build_error(
    err: &rustdoc_json::BuildError,
    captured_stderr: &[u8],
    silent: bool,
) -> RuskelError {
    match err {
        rustdoc_json::BuildError::BuildRustdocJsonError => {
            format_rustdoc_failure(captured_stderr, silent)
        }
        other => {
            let err_msg = other.to_string();

            if err_msg.contains("no library targets found in package") {
                return RuskelError::Generate(
                    "error: no library targets found in package".to_string(),
                );
            }

            if err_msg.contains("toolchain") && err_msg.contains("is not installed") {
                return RuskelError::Generate(
                    "ruskel requires the nightly toolchain to be installed - run 'rustup toolchain install nightly'"
                        .to_string(),
                );
            }

            if err_msg.contains("Failed to build rustdoc JSON") {
                return format_rustdoc_failure(captured_stderr, silent);
            }

            RuskelError::Generate(format!("Failed to build rustdoc JSON: {err_msg}"))
        }
    }
}

/// Format a detailed error for rustdoc build failures, optionally embedding
/// diagnostics.
fn format_rustdoc_failure(captured_stderr: &[u8], silent: bool) -> RuskelError {
    let stderr_raw = String::from_utf8_lossy(captured_stderr).into_owned();
    let stderr_trimmed = stderr_raw.trim();
    let summary = extract_primary_diagnostic(stderr_trimmed).unwrap_or_else(|| {
        "rustdoc exited with an error; rerun with --verbose for full diagnostics.".to_string()
    });
    let summary = summary.trim();

    if silent {
        if stderr_trimmed.is_empty() {
            return RuskelError::Generate(
                "Failed to build rustdoc JSON: rustdoc exited with an error but emitted no diagnostics. \
                 Re-run with --verbose or `cargo rustdoc` to inspect the failure.".to_string(),
            );
        }

        let (diagnostics, truncated) = truncate_diagnostics(stderr_trimmed);
        let mut message = format!("Failed to build rustdoc JSON: {summary}");
        message.push_str("\n\nrustdoc stderr:\n");
        message.push_str(&diagnostics);
        if truncated {
            message.push_str("\n… output truncated …");
        }
        return RuskelError::Generate(message);
    }

    RuskelError::Generate(format!("Failed to build rustdoc JSON: {summary}"))
}

/// Extract the first meaningful rustdoc diagnostic from the captured stderr
/// stream.
fn extract_primary_diagnostic(stderr: &str) -> Option<String> {
    let mut lines = stderr.lines().peekable();

    while let Some(line) = lines.next() {
        if !is_primary_error_line(line) {
            continue;
        }

        let mut snippet = vec![line.trim_end().to_string()];

        while let Some(peek) = lines.peek() {
            let trimmed = peek.trim_end();
            if trimmed.is_empty() {
                lines.next();
                break;
            }

            let trimmed_start = trimmed.trim_start_matches(' ');
            let is_line_number_block = trimmed.contains('|')
                && trimmed
                    .split_once('|')
                    .map(|(prefix, _)| prefix.trim().chars().all(|c| c.is_ascii_digit()))
                    .unwrap_or(false);

            let is_context_line = peek.starts_with(' ')
                || peek.starts_with('\t')
                || peek.starts_with('|')
                || trimmed_start.starts_with("-->")
                || trimmed_start.starts_with("note:")
                || trimmed_start.starts_with("help:")
                || trimmed_start.starts_with("warning:")
                || trimmed_start.starts_with("= note:")
                || trimmed_start.starts_with("= help:")
                || trimmed_start.starts_with("= warning:")
                || is_line_number_block;

            if !is_context_line {
                break;
            }

            if let Some(next_line) = lines.next() {
                snippet.push(next_line.trim_end().to_string());
            } else {
                break;
            }
        }

        return Some(snippet.join("\n"));
    }

    None
}

/// Determine whether a line introduces a new primary rustdoc error diagnostic.
fn is_primary_error_line(line: &str) -> bool {
    let trimmed = line.trim();

    if let Some(body) = trimmed.strip_prefix("error[") {
        return body.contains(']');
    }

    if let Some(body) = trimmed.strip_prefix("error:") {
        let body = body.trim_start();
        return !(body.starts_with("Compilation failed")
            || body.starts_with("could not compile")
            || body.starts_with("could not document"));
    }

    false
}

/// Truncate collected diagnostics to a manageable size, returning whether
/// truncation occurred.
fn truncate_diagnostics(stderr: &str) -> (String, bool) {
    let mut buffer = String::new();
    let mut truncated = false;

    for (idx, ch) in stderr.chars().enumerate() {
        if idx >= MAX_STDERR_CHARS {
            truncated = true;
            break;
        }
        buffer.push(ch);
    }

    (buffer, truncated)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn retry_budget_keeps_reasons_independent_with_three_attempts_total() {
        let mut budget = RetryBudget::default();

        assert!(budget.begin_attempt());
        assert!(budget.take_storage_retry());
        assert!(!budget.take_storage_retry());

        assert!(budget.begin_attempt());
        assert!(budget.take_toolchain_retry());
        assert!(!budget.take_toolchain_retry());

        assert!(budget.begin_attempt());
        assert!(!budget.take_storage_retry());
        assert!(!budget.take_toolchain_retry());
        assert!(!budget.begin_attempt());
    }

    #[test]
    fn primary_diagnostic_extracts_compiler_error() -> Result<()> {
        let stderr = r#"
error: expected pattern, found `=`
 --> src/lib.rs:3:9
  |
3 |     let = left + right;
  |         ^ expected pattern

error: Compilation failed, aborting rustdoc
"#;

        let diagnostic = extract_primary_diagnostic(stderr).ok_or_else(|| {
            RuskelError::Generate("failed to find primary diagnostic".to_string())
        })?;
        assert!(diagnostic.contains("expected pattern"));
        assert!(diagnostic.contains("src/lib.rs:3:9"));
        assert!(!diagnostic.contains("Compilation failed"));

        Ok(())
    }

    #[test]
    fn format_rustdoc_failure_includes_diagnostics_when_silent() {
        let stderr = b"error: expected pattern, found `=`\n --> src/lib.rs:3:9\n  |\n3 |     let = left + right;\n  |         ^ expected pattern\n";
        let message = format_rustdoc_failure(stderr, true).to_string();

        assert!(message.contains("Failed to build rustdoc JSON"));
        assert!(message.contains("expected pattern"));
        assert!(message.contains("src/lib.rs:3:9"));
        assert!(message.contains("rustdoc stderr"));
    }

    #[test]
    fn package_target_selection_preserves_library_and_binary_rules() -> Result<()> {
        let package = tempdir()?;
        fs::create_dir_all(package.path().join("src"))?;
        fs::write(package.path().join("src/lib.rs"), "pub struct Library;\n")?;
        fs::write(package.path().join("src/first.rs"), "fn main() {}\n")?;
        fs::write(package.path().join("src/second.rs"), "fn main() {}\n")?;
        fs::write(
            package.path().join("Cargo.toml"),
            r#"[package]
name = "selection-fixture"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[[bin]]
name = "first"
path = "src/first.rs"

[[bin]]
name = "second"
path = "src/second.rs"
"#,
        )?;
        let manifest = package.path().join("Cargo.toml");

        let library = select_package_target(&manifest, true, None)?;
        assert!(matches!(library.package_target, PackageTarget::Lib));
        assert!(library.bin_target.is_none());

        let binary = select_package_target(&manifest, true, Some("second"))?;
        assert!(matches!(
            binary.package_target,
            PackageTarget::Bin(ref name) if name == "second"
        ));
        let metadata = binary.bin_target.expect("binary metadata");
        assert_eq!(metadata.name, "second");
        assert!(!metadata.is_bin_only);

        let error = select_package_target(&manifest, true, Some("missing"))
            .expect_err("unknown binary should fail");
        assert!(
            error
                .to_string()
                .contains("binary target 'missing' not found")
        );

        let bin_only = tempdir()?;
        fs::create_dir_all(bin_only.path().join("src"))?;
        fs::write(bin_only.path().join("src/main.rs"), "fn main() {}\n")?;
        fs::write(
            bin_only.path().join("Cargo.toml"),
            r#"[package]
name = "bin-only"
version = "0.1.0"
edition = "2024"
"#,
        )?;
        let selected = select_package_target(&bin_only.path().join("Cargo.toml"), true, None)?;
        assert!(matches!(
            selected.package_target,
            PackageTarget::Bin(ref name) if name == "bin-only"
        ));
        assert!(selected.bin_target.expect("binary metadata").is_bin_only);

        Ok(())
    }
}
