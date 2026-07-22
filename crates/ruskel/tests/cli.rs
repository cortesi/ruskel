//! CLI integration tests for ruskel's top-level flag validation.

use std::fs;

use assert_cmd::Command;
use predicates::str::contains;
use tempfile::tempdir;

#[cfg(test)]
mod tests {
    use super::{Command, contains, fs, tempdir};

    const MCP_FLAGS_ERROR: &str = "--mcp can only be used with --cache-dir, --auto-impls, --private, --no-frontmatter, --offline, --verbose, --addr, and --log";

    #[test]
    fn mcp_rejects_search_query_flags() {
        let mut command = Command::cargo_bin("ruskel").expect("binary should build");
        command.args(["--mcp", "--search", "widget"]);

        command.assert().failure().stderr(contains(MCP_FLAGS_ERROR));
    }

    #[test]
    fn mcp_rejects_search_domain_overrides() {
        let mut command = Command::cargo_bin("ruskel").expect("binary should build");
        command.args(["--mcp", "--search-spec", "path"]);

        command.assert().failure().stderr(contains(MCP_FLAGS_ERROR));
    }

    #[test]
    fn cache_commands_conflict_with_targets_mcp_and_each_other() {
        for arguments in [
            vec!["--cache-status", "--clean-cache"],
            vec!["--cache-status", "--mcp"],
            vec!["--clean-cache", "./"],
        ] {
            let mut command = Command::cargo_bin("ruskel").expect("binary should build");
            command.args(arguments);
            command
                .assert()
                .failure()
                .stderr(contains("cannot be used with"));
        }
    }

    #[test]
    fn cache_status_uses_explicit_root_before_environment_without_nightly() {
        let temp = tempdir().expect("temporary directory");
        let environment_root = temp.path().join("environment");
        let explicit_root = temp.path().join("explicit");
        let mut command = Command::cargo_bin("ruskel").expect("binary should build");
        command
            .args([
                "--cache-status",
                "--cache-dir",
                explicit_root.to_str().expect("UTF-8 path"),
            ])
            .env("RUSKEL_CACHE_DIR", &environment_root)
            .env("PATH", "");

        let assertion = command.assert().success();
        let canonical = fs::canonicalize(explicit_root).expect("canonical explicit cache root");
        assertion.stdout(contains(format!("Cache root: {}", canonical.display())));
        assert!(!environment_root.exists());
    }

    #[test]
    fn cache_status_uses_environment_root_without_nightly() {
        let temp = tempdir().expect("temporary directory");
        let environment_root = temp.path().join("environment");
        let mut command = Command::cargo_bin("ruskel").expect("binary should build");
        command
            .arg("--cache-status")
            .env("RUSKEL_CACHE_DIR", &environment_root)
            .env("PATH", "");

        let assertion = command.assert().success();
        let canonical = fs::canonicalize(environment_root).expect("canonical environment root");
        assertion.stdout(contains(format!("Cache root: {}", canonical.display())));
    }

    #[test]
    fn clean_cache_prints_report_and_removes_owned_data_without_nightly() {
        let temp = tempdir().expect("temporary directory");
        let root = temp.path().join("cache");
        let mut status = Command::cargo_bin("ruskel").expect("binary should build");
        status
            .args([
                "--cache-status",
                "--cache-dir",
                root.to_str().expect("UTF-8 path"),
            ])
            .env("PATH", "")
            .assert()
            .success();
        let entry = root.join("build").join("a".repeat(64)).join("b".repeat(64));
        fs::create_dir_all(&entry).expect("cache fixture directory");
        fs::write(entry.join("artifact"), b"data").expect("cache fixture artifact");

        let mut clean = Command::cargo_bin("ruskel").expect("binary should build");
        clean
            .args([
                "--clean-cache",
                "--cache-dir",
                root.to_str().expect("UTF-8 path"),
            ])
            .env("PATH", "")
            .assert()
            .success()
            .stdout(contains("Clean result: complete"))
            .stdout(contains("Removed entries: 1"));
        assert!(
            root.join("build")
                .read_dir()
                .expect("build directory")
                .next()
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn clean_cache_returns_failure_for_partial_removal() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempdir().expect("temporary directory");
        let root = temp.path().join("cache");
        let mut status = Command::cargo_bin("ruskel").expect("binary should build");
        status
            .args([
                "--cache-status",
                "--cache-dir",
                root.to_str().expect("UTF-8 path"),
            ])
            .assert()
            .success();
        let workspace = root.join("build").join("c".repeat(64)).join("d".repeat(64));
        fs::create_dir_all(&workspace).expect("cache fixture directory");
        fs::write(workspace.join("artifact"), b"data").expect("cache fixture artifact");
        fs::set_permissions(&workspace, fs::Permissions::from_mode(0o500))
            .expect("protect cache fixture");

        let mut clean = Command::cargo_bin("ruskel").expect("binary should build");
        clean
            .args([
                "--clean-cache",
                "--cache-dir",
                root.to_str().expect("UTF-8 path"),
            ])
            .assert()
            .failure()
            .stdout(contains("Clean result: partial"))
            .stdout(contains("Failures: 1"));

        for trash in root.join("trash").read_dir().expect("trash directory") {
            let protected = trash.expect("trash entry").path().join("d".repeat(64));
            if protected.exists() {
                fs::set_permissions(protected, fs::Permissions::from_mode(0o700))
                    .expect("restore fixture permissions");
            }
        }
    }
}
