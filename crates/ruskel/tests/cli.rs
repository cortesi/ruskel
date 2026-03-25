//! CLI integration tests for ruskel's top-level flag validation.

use assert_cmd::Command;
use predicates::str::contains;

#[cfg(test)]
mod tests {
    use super::{Command, contains};

    #[test]
    fn mcp_rejects_search_query_flags() {
        let mut command = Command::cargo_bin("ruskel").expect("binary should build");
        command.args(["--mcp", "--search", "widget"]);

        command
            .assert()
            .failure()
            .stderr(contains(
                "--mcp can only be used with --auto-impls, --private, --no-frontmatter, --offline, --verbose, --addr, and --log",
            ));
    }

    #[test]
    fn mcp_rejects_search_domain_overrides() {
        let mut command = Command::cargo_bin("ruskel").expect("binary should build");
        command.args(["--mcp", "--search-spec", "path"]);

        command
            .assert()
            .failure()
            .stderr(contains(
                "--mcp can only be used with --auto-impls, --private, --no-frontmatter, --offline, --verbose, --addr, and --log",
            ));
    }
}
