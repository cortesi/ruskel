use std::fs;

use rustdoc_types::Crate;

use crate::{
    error::{Result, RuskelError},
    stdlib_mapping::STD_MODULE_MAPPING,
    toolchain::nightly_sysroot,
};

/// Check whether a crate name identifies a prebuilt standard-library document.
pub fn is_crate(name: &str) -> bool {
    matches!(name, "std" | "core" | "alloc" | "proc_macro" | "test")
}

/// Check whether a bare name is a mapped standard-library module.
pub fn is_module(name: &str) -> bool {
    mapped_crate(name).is_some()
}

/// Map a `std` re-export path to the crate that owns its rustdoc item.
pub fn resolve_reexport(target: &str) -> Option<String> {
    let after_std = target.strip_prefix("std::")?;
    let module = after_std.split("::").next()?;

    match mapped_crate(module) {
        Some("alloc") => Some(target.replacen("std::", "alloc::", 1)),
        Some("core") => Some(target.replacen("std::", "core::", 1)),
        _ => None,
    }
}

/// Load one prebuilt standard-library rustdoc document.
pub fn load_json(crate_name: &str, display_name: Option<&str>) -> Result<Crate> {
    let json_path = nightly_sysroot()?
        .join("share")
        .join("doc")
        .join("rust")
        .join("json")
        .join(format!("{crate_name}.json"));

    if !json_path.exists() {
        return Err(RuskelError::Generate(
            "Standard library documentation not available (missing rust-docs-json component)"
                .to_string(),
        ));
    }

    let json_content = fs::read_to_string(&json_path)?;
    let mut crate_data: Crate = serde_json::from_str(&json_content).map_err(|error| {
        RuskelError::Generate(format!(
            "Failed to parse standard library JSON documentation: {error}"
        ))
    })?;

    if let Some(display) = display_name
        && let Some(root_item) = crate_data.index.get_mut(&crate_data.root)
    {
        root_item.name = Some(display.to_string());
    }

    Ok(crate_data)
}

/// Return the crate that owns one mapped top-level module.
fn mapped_crate(module: &str) -> Option<&'static str> {
    STD_MODULE_MAPPING
        .binary_search_by_key(&module, |(name, _)| *name)
        .ok()
        .map(|index| STD_MODULE_MAPPING[index].1)
}

#[cfg(test)]
pub fn mapped_modules() -> &'static [(&'static str, &'static str)] {
    STD_MODULE_MAPPING
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_is_sorted_and_unique() {
        assert!(
            STD_MODULE_MAPPING
                .windows(2)
                .all(|entries| entries[0].0 < entries[1].0)
        );
    }

    #[test]
    fn maps_std_reexports_without_rewriting_std_owned_modules() {
        assert_eq!(
            resolve_reexport("std::collections::HashMap"),
            Some(String::from("alloc::collections::HashMap"))
        );
        assert_eq!(
            resolve_reexport("std::option::Option"),
            Some(String::from("core::option::Option"))
        );
        assert_eq!(resolve_reexport("std::io::Read"), None);
        assert_eq!(resolve_reexport("core::option::Option"), None);
    }
}
