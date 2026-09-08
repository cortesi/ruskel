//! End-to-end coverage for retained helper attributes in canonical snapshots.

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, process::Command};

    use libruskel::{
        ApiSnapshot, Result, Ruskel, SnapshotFeatures, SnapshotProfile, SnapshotProfileOptions,
        SnapshotRequest,
    };
    use tempfile::{TempDir, tempdir};

    const TOOLCHAIN: &str = "nightly-2026-07-01";

    /// Temporary workspace containing a proc-macro helper and its API crate.
    struct Fixture {
        /// Workspace root kept alive for the duration of the test.
        root: TempDir,
        /// API package manifest used as the capture input.
        api_manifest: PathBuf,
    }

    impl Fixture {
        /// Create a helper-attribute fixture with a locked workspace.
        fn new() -> Result<Self> {
            let root = tempdir()?;
            let helper = root.path().join("helper");
            let api = root.path().join("api");
            fs::create_dir_all(helper.join("src"))?;
            fs::create_dir_all(api.join("src"))?;
            fs::write(
                root.path().join("Cargo.toml"),
                "[workspace]\nmembers = [\"helper\", \"api\"]\nresolver = \"3\"\n",
            )?;
            fs::write(
                helper.join("Cargo.toml"),
                "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[lib]\nproc-macro = true\n",
            )?;
            fs::write(
                helper.join("src/lib.rs"),
                r#"use proc_macro::TokenStream;

#[proc_macro_derive(Helper, attributes(allowance, stable_api, derive_more, testable))]
pub fn helper(_: TokenStream) -> TokenStream {
    TokenStream::new()
}
"#,
            )?;
            fs::write(
                api.join("Cargo.toml"),
                "[package]\nname = \"helper-api\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nhelper = { path = \"../helper\" }\n",
            )?;
            fs::write(api.join("src/lib.rs"), api_source("#[stable_api]"))?;
            let status = Command::new("cargo")
                .args(["generate-lockfile", "--offline", "--manifest-path"])
                .arg(root.path().join("Cargo.toml"))
                .status()?;
            if !status.success() {
                return Err(libruskel::RuskelError::Generate(
                    "helper-attribute fixture lockfile generation failed".to_string(),
                ));
            }
            Ok(Self {
                root,
                api_manifest: api.join("Cargo.toml"),
            })
        }

        /// Replace the API source while retaining the same workspace and cache.
        fn write_api_source(&self, stable_attribute: &str) -> Result<()> {
            let api_source_path = self
                .api_manifest
                .parent()
                .ok_or_else(|| {
                    libruskel::RuskelError::Generate(
                        "helper-attribute fixture has no API parent".to_string(),
                    )
                })?
                .join("src/lib.rs");
            fs::write(api_source_path, api_source(stable_attribute))?;
            Ok(())
        }

        /// Capture the API package through the public snapshot boundary.
        fn capture(&self) -> Result<ApiSnapshot> {
            let profile = SnapshotProfile::resolve(
                SnapshotProfileOptions::new()
                    .with_toolchain(TOOLCHAIN)
                    .with_features(SnapshotFeatures::default()),
            )?;
            let request = SnapshotRequest::new(vec![self.api_manifest.clone()], profile)?;
            Ruskel::new()
                .with_offline(true)
                .with_silent(true)
                .with_cache_dir(Some(self.root.path().join("cache")))
                .capture_snapshot(&request)
        }
    }

    /// Source for the API package, with one selected helper attribute varied.
    fn api_source(stable_attribute: &str) -> String {
        format!(
            "use helper::Helper;\n\n#[derive(Helper)]\n#[allowance]\n{stable_attribute}\n#[derive_more]\n#[testable]\npub struct Exported;\n"
        )
    }

    /// Find the sole API crate's generated source.
    fn api_contents(snapshot: &ApiSnapshot) -> &str {
        snapshot
            .crates()
            .iter()
            .find(|entry| entry.package() == "helper-api")
            .expect("helper API snapshot")
            .contents()
    }

    #[test]
    fn changing_retained_helper_attribute_changes_snapshot_artifact() -> Result<()> {
        let fixture = Fixture::new()?;
        let first = fixture.capture()?;
        let first_contents = api_contents(&first);
        for attribute in [
            "#[allowance]",
            "#[stable_api]",
            "#[derive_more]",
            "#[testable]",
        ] {
            assert!(
                first_contents.contains(attribute),
                "snapshot lost retained helper attribute {attribute}:\n{first_contents}"
            );
        }

        fixture.write_api_source("#[stable_api(note = \"changed\")]")?;
        let second = fixture.capture()?;
        let second_contents = api_contents(&second);
        assert!(second_contents.contains("#[stable_api(note = \"changed\")]"));
        assert_ne!(first_contents, second_contents);
        Ok(())
    }
}
