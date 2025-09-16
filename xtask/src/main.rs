//! Build-time code generation and project maintenance tasks

use std::{collections::HashMap, error::Error, fs, path::PathBuf, process::Command};

use clap::{Parser, Subcommand};
use rustdoc_types::{Crate, ItemEnum, Visibility};

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
    /// Generate the standard library module mapping
    GenStdMapping {
        /// Write the output to the source file instead of stdout
        #[arg(short, long)]
        write: bool,
    },
}

/// Run the CLI and dispatch to the selected subcommand.
fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::GenStdMapping { write } => generate_std_mapping(write),
    }
}

/// Locate the nightly toolchain sysroot used for documentation JSON artifacts.
fn get_sysroot() -> Result<PathBuf, Box<dyn Error>> {
    let output = Command::new("rustc")
        .args(["+nightly", "--print", "sysroot"])
        .output()?;

    if !output.status.success() {
        return Err("Failed to get nightly sysroot - ensure nightly toolchain is installed".into());
    }

    let sysroot = String::from_utf8(output.stdout)?.trim().to_string();
    Ok(PathBuf::from(sysroot))
}

/// Load the rustdoc JSON metadata for the provided crate name.
fn load_crate_json(crate_name: &str) -> Result<Crate, Box<dyn Error>> {
    let sysroot = get_sysroot()?;
    let json_path = sysroot
        .join("share/doc/rust/json")
        .join(format!("{}.json", crate_name));

    if !json_path.exists() {
        return Err(format!(
            "JSON file not found: {:?}\nEnsure rust-docs-json component is installed: rustup component add --toolchain nightly rust-docs-json",
            json_path
        ).into());
    }

    let json_str = fs::read_to_string(&json_path)?;
    let crate_data: Crate = serde_json::from_str(&json_str)?;
    Ok(crate_data)
}

/// Map top-level `std` modules to the crate that actually provides them.
fn find_std_reexports() -> Result<HashMap<String, String>, Box<dyn Error>> {
    // Load std crate
    let std_crate = load_crate_json("std")?;

    let mut mapping = HashMap::new();

    // Get the root module
    if let Some(root_item) = std_crate.index.get(&std_crate.root)
        && let ItemEnum::Module(root_module) = &root_item.inner
    {
        // Iterate through top-level items in std
        for item_id in &root_module.items {
            if let Some(item) = std_crate.index.get(item_id) {
                // Only consider public items
                if !matches!(item.visibility, Visibility::Public) {
                    continue;
                }

                if let Some(name) = &item.name {
                    match &item.inner {
                        ItemEnum::Use(use_item) => {
                            // This is a re-export - analyze where it comes from
                            if use_item.source.starts_with("core::") {
                                // Extract module name from path like "core::mem"
                                if let Some(module) = use_item.source.strip_prefix("core::")
                                    && let Some(module_name) = module.split("::").next()
                                    && module_name == name
                                {
                                    mapping.insert(name.clone(), "core".to_string());
                                }
                            } else if use_item.source.starts_with("alloc::") {
                                // Extract module name from path like "alloc::vec"
                                if let Some(module) = use_item.source.strip_prefix("alloc::")
                                    && let Some(module_name) = module.split("::").next()
                                    && module_name == name
                                {
                                    mapping.insert(name.clone(), "alloc".to_string());
                                }
                            }
                        }
                        ItemEnum::Module(_) => {
                            // For modules that are not re-exports, they're std-specific
                            // But we need to check if this is actually a re-export at the module level
                            // For now, we'll mark them as std and manually verify later
                            if !mapping.contains_key(name) {
                                mapping.insert(name.clone(), "std".to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // Now manually check some known patterns
    // Some modules might be re-exported as entire modules, not just use statements
    let known_core_modules = vec![
        "any",
        "array",
        "ascii",
        "cell",
        "char",
        "clone",
        "cmp",
        "convert",
        "default",
        "error",
        "f32",
        "f64",
        "ffi",
        "fmt",
        "future",
        "hash",
        "hint",
        "i8",
        "i16",
        "i32",
        "i64",
        "i128",
        "isize",
        "iter",
        "marker",
        "mem",
        "num",
        "ops",
        "option",
        "panic",
        "pin",
        "primitive",
        "ptr",
        "result",
        "slice",
        "str",
        "task",
        "time",
        "u8",
        "u16",
        "u32",
        "u64",
        "u128",
        "usize",
    ];

    let known_alloc_modules = vec![
        "alloc",
        "borrow",
        "boxed",
        "collections",
        "rc",
        "string",
        "vec",
    ];

    // Update mappings based on known patterns
    for module in known_core_modules {
        if mapping.get(module).is_none_or(|v| v == "std") {
            mapping.insert(module.to_string(), "core".to_string());
        }
    }

    for module in known_alloc_modules {
        // Only update if not already mapped to core
        if mapping.get(module).is_none_or(|v| v == "std") {
            mapping.insert(module.to_string(), "alloc".to_string());
        }
    }

    // Some special cases where std has its own version
    let std_specific = vec![
        "env",
        "fs",
        "io",
        "net",
        "os",
        "path",
        "process",
        "thread",
        "backtrace",
    ];

    // sync exists in both alloc and std, but we want to map to alloc
    // since that's where the basic sync types (Arc) come from
    mapping.insert("sync".to_string(), "alloc".to_string());

    for module in std_specific {
        mapping.insert(module.to_string(), "std".to_string());
    }

    Ok(mapping)
}

/// Render the module mapping into Rust source code.
fn generate_rust_code(mapping: &HashMap<String, String>) -> String {
    let mut output = String::new();

    output.push_str("/// Mapping of std library modules to their actual crate location.\n");
    output.push_str("/// This provides a single source of truth for:\n");
    output.push_str("/// 1. Which modules should not be resolved as standalone crates\n");
    output.push_str("/// 2. Where std re-exports actually come from (core/alloc/std)\n");
    output.push_str("///\n");
    output.push_str("/// Generated by `cargo xtask gen-std-mapping`\n");
    output.push_str("/// To regenerate: `cargo xtask gen-std-mapping --write`\n");
    output.push_str("///\n");
    output.push_str("/// Based on the Rust standard library structure where:\n");
    output.push_str("/// - `core`: fundamental types and traits, no heap allocation\n");
    output.push_str("/// - `alloc`: heap allocation support (Vec, String, etc.)\n");
    output.push_str("/// - `std`: OS abstractions and re-exports from core/alloc\n");
    output.push_str(
        "static STD_MODULE_MAPPING: Lazy<HashMap<&'static str, &'static str>> = Lazy::new(|| {\n",
    );
    output.push_str("    let mut map = HashMap::new();\n");
    output.push('\n');

    // Group by crate for better organization
    let mut by_crate: HashMap<&str, Vec<(&str, &str)>> = HashMap::new();
    for (module, crate_name) in mapping {
        by_crate
            .entry(crate_name.as_str())
            .or_default()
            .push((module.as_str(), crate_name.as_str()));
    }

    // Sort each group
    for (crate_name, modules) in &mut by_crate {
        modules.sort_by_key(|&(module, _)| module);

        output.push_str(&format!("    // Modules from {}\n", crate_name));
        for (module, _) in modules {
            output.push_str(&format!(
                "    map.insert(\"{}\", \"{}\");\n",
                module, crate_name
            ));
        }
        output.push('\n');
    }

    output.push_str("    map\n");
    output.push_str("});\n");

    output
}

/// Build the std module mapping and optionally write it to the repository.
fn generate_std_mapping(write: bool) -> Result<(), Box<dyn Error>> {
    eprintln!("Analyzing standard library structure...");

    let mapping = find_std_reexports()?;

    eprintln!("Found {} modules", mapping.len());

    let rust_code = generate_rust_code(&mapping);

    if write {
        // Find the target file
        let target_path = PathBuf::from("crates/libruskel/src/cargoutils.rs");
        if !target_path.exists() {
            return Err(format!("Target file not found: {:?}", target_path).into());
        }

        // Read the current file
        let current_content = fs::read_to_string(&target_path)?;

        // Find the start and end markers
        let start_marker =
            "static STD_MODULE_MAPPING: Lazy<HashMap<&'static str, &'static str>> = Lazy::new(|| {";
        let end_marker = "});\n";

        let start_pos = current_content
            .find(start_marker)
            .ok_or("Could not find STD_MODULE_MAPPING start marker")?;

        // Find the end position after the start
        let search_from = start_pos + start_marker.len();
        let relative_end = current_content[search_from..]
            .find(end_marker)
            .ok_or("Could not find STD_MODULE_MAPPING end marker")?;
        let end_pos = search_from + relative_end + end_marker.len();

        // Find the documentation comment start
        let doc_start = current_content[..start_pos]
            .rfind("/// Mapping of std library modules")
            .ok_or("Could not find documentation comment")?;

        // Replace the content
        let new_content = format!(
            "{}{}{}",
            &current_content[..doc_start],
            rust_code,
            &current_content[end_pos..]
        );

        fs::write(&target_path, new_content)?;
        eprintln!("Updated {}", target_path.display());
    } else {
        println!("{}", rust_code);
    }

    Ok(())
}
