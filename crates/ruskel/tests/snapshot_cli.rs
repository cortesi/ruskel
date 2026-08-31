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
            let mut command = Command::cargo_bin("ruskel-snapshot").expect("snapshot binary");
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
        let output = ProcessCommand::new("rustup")
            .args(["run", TOOLCHAIN, "rustc", "-vV"])
            .output()
            .expect("inspect toolchain");
        assert!(output.status.success(), "fixture toolchain");
        String::from_utf8(output.stdout)
            .expect("UTF-8 rustc output")
            .lines()
            .find_map(|line| line.strip_prefix("host: "))
            .expect("host target")
            .to_string()
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
    fn mixed_workspace_update_reuse_offline_check_and_migration() {
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
                "changed     .ruskel-snapshot.toml\n",
                "changed     alpha-api.rs\n",
                "changed     beta-api.rs\n",
                "skipped     workspace-tool (no library target)\n",
            )
        );

        let mut repeat = Command::cargo_bin("ruskel-snapshot").expect("snapshot binary");
        repeat
            .args([
                "--output",
                output.to_str().expect("UTF-8 output"),
                "--offline",
            ])
            .args(inputs.iter());
        assert_eq!(
            success_stdout(&mut repeat),
            concat!(
                "unchanged   .ruskel-snapshot.toml\n",
                "unchanged   alpha-api.rs\n",
                "unchanged   beta-api.rs\n",
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
        let mut check = Command::cargo_bin("ruskel-snapshot").expect("snapshot binary");
        check
            .args([
                "--output",
                output.to_str().expect("UTF-8 output"),
                "--check",
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

        let mut migration = Command::cargo_bin("ruskel-snapshot").expect("snapshot binary");
        migration
            .args([
                "--output",
                output.to_str().expect("UTF-8 output"),
                "--no-default-features",
            ])
            .args(inputs.iter());
        let migrated = success_stdout(&mut migration);
        assert!(migrated.starts_with("changed     .ruskel-snapshot.toml\n"));
        assert!(
            fs::read_to_string(output.join(".ruskel-snapshot.toml"))
                .expect("marker")
                .contains("default_features = false")
        );
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
        let mut drift = Command::cargo_bin("ruskel-snapshot").expect("snapshot binary");
        drift
            .args([
                "--output",
                output.to_str().expect("UTF-8 output"),
                "--check",
            ])
            .arg(&alpha);
        drift.assert().code(1).stdout(contains(
            "unchanged   .ruskel-snapshot.toml\nchanged     alpha-api.rs\n",
        ));

        let backup = fixture.path().join(".api.ruskel-snapshot-backup-test");
        fs::rename(&output, &backup).expect("simulate interrupted swap");
        fs::write(alpha.join("src/lib.rs"), "pub struct Alpha;\n").expect("original source");
        let mut interrupted = Command::cargo_bin("ruskel-snapshot").expect("snapshot binary");
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
            .stderr(contains("non-empty destination has no ownership marker"));
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
        capture.arg(fixture.path());
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
        let mut ordinary = Command::cargo_bin("ruskel").expect("ruskel binary");
        ordinary
            .current_dir(fixture.path())
            .args(["snapshot", "--no-page"]);
        ordinary
            .assert()
            .success()
            .stdout(contains("pub struct Target"));
    }
}
