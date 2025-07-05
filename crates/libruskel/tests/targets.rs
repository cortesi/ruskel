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
    let ruskel = Ruskel::new().with_silent(true);
    let output = ruskel.render(&target, false, false, Vec::new(), false)?;

    assert!(output.contains("pub struct DummyStruct;"));

    Ok(())
}

#[test]
fn test_target_arch_with_riscv() -> Result<()> {
    // Test the specific case from the issue: ESP32-C6 with riscv32imac-unknown-none-elf
    let ruskel = Ruskel::new()
        .with_silent(true)
        .with_target_arch(Some("riscv32imac-unknown-none-elf".to_string()));

    let output = ruskel
        .render("esp-hal", false, false, vec!["esp32c6".to_string()], false)
        .expect("Failed to render esp-hal with esp32c6 target");

    // Verify the output contains expected content
    assert!(
        output.contains("esp_hal"),
        "Output should contain 'esp_hal'"
    );
    assert!(!output.is_empty(), "Output should not be empty");
    assert!(
        output.contains("pub"),
        "Output should contain at least one 'pub' declaration"
    );

    Ok(())
}

#[test]
fn test_target_arch_configuration() -> Result<()> {
    let temp_dir = tempdir()?;
    let src_dir = temp_dir.path().join("src");
    let lib_path = src_dir.join("lib.rs");
    let cargo_toml_path = temp_dir.path().join("Cargo.toml");

    std::fs::create_dir_all(&src_dir)?;
    std::fs::write(&lib_path, "pub fn test_fn() {}")?;
    std::fs::write(
        &cargo_toml_path,
        r#"
        [package]
        name = "test_crate"
        version = "0.1.0"
        edition = "2021"
    "#,
    )?;

    let ruskel = Ruskel::new()
        .with_silent(true)
        .with_target_arch(Some("x86_64-unknown-linux-gnu".to_string()));

    let output = ruskel
        .render(
            temp_dir.path().to_str().unwrap(),
            false,
            false,
            Vec::new(),
            false,
        )
        .expect("Failed to render test_crate with x86_64-unknown-linux-gnu target");

    assert!(
        output.contains("pub fn test_fn"),
        "Output should contain 'pub fn test_fn'"
    );
    assert!(
        output.contains("test_crate"),
        "Output should contain the crate name"
    );
    assert!(!output.is_empty(), "Output should not be empty");

    Ok(())
}

#[test]
fn test_target_arch_with_common_targets() -> Result<()> {
    // Test common target architectures that should work
    let common_targets = [
        "x86_64-unknown-linux-gnu",
        "aarch64-apple-darwin",
        "wasm32-unknown-unknown",
    ];

    for target in common_targets {
        let ruskel = Ruskel::new()
            .with_silent(true)
            .with_target_arch(Some(target.to_string()));

        let output = ruskel
            .render("serde", false, false, Vec::new(), false)
            .expect(&format!("Failed to render serde with target {}", target));

        // Verify the output contains expected content
        assert!(
            output.contains("serde"),
            "Output should contain 'serde' for target {}",
            target
        );
        assert!(
            output.contains("pub"),
            "Output should contain at least one 'pub' declaration for target {}",
            target
        );
        assert!(
            !output.is_empty(),
            "Output should not be empty for target {}",
            target
        );
    }

    Ok(())
}

#[test]
fn test_target_arch_with_invalid_target() -> Result<()> {
    // Test invalid target architectures - these should fail
    let invalid_targets = ["invalid-target-triple", "not-a-target"];

    for target in invalid_targets {
        let ruskel = Ruskel::new()
            .with_silent(true)
            .with_target_arch(Some(target.to_string()));

        let result = ruskel.render("serde", false, false, Vec::new(), false);

        // Invalid targets should fail - we expect an error
        assert!(
            result.is_err(),
            "Expected error for invalid target {}, but got success",
            target
        );
    }

    Ok(())
}

#[test]
fn test_target_arch_with_features() -> Result<()> {
    // Test target arch combined with features
    let ruskel = Ruskel::new()
        .with_silent(true)
        .with_target_arch(Some("x86_64-unknown-linux-gnu".to_string()));

    let output = ruskel
        .render("serde", false, true, vec!["derive".to_string()], false)
        .expect("Failed to render serde with target arch and features");

    // Verify the output contains expected content
    assert!(output.contains("serde"), "Output should contain 'serde'");
    assert!(
        output.contains("pub"),
        "Output should contain at least one 'pub' declaration"
    );
    assert!(
        output.contains("derive") || output.contains("Derive"),
        "Output should contain derive-related content"
    );
    assert!(!output.is_empty(), "Output should not be empty");

    Ok(())
}
