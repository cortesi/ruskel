//! Build-time code generation and project maintenance tasks.

use std::{
    collections::BTreeMap,
    error::Error,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use clap::{Parser, Subcommand};
use libruskel::toolchain::nightly_sysroot;
use rustdoc_types::{Crate, ItemEnum, Visibility};
use tempfile::NamedTempFile;

/// Corrections for modules whose ownership is not represented reliably by
/// top-level rustdoc uses.
const STD_MODULE_OVERRIDES: &[(&str, &str)] = &[
    ("alloc", "alloc"),
    ("any", "core"),
    ("array", "core"),
    ("ascii", "core"),
    ("backtrace", "std"),
    ("borrow", "alloc"),
    ("boxed", "alloc"),
    ("cell", "core"),
    ("char", "core"),
    ("clone", "core"),
    ("cmp", "core"),
    ("collections", "alloc"),
    ("convert", "core"),
    ("default", "core"),
    ("env", "std"),
    ("error", "core"),
    ("f32", "core"),
    ("f64", "core"),
    ("ffi", "core"),
    ("fmt", "core"),
    ("fs", "std"),
    ("future", "core"),
    ("hash", "core"),
    ("hint", "core"),
    ("i128", "core"),
    ("i16", "core"),
    ("i32", "core"),
    ("i64", "core"),
    ("i8", "core"),
    ("io", "std"),
    ("isize", "core"),
    ("iter", "core"),
    ("marker", "core"),
    ("mem", "core"),
    ("net", "std"),
    ("num", "core"),
    ("ops", "core"),
    ("option", "core"),
    ("os", "std"),
    ("panic", "core"),
    ("path", "std"),
    ("pin", "core"),
    ("primitive", "core"),
    ("process", "std"),
    ("ptr", "core"),
    ("rc", "alloc"),
    ("result", "core"),
    ("slice", "core"),
    ("str", "core"),
    ("string", "alloc"),
    ("sync", "alloc"),
    ("task", "core"),
    ("thread", "std"),
    ("time", "core"),
    ("u128", "core"),
    ("u16", "core"),
    ("u32", "core"),
    ("u64", "core"),
    ("u8", "core"),
    ("usize", "core"),
    ("vec", "alloc"),
];

#[derive(Parser)]
#[command(name = "xtask")]
#[command(about = "Development tasks for ruskel")]
/// Command-line interface entry point for the `xtask` binary.
struct Cli {
    /// Subcommand dispatched by the CLI.
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
/// Supported automation commands.
enum Commands {
    /// Generate the standard-library module mapping.
    GenStdMapping {
        /// Write the generated artifact to the repository.
        #[arg(short, long, conflicts_with = "check")]
        write: bool,
        /// Fail when the checked-in artifact differs from generated output.
        #[arg(long, conflicts_with = "write")]
        check: bool,
    },
}

/// Requested handling for generated output.
#[derive(Clone, Copy)]
enum OutputMode {
    /// Print generated output.
    Print,
    /// Atomically update the checked-in artifact.
    Write,
    /// Compare generated output with the checked-in artifact.
    Check,
}

/// Run the CLI and dispatch to the selected subcommand.
fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::GenStdMapping { write, check } => {
            let mode = if write {
                OutputMode::Write
            } else if check {
                OutputMode::Check
            } else {
                OutputMode::Print
            };
            generate_std_mapping(mode)
        }
    }
}

/// Load rustdoc JSON metadata for one standard-library crate.
fn load_crate_json(crate_name: &str) -> Result<Crate, Box<dyn Error>> {
    let json_path = nightly_sysroot()?
        .join("share/doc/rust/json")
        .join(format!("{crate_name}.json"));

    if !json_path.exists() {
        return Err(format!(
            "JSON file not found: '{}'\nEnsure rust-docs-json is installed: rustup component add --toolchain nightly rust-docs-json",
            json_path.display()
        )
        .into());
    }

    Ok(serde_json::from_str(&fs::read_to_string(json_path)?)?)
}

/// Discover top-level `std` modules and the crate that defines each re-export.
fn find_std_reexports() -> Result<BTreeMap<String, String>, Box<dyn Error>> {
    let std_crate = load_crate_json("std")?;
    let mut mapping = BTreeMap::new();

    if let Some(root_item) = std_crate.index.get(&std_crate.root)
        && let ItemEnum::Module(root_module) = &root_item.inner
    {
        for item_id in &root_module.items {
            let Some(item) = std_crate.index.get(item_id) else {
                continue;
            };
            if !matches!(item.visibility, Visibility::Public) {
                continue;
            }
            let Some(name) = &item.name else {
                continue;
            };

            match &item.inner {
                ItemEnum::Use(use_item) => {
                    for crate_name in ["core", "alloc"] {
                        if let Some(path) = use_item.source.strip_prefix(crate_name)
                            && let Some(path) = path.strip_prefix("::")
                            && path.split("::").next() == Some(name.as_str())
                        {
                            mapping.insert(name.clone(), String::from(crate_name));
                        }
                    }
                }
                ItemEnum::Module(_) => {
                    mapping
                        .entry(name.clone())
                        .or_insert_with(|| String::from("std"));
                }
                _ => {}
            }
        }
    }

    for &(module, crate_name) in STD_MODULE_OVERRIDES {
        mapping.insert(String::from(module), String::from(crate_name));
    }

    Ok(mapping)
}

/// Render a sorted standard-library mapping artifact.
fn render_std_mapping(mapping: &BTreeMap<String, String>) -> String {
    let mut output = String::from(
        "// @generated by `cargo xtask gen-std-mapping --write`.\n\
         // Check with `cargo xtask gen-std-mapping --check`.\n\n\
         /// Sorted top-level module names and their rustdoc-owning crates.\n\
         pub const STD_MODULE_MAPPING: &[(&str, &str)] = &[\n",
    );

    for (module, crate_name) in mapping {
        output.push_str(&format!("    (\"{module}\", \"{crate_name}\"),\n"));
    }
    output.push_str("];\n");
    output
}

/// Return the repository path for the generated mapping artifact.
fn mapping_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask must be inside the repository")
        .join("crates/libruskel/src/stdlib_mapping.rs")
}

/// Atomically replace a generated artifact.
fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), Box<dyn Error>> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("generated path '{}' has no parent", path.display()))?;
    let mut temporary = NamedTempFile::new_in(parent)?;
    temporary.write_all(contents)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path)?;
    Ok(())
}

/// Generate the mapping once, then print, write, or verify the same bytes.
fn generate_std_mapping(mode: OutputMode) -> Result<(), Box<dyn Error>> {
    eprintln!("Analyzing standard library structure...");
    let mapping = find_std_reexports()?;
    let generated = render_std_mapping(&mapping);
    let target = mapping_path();
    eprintln!("Found {} modules", mapping.len());

    match mode {
        OutputMode::Print => print!("{generated}"),
        OutputMode::Write => {
            write_atomic(&target, generated.as_bytes())?;
            eprintln!("Updated {}", target.display());
        }
        OutputMode::Check => {
            let checked_in = fs::read(&target)?;
            if checked_in != generated.as_bytes() {
                return Err(format!(
                    "{} is stale; run `cargo xtask gen-std-mapping --write`",
                    target.display()
                )
                .into());
            }
            eprintln!("{} is current", target.display());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renderer_is_sorted_and_repeatable() {
        let mapping = BTreeMap::from([
            (String::from("vec"), String::from("alloc")),
            (String::from("any"), String::from("core")),
        ]);

        let first = render_std_mapping(&mapping);
        let second = render_std_mapping(&mapping);

        assert_eq!(first, second);
        assert!(first.find("(\"any\"").unwrap() < first.find("(\"vec\"").unwrap());
    }
}
