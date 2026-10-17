//! Cargo fixture setup for integration tests.

use std::{env, process::Command};

/// Launch the test toolchain's Cargo without this package's identity.
pub fn cargo_command() -> Command {
    let mut command = Command::new(env!("CARGO"));
    let package_variables = env::vars_os().map(|(name, _)| name).filter(|name| {
        name.to_str().is_some_and(|name| {
            name.starts_with("CARGO_PKG_")
                || matches!(
                    name,
                    "CARGO_MANIFEST_DIR" | "CARGO_MANIFEST_PATH" | "CARGO_MANIFEST_LINKS"
                )
        })
    });
    for variable in package_variables {
        command.env_remove(variable);
    }
    command
}
