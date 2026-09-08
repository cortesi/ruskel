//! Process-level checks for the `ruskel-snapshot` command.

#[cfg(test)]
mod tests {
    use std::{fs, path::Path, process::Command as ProcessCommand};

    use assert_cmd::Command;
    use predicates::str::contains;
    use tempfile::{TempDir, tempdir};

    const TOOLCHAIN: &str = "nightly-2026-07-01";

    /// One locked mixed Cargo workspace.
    struct Fixture {
        root: TempDir,
        target: String,
    }

    impl Fixture {
        /// Create libraries with features and one binary-only member.
        fn mixed() -> Self {
            let root = tempdir().expect("fixture root");
            write_library(
                root.path(),
                "root-api",
                "[features]\nextra = []\n",
                "pub struct Root;\n#[cfg(feature = \"extra\")] pub fn extra() {}\n",
            );
            write_library(
                &root.path().join("crates/alpha"),
                "alpha-api",
                "[features]\nextra = []\n",
                "pub struct Alpha;\n#[cfg(feature = \"extra\")] pub fn extra() {}\n",
            );
            write_library(
                &root.path().join("crates/beta"),
                "beta-api",
                "",
                "pub fn beta() {}\n",
            );
            write_binary(&root.path().join("crates/tool"), "workspace-tool");
            let manifest = root.path().join("Cargo.toml");
            let mut contents = fs::read_to_string(&manifest).expect("root manifest");
            contents.push_str(
                "[workspace]\nmembers = [\"crates/alpha\", \"crates/beta\", \"crates/tool\"]\nresolver = \"2\"\n",
            );
            fs::write(&manifest, contents).expect("workspace manifest");
            generate_lockfile(&manifest);
            Self {
                root,
                target: host_target(),
            }
        }

        /// Return the workspace path.
        fn path(&self) -> &Path {
            self.root.path()
        }

        /// Build a snapshot command with explicit first-capture profile values.
        fn command(&self, output: &Path) -> Command {
            let mut command = cargo_binary("ruskel-snapshot");
            command.args([
                "--output",
                output.to_str().expect("UTF-8 output"),
                "--cache-dir",
                self.path().join("cache").to_str().expect("UTF-8 cache"),
                "--toolchain",
                TOOLCHAIN,
                "--target",
                &self.target,
            ]);
            command
        }
    }

    /// Write one library package.
    fn write_library(root: &Path, name: &str, extra: &str, source: &str) {
        fs::create_dir_all(root.join("src")).expect("source directory");
        fs::write(root.join("src/lib.rs"), source).expect("library source");
        fs::write(
            root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n{extra}"
            ),
        )
        .expect("package manifest");
    }

    /// Write one binary-only package.
    fn write_binary(root: &Path, name: &str) {
        fs::create_dir_all(root.join("src")).expect("source directory");
        fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("binary source");
        fs::write(
            root.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n"),
        )
        .expect("package manifest");
    }

    /// Create a lockfile without network access.
    fn generate_lockfile(manifest: &Path) {
        let status = ProcessCommand::new("cargo")
            .args(["generate-lockfile", "--offline", "--manifest-path"])
            .arg(manifest)
            .status()
            .expect("cargo generate-lockfile");
        assert!(status.success(), "fixture lockfile");
    }

    /// Return the installed fixture toolchain host.
    fn host_target() -> String {
        let mut command = ProcessCommand::new("rustup");
        remove_loader_paths(&mut command);
        let output = command
            .args(["run", TOOLCHAIN, "rustc", "-vV"])
            .output()
            .expect("inspect toolchain");
        assert!(
            output.status.success(),
            "fixture toolchain: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("UTF-8 rustc output")
            .lines()
            .find_map(|line| line.strip_prefix("host: "))
            .expect("host target")
            .to_string()
    }

    /// Build a CLI command without Nextest's current-toolchain loader paths.
    fn cargo_binary(name: &str) -> Command {
        let mut command = Command::cargo_bin(name).expect("Cargo binary");
        for variable in [
            "DYLD_LIBRARY_PATH",
            "DYLD_FALLBACK_LIBRARY_PATH",
            "LD_LIBRARY_PATH",
        ] {
            command.env_remove(variable);
        }
        command
    }

    /// Prevent a selected toolchain from loading another toolchain's LLVM.
    fn remove_loader_paths(command: &mut ProcessCommand) {
        for variable in [
            "DYLD_LIBRARY_PATH",
            "DYLD_FALLBACK_LIBRARY_PATH",
            "LD_LIBRARY_PATH",
        ] {
            command.env_remove(variable);
        }
    }

    /// Return stdout from one successful command.
    fn success_stdout(command: &mut Command) -> String {
        let output = command.output().expect("run snapshot command");
        assert!(
            output.status.success(),
            "command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("UTF-8 stdout")
    }

    #[test]
    fn mixed_workspace_update_check_and_invocation_profile() {
        let fixture = Fixture::mixed();
        let output = fixture.path().join("api");
        let inputs = [
            fixture.path().join("crates/alpha"),
            fixture.path().join("crates/beta"),
            fixture.path().join("crates/tool"),
        ];
        let mut first = fixture.command(&output);
        first.arg("--features").arg("alpha-api/extra");
        first.args(inputs.iter());
        assert_eq!(
            success_stdout(&mut first),
            concat!(
                "changed     alpha-api.rs\n",
                "changed     beta-api.rs\n",
                "skipped     workspace-tool (no library target)\n",
            )
        );
        fs::write(
            output.join(".ruskel-snapshot.toml"),
            "# @generated by `ruskel-snapshot`; do not edit.\nformat = 1\n",
        )
        .expect("legacy marker");

        let mut repeat = cargo_binary("ruskel-snapshot");
        repeat
            .args([
                "--output",
                output.to_str().expect("UTF-8 output"),
                "--offline",
                "--features",
                "alpha-api/extra",
            ])
            .args(inputs.iter());
        assert_eq!(
            success_stdout(&mut repeat),
            concat!(
                "unchanged   alpha-api.rs\n",
                "unchanged   beta-api.rs\n",
                "removed     .ruskel-snapshot.toml\n",
                "skipped     workspace-tool (no library target)\n",
            )
        );

        let before = fs::read_dir(&output)
            .expect("snapshot tree")
            .map(|entry| {
                let path = entry.expect("tree entry").path();
                (
                    path.file_name().expect("file name").to_owned(),
                    fs::read(path).expect("bytes"),
                )
            })
            .collect::<Vec<_>>();
        let mut check = cargo_binary("ruskel-snapshot");
        check
            .args([
                "--output",
                output.to_str().expect("UTF-8 output"),
                "--check",
                "--features",
                "alpha-api/extra",
            ])
            .args(inputs.iter());
        check
            .assert()
            .success()
            .stdout(contains("unchanged   alpha-api.rs"));
        let after = fs::read_dir(&output)
            .expect("snapshot tree")
            .map(|entry| {
                let path = entry.expect("tree entry").path();
                (
                    path.file_name().expect("file name").to_owned(),
                    fs::read(path).expect("bytes"),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(before, after, "check preserves recursive bytes");

        let mut defaults = cargo_binary("ruskel-snapshot");
        defaults
            .args(["--output", output.to_str().expect("UTF-8 output")])
            .args(inputs.iter());
        assert_eq!(
            success_stdout(&mut defaults),
            concat!(
                "changed     alpha-api.rs\n",
                "unchanged   beta-api.rs\n",
                "skipped     workspace-tool (no library target)\n",
            )
        );
        let mut entries = fs::read_dir(&output)
            .expect("snapshot tree")
            .map(|entry| entry.expect("tree entry").file_name())
            .collect::<Vec<_>>();
        entries.sort();
        assert_eq!(entries, ["alpha-api.rs", "beta-api.rs"]);
    }

    #[test]
    fn check_reports_drift_and_interrupted_swap_with_status_one() {
        let fixture = Fixture::mixed();
        let output = fixture.path().join("api");
        let alpha = fixture.path().join("crates/alpha");
        let mut first = fixture.command(&output);
        first.arg(&alpha);
        first.assert().success();

        fs::write(alpha.join("src/lib.rs"), "pub struct Changed;\n").expect("changed source");
        let mut drift = cargo_binary("ruskel-snapshot");
        drift
            .args([
                "--output",
                output.to_str().expect("UTF-8 output"),
                "--check",
            ])
            .arg(&alpha);
        drift
            .assert()
            .code(1)
            .stdout(contains("changed     alpha-api.rs\n"));

        let backup = fixture.path().join(".api.ruskel-snapshot-backup-test");
        fs::rename(&output, &backup).expect("simulate interrupted swap");
        fs::write(alpha.join("src/lib.rs"), "pub struct Alpha;\n").expect("original source");
        let mut interrupted = cargo_binary("ruskel-snapshot");
        interrupted
            .args([
                "--output",
                output.to_str().expect("UTF-8 output"),
                "--check",
            ])
            .arg(&alpha);
        interrupted
            .assert()
            .code(1)
            .stdout(contains("interrupted .api.ruskel-snapshot-backup-test"));
        assert!(backup.exists());
        assert!(!output.exists());
    }

    #[test]
    fn feature_errors_binary_only_and_output_safety_use_status_two() {
        let fixture = Fixture::mixed();
        let output = fixture.path().join("api");
        let mut unqualified = fixture.command(&output);
        unqualified.args(["--features", "extra"]).args([
            fixture.path().join("crates/alpha"),
            fixture.path().join("crates/beta"),
        ]);
        unqualified
            .assert()
            .code(2)
            .stderr(contains("package/feature"));
        assert!(!output.exists());

        let mut binary = fixture.command(&output);
        binary.arg(fixture.path().join("crates/tool"));
        binary
            .assert()
            .code(2)
            .stderr(contains("no library or procedural-macro"));
        assert!(!output.exists());

        fs::create_dir(&output).expect("output directory");
        fs::write(output.join("notes.txt"), "owned by user\n").expect("unowned file");
        let mut unsafe_update = fixture.command(&output);
        unsafe_update.arg(fixture.path().join("crates/alpha"));
        unsafe_update
            .assert()
            .code(2)
            .stderr(contains("destination contains unowned entry 'notes.txt'"));
        assert_eq!(
            fs::read_to_string(output.join("notes.txt")).expect("preserved file"),
            "owned by user\n"
        );
    }

    #[test]
    fn workspace_root_selects_members_and_ruskel_keeps_snapshot_as_a_target() {
        let fixture = Fixture::mixed();
        let output = fixture.path().join("api");
        let mut capture = fixture.command(&output);
        capture
            .current_dir(fixture.path().join("crates/alpha"))
            .arg("--workspace");
        let stdout = success_stdout(&mut capture);
        assert!(stdout.contains("changed     root-api.rs\n"));
        assert!(stdout.ends_with("skipped     workspace-tool (no library target)\n"));

        let snapshot = fixture.path().join("crates/snapshot");
        write_library(&snapshot, "snapshot", "", "pub struct Target;\n");
        let manifest = fixture.path().join("Cargo.toml");
        let contents = fs::read_to_string(&manifest)
            .expect("workspace manifest")
            .replace("\"crates/tool\"]", "\"crates/tool\", \"crates/snapshot\"]");
        fs::write(&manifest, contents).expect("workspace manifest");
        generate_lockfile(&manifest);
        let mut ordinary = cargo_binary("ruskel");
        ordinary
            .current_dir(fixture.path())
            .args(["snapshot", "--no-page"]);
        ordinary
            .assert()
            .success()
            .stdout(contains("pub struct Target"));
    }
}
