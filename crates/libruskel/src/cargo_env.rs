//! Child Cargo commands without inherited package identity.

use std::{env, ffi::OsStr, process::Command};

/// Preserve build settings and remove the launching package's Cargo metadata.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    for (name, _) in env::vars_os() {
        let Some(key) = name.to_str() else {
            continue;
        };
        if key.starts_with("CARGO_PKG_")
            || matches!(
                key,
                "CARGO_MANIFEST_DIR" | "CARGO_MANIFEST_PATH" | "CARGO_MANIFEST_LINKS"
            )
        {
            command.env_remove(name);
        }
    }
    command
}
