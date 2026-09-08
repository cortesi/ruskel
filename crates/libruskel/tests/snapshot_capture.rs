//! End-to-end tests for canonical workspace snapshot capture.

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Command,
    };

    use libruskel::{
        ApiSnapshot, Result, Ruskel, SnapshotFeatures, SnapshotProfile, SnapshotProfileOptions,
        SnapshotRequest,
    };
    use tempfile::{TempDir, tempdir};

    const TOOLCHAIN: &str = "nightly-2026-07-01";

    struct Fixture {
        root: TempDir,
    }

    impl Fixture {
        fn workspace() -> Self {
            let root = tempdir().expect("create fixture root");
            write_package(
                root.path(),
                "root-api",
                "[features]\nextra = []\n",
                "pub struct Root;\n#[cfg(feature = \"extra\")] pub fn extra() {}\n",
            );
            write_package(
                &root.path().join("crates/alpha"),
                "alpha-api",
                "[lib]\nname = \"alpha_public\"\n[features]\nextra = []\n",
                "pub struct Alpha;\n#[cfg(feature = \"extra\")] pub fn extra() {}\n",
            );
            write_package(
                &root.path().join("crates/private-publish"),
                "private-publish",
                "publish = false\n",
                "pub struct Included;\n",
            );

            let proc_macro = root.path().join("crates/macro-api");
            fs::create_dir_all(proc_macro.join("src")).expect("create proc-macro source");
            fs::write(
            proc_macro.join("src/lib.rs"),
            "extern crate proc_macro;\nuse proc_macro::TokenStream;\n#[proc_macro]\npub fn public_macro(input: TokenStream) -> TokenStream { input }\n",
        )
        .expect("write proc-macro source");
            fs::write(
            proc_macro.join("Cargo.toml"),
            "[package]\nname = \"macro-api\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[lib]\nproc-macro = true\n",
        )
        .expect("write proc-macro manifest");

            let binary = root.path().join("crates/tool");
            fs::create_dir_all(binary.join("src")).expect("create binary source");
            fs::write(binary.join("src/main.rs"), "fn main() {}\n").expect("write binary source");
            fs::write(
                binary.join("Cargo.toml"),
                "[package]\nname = \"workspace-tool\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
            )
            .expect("write binary manifest");

            let root_manifest = root.path().join("Cargo.toml");
            let mut manifest = fs::read_to_string(&root_manifest).expect("read root manifest");
            manifest.push_str(
            "[workspace]\nmembers = [\"crates/alpha\", \"crates/private-publish\", \"crates/macro-api\", \"crates/tool\"]\nresolver = \"2\"\n",
        );
            fs::write(&root_manifest, manifest).expect("write root workspace manifest");
            generate_lockfile(&root_manifest);
            Self { root }
        }

        fn path(&self) -> &Path {
            self.root.path()
        }
    }

    fn write_package(root: &Path, name: &str, extra: &str, source: &str) {
        fs::create_dir_all(root.join("src")).expect("create package source");
        fs::write(root.join("src/lib.rs"), source).expect("write package source");
        fs::write(
            root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n{extra}"
            ),
        )
        .expect("write package manifest");
    }

    fn generate_lockfile(manifest: &Path) {
        let status = Command::new("cargo")
            .args(["generate-lockfile", "--offline", "--manifest-path"])
            .arg(manifest)
            .status()
            .expect("run cargo generate-lockfile");
        assert!(status.success(), "generate fixture lockfile");
    }

    fn profile(features: SnapshotFeatures) -> SnapshotProfile {
        SnapshotProfile::resolve(
            SnapshotProfileOptions::new()
                .with_toolchain(TOOLCHAIN)
                .with_features(features),
        )
        .expect("valid fixture profile")
    }

    fn capture(
        root: &Path,
        inputs: Vec<PathBuf>,
        features: SnapshotFeatures,
    ) -> Result<ApiSnapshot> {
        let request = SnapshotRequest::new(inputs, profile(features))?;
        Ruskel::new()
            .with_offline(true)
            .with_silent(true)
            .with_cache_dir(Some(root.join("cache")))
            .with_auto_impls(true)
            .with_frontmatter(true)
            .with_bin_target(Some("ignored".to_string()))
            .capture_snapshot(&request)
    }

    #[test]
    fn captures_root_workspace_libraries_proc_macros_and_skips_binary() -> Result<()> {
        let fixture = Fixture::workspace();
        let snapshot = capture(
            fixture.path(),
            vec![fixture.path().to_path_buf()],
            SnapshotFeatures::new(true, false, vec!["alpha-api/extra".into()])?,
        )?;

        assert_eq!(
            snapshot
                .crates()
                .iter()
                .map(|entry| (entry.package(), entry.crate_name(), entry.filename()))
                .collect::<Vec<_>>(),
            [
                ("alpha-api", "alpha_public", "alpha-api.rs"),
                ("macro-api", "macro_api", "macro-api.rs"),
                ("private-publish", "private_publish", "private-publish.rs"),
                ("root-api", "root_api", "root-api.rs"),
            ]
        );
        assert_eq!(snapshot.skipped_packages(), ["workspace-tool"]);
        assert_eq!(
            snapshot.profile().features().features(),
            ["alpha-api/extra"]
        );
        assert!(snapshot.crates()[0].contents().starts_with(
            "// @generated by `ruskel-snapshot`; do not edit.\n\npub mod alpha_public {"
        ));
        assert!(snapshot.crates()[0].contents().contains("pub fn extra()"));
        for entry in snapshot.crates() {
            assert!(!entry.contents().contains('\r'));
            assert!(entry.contents().ends_with("}\n"));
            assert!(!entry.contents().ends_with("}\n\n"));
        }
        assert!(
            snapshot
                .crates()
                .iter()
                .find(|entry| entry.package() == "macro-api")
                .expect("proc macro snapshot")
                .contents()
                .contains("pub fn public_macro")
        );
        Ok(())
    }

    #[test]
    fn direct_member_selection_is_deduplicated_and_location_independent() -> Result<()> {
        let first = Fixture::workspace();
        let second = Fixture::workspace();
        let relative = PathBuf::from("crates/alpha/Cargo.toml");
        let first_manifest = first.path().join(&relative);
        let first_snapshot = capture(
            first.path(),
            vec![first_manifest.clone(), first_manifest],
            SnapshotFeatures::new(true, false, vec!["extra".into()])?,
        )?;
        let second_snapshot = capture(
            second.path(),
            vec![second.path().join(relative)],
            SnapshotFeatures::new(true, false, vec!["extra".into()])?,
        )?;

        assert_eq!(first_snapshot.crates().len(), 1);
        assert_eq!(first_snapshot.crates(), second_snapshot.crates());
        assert_eq!(first_snapshot.profile(), second_snapshot.profile());
        Ok(())
    }

    #[test]
    fn repeated_and_version_only_captures_are_identical() -> Result<()> {
        let fixture = Fixture::workspace();
        let member = fixture.path().join("crates/private-publish/Cargo.toml");
        let first = capture(
            fixture.path(),
            vec![member.clone()],
            SnapshotFeatures::default(),
        )?;
        let second = capture(
            fixture.path(),
            vec![member.clone()],
            SnapshotFeatures::default(),
        )?;
        assert_eq!(first, second);

        let manifest =
            fs::read_to_string(&member)?.replace("version = \"0.1.0\"", "version = \"9.8.7\"");
        fs::write(&member, manifest)?;
        generate_lockfile(&fixture.path().join("Cargo.toml"));
        let version_changed = capture(fixture.path(), vec![member], SnapshotFeatures::default())?;
        assert_eq!(first, version_changed);
        Ok(())
    }

    #[test]
    fn direct_binary_only_capture_fails_without_partial_snapshot() {
        let fixture = Fixture::workspace();
        let error = capture(
            fixture.path(),
            vec![fixture.path().join("crates/tool")],
            SnapshotFeatures::default(),
        )
        .expect_err("binary-only selection must fail");
        assert!(error.to_string().contains("no library or procedural-macro"));
    }
}
