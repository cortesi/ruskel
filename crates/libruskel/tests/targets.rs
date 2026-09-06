//! Integration tests for resolving filesystem targets.

use std::fs;

use libruskel::{CrateRequest, Result, Ruskel};
use tempfile::tempdir;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_specific_struct() -> Result<()> {
        let temp_dir = tempdir()?;
        let src_dir = temp_dir.path().join("src");
        let lib_path = src_dir.join("lib.rs");
        let foo_path = src_dir.join("foo.rs");
        let cargo_toml_path = temp_dir.path().join("Cargo.toml");

        fs::create_dir_all(&src_dir)?;
        fs::write(&lib_path, "pub mod foo;")?;
        fs::write(&foo_path, "pub struct DummyStruct;")?;
        fs::write(
            &cargo_toml_path,
            r#"
            [package]
            name = "dummy_crate"
            version = "0.1.0"
            edition = "2021"

            [dependencies]
            "#,
        )?;

        let target = format!("{}::DummyStruct", foo_path.display());
        let ruskel = Ruskel::new()
            .with_cache_dir(Some(temp_dir.path().join("ruskel-cache")))
            .with_silent(true);
        let output = ruskel.render(&target, &CrateRequest::default())?;

        assert!(output.contains("pub struct DummyStruct;"));

        Ok(())
    }

    #[test]
    fn test_render_library_crate_root_file() -> Result<()> {
        let temp_dir = tempdir()?;
        let src_dir = temp_dir.path().join("src");
        let lib_path = src_dir.join("lib.rs");
        let cargo_toml_path = temp_dir.path().join("Cargo.toml");

        fs::create_dir_all(&src_dir)?;
        fs::write(&lib_path, "pub struct LibraryRoot;\n")?;
        fs::write(
            &cargo_toml_path,
            r#"[package]
name = "library-root-fixture"
version = "0.1.0"
edition = "2024"
"#,
        )?;

        let target = format!("{}::LibraryRoot", lib_path.display());
        let output = Ruskel::new()
            .with_cache_dir(Some(temp_dir.path().join("ruskel-cache")))
            .with_silent(true)
            .render(&target, &CrateRequest::default())?;

        assert!(output.contains("pub struct LibraryRoot;"));
        Ok(())
    }

    #[test]
    fn test_render_binary_crate_root_file() -> Result<()> {
        let temp_dir = tempdir()?;
        let src_dir = temp_dir.path().join("src");
        let main_path = src_dir.join("main.rs");
        let cargo_toml_path = temp_dir.path().join("Cargo.toml");

        fs::create_dir_all(&src_dir)?;
        fs::write(&main_path, "pub struct BinaryRoot;\nfn main() {}\n")?;
        fs::write(
            &cargo_toml_path,
            r#"[package]
name = "binary-root-fixture"
version = "0.1.0"
edition = "2024"
"#,
        )?;

        let target = format!("{}::BinaryRoot", main_path.display());
        let output = Ruskel::new()
            .with_cache_dir(Some(temp_dir.path().join("ruskel-cache")))
            .with_silent(true)
            .render(&target, &CrateRequest::default())?;

        assert!(output.contains("pub struct BinaryRoot;"));
        Ok(())
    }
}
