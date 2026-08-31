//! Command-line interface for canonical workspace API snapshots.

use std::{path::PathBuf, process::ExitCode};

use clap::Parser;
use libruskel::{
    Ruskel, SnapshotChangeKind, SnapshotFeatures, SnapshotMode, SnapshotProfile,
    SnapshotProfileOptions, SnapshotRequest, SnapshotStore,
};

/// Capture canonical public API snapshots for Cargo packages.
#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "Captures canonical workspace API snapshots",
    long_about = None
)]
struct Cli {
    /// Destination for the generated snapshot tree.
    #[arg(long, value_name = "DIR")]
    output: PathBuf,

    /// Compare the capture without changing the destination.
    #[arg(long)]
    check: bool,

    /// Require Cargo to use only local dependencies.
    #[arg(long)]
    offline: bool,

    /// Show Cargo diagnostics during capture.
    #[arg(long)]
    verbose: bool,

    /// Override the dedicated Ruskel cache root.
    #[arg(long, value_name = "DIR", env = "RUSKEL_CACHE_DIR")]
    cache_dir: Option<PathBuf>,

    /// Disable default features for all selected packages.
    #[arg(long)]
    no_default_features: bool,

    /// Enable all features for all selected packages.
    #[arg(long, conflicts_with_all = ["no_default_features", "features"])]
    all_features: bool,

    /// Enable comma-separated features. Qualify multi-package features as
    /// PACKAGE/FEATURE.
    #[arg(long, value_delimiter = ',', value_name = "FEATURES")]
    features: Vec<String>,

    /// Select a portable dated-nightly toolchain.
    #[arg(long, value_name = "NIGHTLY")]
    toolchain: Option<String>,

    /// Select one installed target triple.
    #[arg(long, value_name = "TRIPLE")]
    target: Option<String>,

    /// Cargo manifests or directories that contain Cargo.toml.
    #[arg(required = true, value_name = "INPUT")]
    inputs: Vec<PathBuf>,
}

impl Cli {
    /// Build only the profile values that the caller explicitly selected.
    fn profile_options(&self) -> libruskel::Result<SnapshotProfileOptions> {
        let mut options = SnapshotProfileOptions::new();
        if let Some(toolchain) = &self.toolchain {
            options = options.with_toolchain(toolchain);
        }
        if let Some(target) = &self.target {
            options = options.with_target(target);
        }
        if self.no_default_features || self.all_features || !self.features.is_empty() {
            options = options.with_features(SnapshotFeatures::new(
                !self.no_default_features,
                self.all_features,
                self.features.clone(),
            )?);
        }
        Ok(options)
    }
}

/// Run one capture and return the process status.
fn run(cli: Cli) -> libruskel::Result<ExitCode> {
    let mode = if cli.check {
        SnapshotMode::Check
    } else {
        SnapshotMode::Update
    };
    let store = SnapshotStore::open(&cli.output, mode)?;
    let profile = SnapshotProfile::resolve(store.profile(), cli.profile_options()?)?;
    let request = SnapshotRequest::new(cli.inputs, profile)?;
    let snapshot = Ruskel::new()
        .with_cache_dir(cli.cache_dir)
        .with_offline(cli.offline)
        .with_silent(!cli.verbose)
        .capture_snapshot(&request)?;
    let report = store.sync(&snapshot)?;

    for change in report.changes() {
        let status = match change.kind() {
            SnapshotChangeKind::Added | SnapshotChangeKind::Changed => "changed",
            SnapshotChangeKind::Removed => "removed",
            SnapshotChangeKind::Unchanged => "unchanged",
            SnapshotChangeKind::Unexpected => "unexpected",
            SnapshotChangeKind::Interrupted => "interrupted",
        };
        println!("{status:<11} {}", change.path().display());
    }
    for package in report.skipped_packages() {
        println!("{:<11} {package} (no library target)", "skipped");
    }

    Ok(if cli.check && !report.is_current() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(status) => status,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_complete_command_surface() {
        let cli = Cli::try_parse_from([
            "ruskel-snapshot",
            "--output",
            "api",
            "--check",
            "--offline",
            "--verbose",
            "--cache-dir",
            "cache",
            "--no-default-features",
            "--features",
            "alpha/extra,beta/std",
            "--toolchain",
            "nightly-2026-07-01",
            "--target",
            "aarch64-apple-darwin",
            "crates/alpha",
            "crates/beta/Cargo.toml",
        ])
        .expect("valid arguments");

        assert_eq!(cli.output, PathBuf::from("api"));
        assert!(cli.check && cli.offline && cli.verbose);
        assert_eq!(cli.features, ["alpha/extra", "beta/std"]);
        assert_eq!(cli.inputs.len(), 2);
        let options = cli.profile_options().expect("valid features");
        let features = options.features().expect("explicit features");
        assert!(!features.default_features());
    }

    #[test]
    fn omitted_profile_flags_reuse_the_stored_profile() {
        let cli = Cli::try_parse_from(["ruskel-snapshot", "--output", "api", "."])
            .expect("valid arguments");
        let options = cli.profile_options().expect("valid defaults");

        assert!(options.toolchain().is_none());
        assert!(options.target().is_none());
        assert!(options.features().is_none());
    }

    #[test]
    fn all_features_conflicts_are_argument_errors() {
        for arguments in [
            vec![
                "ruskel-snapshot",
                "--output",
                "api",
                "--all-features",
                "--no-default-features",
                ".",
            ],
            vec![
                "ruskel-snapshot",
                "--output",
                "api",
                "--all-features",
                "--features",
                "extra",
                ".",
            ],
        ] {
            assert_eq!(
                Cli::try_parse_from(arguments)
                    .expect_err("conflicting arguments")
                    .exit_code(),
                2
            );
        }
    }
}
