//! Generate std library module mapping from rust-docs-json

use rustdoc_types::{Crate, ItemEnum, Visibility};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn get_sysroot() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let output = Command::new("rustc")
        .args(["+nightly", "--print", "sysroot"])
        .output()?;

    if !output.status.success() {
        return Err("Failed to get nightly sysroot".into());
    }

    let sysroot = String::from_utf8(output.stdout)?.trim().to_string();
    Ok(PathBuf::from(sysroot))
}

fn load_crate_json(crate_name: &str) -> Result<Crate, Box<dyn std::error::Error>> {
    let sysroot = get_sysroot()?;
    let json_path = sysroot
        .join("share/doc/rust/json")
        .join(format!("{}.json", crate_name));

    if !json_path.exists() {
        return Err(format!("JSON file not found: {:?}", json_path).into());
    }

    let json_str = fs::read_to_string(&json_path)?;
    let crate_data: Crate = serde_json::from_str(&json_str)?;
    Ok(crate_data)
}

fn find_std_reexports() -> Result<HashMap<String, String>, Box<dyn std::error::Error>> {
    // Load std crate
    let std_crate = load_crate_json("std")?;
    
    let mut mapping = HashMap::new();
    
    // Get the root module
    if let Some(root_item) = std_crate.index.get(&std_crate.root) {
        if let ItemEnum::Module(root_module) = &root_item.inner {
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
                                    if let Some(module) = use_item.source.strip_prefix("core::") {
                                        if let Some(module_name) = module.split("::").next() {
                                            if module_name == name {
                                                mapping.insert(name.clone(), "core".to_string());
                                            }
                                        }
                                    }
                                } else if use_item.source.starts_with("alloc::") {
                                    // Extract module name from path like "alloc::vec"
                                    if let Some(module) = use_item.source.strip_prefix("alloc::") {
                                        if let Some(module_name) = module.split("::").next() {
                                            if module_name == name {
                                                mapping.insert(name.clone(), "alloc".to_string());
                                            }
                                        }
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
    }
    
    // Now manually check some known patterns
    // Some modules might be re-exported as entire modules, not just use statements
    let known_core_modules = vec![
        "any", "array", "ascii", "cell", "char", "clone", "cmp", 
        "convert", "default", "error", "f32", "f64", "ffi", "fmt", 
        "future", "hash", "hint", "i8", "i16", "i32", "i64", "i128",
        "isize", "iter", "marker", "mem", "num", "ops", "option",
        "panic", "pin", "primitive", "ptr", "result", "slice", "str",
        "task", "time", "u8", "u16", "u32", "u64", "u128", "usize"
    ];
    
    let known_alloc_modules = vec![
        "alloc", "borrow", "boxed", "collections", "rc", "string", "vec"
    ];
    
    // Update mappings based on known patterns
    for module in known_core_modules {
        if mapping.get(module).map_or(true, |v| v == "std") {
            mapping.insert(module.to_string(), "core".to_string());
        }
    }
    
    for module in known_alloc_modules {
        // Only update if not already mapped to core
        if mapping.get(module).map_or(true, |v| v == "std") {
            mapping.insert(module.to_string(), "alloc".to_string());
        }
    }
    
    // Some special cases where std has its own version
    let std_specific = vec![
        "env", "fs", "io", "net", "os", "path", "process", "thread", "backtrace"
    ];
    
    // sync exists in both alloc and std, but we want to map to alloc
    // since that's where the basic sync types (Arc) come from
    mapping.insert("sync".to_string(), "alloc".to_string());
    
    for module in std_specific {
        mapping.insert(module.to_string(), "std".to_string());
    }
    
    Ok(mapping)
}

fn generate_rust_code(mapping: &HashMap<String, String>) -> String {
    let mut output = String::new();
    
    output.push_str("/// Mapping of std library modules to their actual crate location.\n");
    output.push_str("/// This provides a single source of truth for:\n");
    output.push_str("/// 1. Which modules should not be resolved as standalone crates\n");
    output.push_str("/// 2. Where std re-exports actually come from (core/alloc/std)\n");
    output.push_str("/// \n");
    output.push_str("/// Generated by scripts/generate_std_mapping\n");
    output.push_str("/// To regenerate: cd scripts/generate_std_mapping && cargo run\n");
    output.push_str("/// \n");
    output.push_str("/// Based on the Rust standard library structure where:\n");
    output.push_str("/// - `core`: fundamental types and traits, no heap allocation\n");
    output.push_str("/// - `alloc`: heap allocation support (Vec, String, etc.)\n");
    output.push_str("/// - `std`: OS abstractions and re-exports from core/alloc\n");
    output.push_str("static STD_MODULE_MAPPING: Lazy<HashMap<&'static str, &'static str>> = Lazy::new(|| {\n");
    output.push_str("    let mut map = HashMap::new();\n");
    output.push_str("    \n");
    
    // Group by crate for better organization
    let mut by_crate: HashMap<&str, Vec<(&str, &str)>> = HashMap::new();
    for (module, crate_name) in mapping {
        by_crate.entry(crate_name.as_str())
            .or_insert_with(Vec::new)
            .push((module.as_str(), crate_name.as_str()));
    }
    
    // Sort each group
    for (crate_name, modules) in &mut by_crate {
        modules.sort_by_key(|&(module, _)| module);
        
        output.push_str(&format!("    // Modules from {}\n", crate_name));
        for (module, _) in modules {
            output.push_str(&format!("    map.insert(\"{}\", \"{}\");\n", module, crate_name));
        }
        output.push_str("    \n");
    }
    
    output.push_str("    map\n");
    output.push_str("});\n");
    
    output
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Analyzing standard library structure...");
    
    let mapping = find_std_reexports()?;
    
    println!("Found {} modules:", mapping.len());
    let mut sorted: Vec<_> = mapping.iter().collect();
    sorted.sort_by_key(|&(k, _)| k);
    
    for (module, crate_name) in &sorted {
        println!("  {} -> {}", module, crate_name);
    }
    
    println!("\nGenerating Rust code...");
    let rust_code = generate_rust_code(&mapping);
    
    // Write to stdout so it can be copied or redirected
    println!("\n--- Generated Code ---\n");
    println!("{}", rust_code);
    
    Ok(())
}