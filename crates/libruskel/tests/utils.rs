// Dead code detection breaks here, because the integration test crates all use a disjoint set of
// the pub items.
#![allow(dead_code)]

use libruskel::{Renderer, Ruskel};
use pretty_assertions::assert_eq;
use rust_format::{Formatter, RustFmt};
use rustdoc_types::Crate;
use std::fs;
use tempfile::TempDir;

fn normalize_whitespace(s: &str) -> String {
    let lines: Vec<&str> = s
        .lines()
        .map(|line| line.trim_end()) // Remove trailing whitespace
        .filter(|line| !line.is_empty()) // Remove blank lines
        .collect();

    if lines.is_empty() {
        return String::new();
    }

    // Find the minimum indentation
    let min_indent = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);

    // Dedent all lines by the minimum indentation
    lines
        .into_iter()
        .map(|line| {
            if line.len() > min_indent {
                &line[min_indent..]
            } else {
                line.trim_start()
            }
        })
        .collect::<Vec<&str>>()
        .join("\n")
}

fn strip_module_declaration(s: &str) -> String {
    let lines: Vec<&str> = s
        .lines()
        .map(|line| line.trim_end())
        .filter(|line| !line.is_empty())
        .collect();

    if lines.len() <= 2 {
        return String::new();
    }

    lines[1..lines.len() - 1].join("\n")
}

pub fn inspect_crate(source: &str, private_items: bool, is_proc_macro: bool) -> Crate {
    let temp_dir = TempDir::new().unwrap();
    let crate_path = temp_dir.path().join("src");
    fs::create_dir(&crate_path).unwrap();
    let lib_rs_path = crate_path.join("lib.rs");
    fs::write(&lib_rs_path, source).unwrap();

    let cargo_toml_content = if is_proc_macro {
        r#"
                [package]
                name = "dummy_crate"
                version = "0.1.0"
                edition = "2021"

                [lib]
                proc-macro = true
            "#
    } else {
        r#"
                [package]
                name = "dummy_crate"
                version = "0.1.0"
                edition = "2021"
            "#
    };
    fs::write(temp_dir.path().join("Cargo.toml"), cargo_toml_content).unwrap();

    let ruskel = Ruskel::new().with_offline(true).with_silent(true);
    ruskel
        .inspect(
            temp_dir.path().to_str().unwrap(),
            false,
            false,
            Vec::new(),
            private_items,
        )
        .unwrap()
}

pub fn render(renderer: Renderer, source: &str, expected_output: &str, is_proc_macro: bool) {
    let crate_data = inspect_crate(source, true, is_proc_macro);

    // Render the crate data
    let normalized_rendered = normalize_whitespace(&strip_module_declaration(
        &renderer.render(&crate_data).unwrap(),
    ));

    let normalized_expected = normalize_whitespace(expected_output);

    let formatter = RustFmt::default();
    assert_eq!(
        formatter.format_str(normalized_rendered).unwrap(),
        formatter.format_str(normalized_expected).unwrap(),
    );
}

/// Idempotent rendering test
pub fn rt_idemp(source: &str) {
    render(Renderer::default(), source, source, false);
}

/// Idempotent rendering test with private items
pub fn rt_priv_idemp(source: &str) {
    render(
        Renderer::default().with_private_items(true),
        source,
        source,
        false,
    );
}

/// Render roundtrip
pub fn rt(source: &str, expected_output: &str) {
    render(Renderer::default(), source, expected_output, false);
}

/// Render roundtrip with private items
pub fn rt_private(source: &str, expected_output: &str) {
    render(
        Renderer::default().with_private_items(true),
        source,
        expected_output,
        false,
    );
}

pub fn rt_procmacro(source: &str, expected_output: &str) {
    render(Renderer::default(), source, expected_output, true);
}

pub fn render_err(renderer: Renderer, source: &str, expected_error: &str) {
    let crate_data = inspect_crate(source, true, false);

    // Render the crate data
    let result = renderer.render(&crate_data);

    assert!(
        result.is_err(),
        "Expected an error, but rendering succeeded"
    );
    let error = result.unwrap_err();
    let error_string = error.to_string();

    assert_eq!(
        error_string, expected_error,
        "Error mismatch.\nExpected: {}\nGot: {}",
        expected_error, error_string
    );
}

#[macro_export]
macro_rules! gen_tests {
    ($prefix:ident, {
        $(idemp {
            $idemp_name:ident: $input:expr
        })*
        $(rt {
            $rt_name:ident: {
                input: $rt_input:expr,
                output: $rt_output:expr
            }
        })*
        $(rt_custom {
            $rt_custom_name:ident: {
                renderer: $rt_custom_renderer:expr,
                input: $rt_custom_input:expr,
                output: $rt_custom_output:expr
            }
        })*
        $(rt_err {
            $rt_err_name:ident: {
                renderer: $rt_err_renderer:expr,
                input: $rt_err_input:expr,
                error: $rt_err_error:expr
            }
        })*
    }) => {
        mod $prefix {
            use super::*;

            $(
                #[test]
                fn $idemp_name() {
                    rt_priv_idemp($input);
                }
            )*

            $(
                #[test]
                fn $rt_name() {
                    rt($rt_input, $rt_output);
                }
            )*

            $(
                #[test]
                fn $rt_custom_name() {
                    let custom_renderer = $rt_custom_renderer;
                    render(custom_renderer, $rt_custom_input, $rt_custom_output, false);
                }
            )*

            $(
                #[test]
                fn $rt_err_name() {
                    let custom_renderer = $rt_err_renderer;
                    render_err(custom_renderer, $rt_err_input, $rt_err_error);
                }
            )*
        }
    };
}
