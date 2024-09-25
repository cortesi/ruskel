use libruskel::{Result, Ruskel};
use tempfile::tempdir;

#[test]
fn test_render_specific_struct() -> Result<()> {
    let temp_dir = tempdir()?;
    let src_dir = temp_dir.path().join("src");
    let lib_path = src_dir.join("lib.rs");
    let foo_path = src_dir.join("foo.rs");
    let cargo_toml_path = temp_dir.path().join("Cargo.toml");

    std::fs::create_dir_all(&src_dir)?;
    std::fs::write(&lib_path, "pub mod foo;")?;
    std::fs::write(&foo_path, "pub struct DummyStruct;")?;
    std::fs::write(
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
    let ruskel = Ruskel::new(&target);
    let output = ruskel.render(false, false, true)?;

    assert!(output.contains("pub struct DummyStruct;"));

    Ok(())
}
