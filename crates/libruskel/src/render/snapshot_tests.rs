use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use rustdoc_types::{Crate, ItemEnum, MacroKind, ProcMacro};
use tempfile::TempDir;

use super::{SNAPSHOT_RUSTFMT_V1, snapshot_rustfmt_command};
use crate::{
    Renderer, Result,
    cache::CacheHandle,
    rustdoc_build::{self, CrateReadOptions},
    target_resolution::resolve_target,
};

const SNAPSHOT_TOOLCHAIN: &str = "nightly-2026-07-01";

/// Isolated fixture root and nested package path.
struct Fixture {
    /// Temporary parent that owns the complete fixture.
    _root: TempDir,
    /// Package directory below the hostile parent configuration.
    package: PathBuf,
}

/// Create a locked fixture with a broad public Rust surface.
fn fixture() -> Result<Fixture> {
    let root = tempfile::tempdir()?;
    let package = root.path().join("project");
    fs::create_dir(&package)?;
    fs::write(
        package.join("rustfmt.toml"),
        "hard_tabs = true\nfn_single_line = true\n",
    )?;
    fs::write(root.path().join("rustfmt.toml"), "tab_spaces = 7\n")?;
    fs::create_dir_all(package.join("src"))?;
    fs::write(
        package.join("Cargo.toml"),
        r#"[package]
name = "snapshot-render-fixture"
version = "0.1.0"
edition = "2024"

[lib]
name = "renamed_snapshot_lib"
"#,
    )?;
    fs::write(
        package.join("src/lib.rs"),
        r#"#![feature(trait_alias)]

/// Crate API documentation.
pub mod zed {
    /// Last declaration in source.
    pub fn zed() {}
}

/// An ordered data type.
#[repr(C)]
#[doc(hidden)]
#[derive(Clone)]
pub struct Alpha {
    /// First field.
    pub first: u8,
    /// Second field.
    pub second: u16,
}

impl std::fmt::Display for Alpha {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.first)
    }
}

pub union Choice {
    pub integer: u32,
    pub float: f32,
}

pub enum Ordered {
    First(u8),
    Second { value: u16 },
}

#[repr(u8)]
pub enum Discriminated {
    One = 10,
    Two = 20,
}

pub trait Surface {
    type Item: Clone + Send;
    fn zed(&self);
    fn alpha(&self);
}

pub trait Alias = Sync + Send;

#[unsafe(no_mangle)]
pub static EXPORTED: u8 = 7;

/// First documentation line.
/// Second documentation line.
pub fn ordered_parameters(first: u8, second: u16) {}

pub fn constrained<T>(value: T)
where
    T: Send,
    T: Clone,
{
}

fn private_only() {}

#[macro_export]
macro_rules! exported_macro {
    () => {};
}

mod private_support {
    pub struct Internal;

    impl Internal {
        pub fn public_method(&self) {}
    }
}

pub use private_support::Internal as Renamed;
"#,
    )?;
    let status = Command::new("cargo")
        .arg("generate-lockfile")
        .arg("--manifest-path")
        .arg(package.join("Cargo.toml"))
        .status()?;
    assert!(status.success(), "fixture lockfile generation failed");
    Ok(Fixture {
        _root: root,
        package,
    })
}

/// Build rustdoc JSON for the fixture through the ordinary inspection path.
fn inspect_fixture(root: &Path) -> Result<Crate> {
    let resolved = resolve_target(root.to_str().expect("UTF-8 fixture path"), true)?;
    Ok(rustdoc_build::build(
        &resolved,
        &CrateReadOptions {
            no_default_features: false,
            all_features: false,
            features: Vec::new(),
            private_items: true,
            hidden_items: true,
            silent: true,
            offline: true,
            bin_override: None,
            toolchain: SNAPSHOT_TOOLCHAIN.to_string(),
            target: None,
            locked: true,
            cache: CacheHandle::new(Some(root.join("cache"))),
        },
    )?
    .crate_data)
}

/// Render with the strict format 1 policy.
fn snapshot(crate_data: &Crate) -> Result<String> {
    Renderer::snapshot_v1(SNAPSHOT_TOOLCHAIN).render(crate_data)
}

#[test]
fn snapshot_rustfmt_command_and_configuration_are_exact() {
    let command = snapshot_rustfmt_command(
        Path::new("/toolchain/bin/rustfmt"),
        Path::new("/empty/snapshot-rustfmt-v1.toml"),
        Path::new("/empty"),
    );
    let arguments: Vec<_> = command
        .get_args()
        .map(|argument| argument.to_string_lossy())
        .collect();
    assert_eq!(
        arguments,
        [
            "--edition",
            "2024",
            "--style-edition",
            "2024",
            "--config-path",
            "/empty/snapshot-rustfmt-v1.toml",
        ]
    );
    assert_eq!(command.get_current_dir(), Some(Path::new("/empty")));
    assert_eq!(
        SNAPSHOT_RUSTFMT_V1,
        b"brace_style = \"PreferSameLine\"\nnewline_style = \"Unix\"\n"
    );
}

#[test]
fn snapshot_is_stable_across_unordered_rustdoc_sequences() -> Result<()> {
    let root = fixture()?;
    let original = inspect_fixture(&root.package)?;
    let expected = snapshot(&original)?;
    let mut permuted = original.clone();

    let mut values: Vec<_> = permuted.index.drain().collect();
    values.reverse();
    permuted.index = values.into_iter().collect::<HashMap<_, _>>();
    for item in permuted.index.values_mut() {
        match &mut item.inner {
            ItemEnum::Module(module) => module.items.reverse(),
            ItemEnum::Struct(struct_) => struct_.impls.reverse(),
            ItemEnum::Union(union_) => union_.impls.reverse(),
            ItemEnum::Enum(enum_) => enum_.impls.reverse(),
            ItemEnum::Trait(trait_) => {
                trait_.items.reverse();
                trait_.bounds.reverse();
                trait_.generics.where_predicates.reverse();
            }
            ItemEnum::TraitAlias(alias) => alias.params.reverse(),
            ItemEnum::Function(function) => function.generics.where_predicates.reverse(),
            ItemEnum::Impl(impl_) => {
                impl_.items.reverse();
                impl_.generics.where_predicates.reverse();
            }
            _ => {}
        }
    }

    assert_eq!(snapshot(&permuted)?, expected);
    assert!(
        expected.contains("#[doc(hidden)]"),
        "snapshot omitted doc(hidden):\n{expected}"
    );
    assert!(expected.contains("impl Clone for Alpha"));
    assert!(expected.contains("impl Renamed"));
    assert!(expected.contains("pub union Choice"));
    assert!(expected.contains("pub trait Alias"));
    assert!(expected.contains("pub static EXPORTED"));
    assert!(expected.contains("#[unsafe(no_mangle)]"));
    assert!(!expected.contains('\t'), "hostile rustfmt config leaked in");

    let mut private_only = original;
    let item = private_only
        .index
        .values_mut()
        .find(|item| item.name.as_deref() == Some("private_only"))
        .expect("private-only fixture");
    item.docs = Some("A private-only change.".to_string());
    assert_eq!(snapshot(&private_only)?, expected);
    Ok(())
}

#[test]
fn snapshot_preserves_ordered_api_sequences() -> Result<()> {
    let root = fixture()?;
    let original = inspect_fixture(&root.package)?;
    let expected = snapshot(&original)?;

    let mut parameters = original.clone();
    let function = parameters
        .index
        .values_mut()
        .find(|item| item.name.as_deref() == Some("ordered_parameters"))
        .expect("ordered function");
    let ItemEnum::Function(function) = &mut function.inner else {
        panic!("ordered_parameters must be a function");
    };
    function.sig.inputs.reverse();
    assert_ne!(snapshot(&parameters)?, expected);

    let mut variants = original.clone();
    let ordered = variants
        .index
        .values_mut()
        .find(|item| item.name.as_deref() == Some("Ordered"))
        .expect("ordered enum");
    let ItemEnum::Enum(ordered) = &mut ordered.inner else {
        panic!("Ordered must be an enum");
    };
    ordered.variants.reverse();
    assert_ne!(snapshot(&variants)?, expected);

    let mut field_order = original.clone();
    let alpha = field_order
        .index
        .values_mut()
        .find(|item| item.name.as_deref() == Some("Alpha"))
        .expect("ordered struct");
    let ItemEnum::Struct(alpha) = &mut alpha.inner else {
        panic!("Alpha must be a struct");
    };
    let rustdoc_types::StructKind::Plain { fields, .. } = &mut alpha.kind else {
        panic!("Alpha must have named fields");
    };
    fields.reverse();
    assert_ne!(snapshot(&field_order)?, expected);

    let mut attributes = original.clone();
    let alpha = attributes
        .index
        .values_mut()
        .find(|item| item.name.as_deref() == Some("Alpha"))
        .expect("attributed struct");
    alpha.attrs.reverse();
    assert_ne!(snapshot(&attributes)?, expected);

    let mut attribute_arguments = original.clone();
    let alpha_id = attribute_arguments
        .index
        .values()
        .find(|item| item.name.as_deref() == Some("Alpha"))
        .expect("attribute argument fixture")
        .id;
    attribute_arguments
        .index
        .get_mut(&alpha_id)
        .expect("attribute argument fixture")
        .attrs
        .push(rustdoc_types::Attribute::Other(
            "#[cfg(any(unix, windows))]".to_string(),
        ));
    let first = snapshot(&attribute_arguments)?;
    let Some(rustdoc_types::Attribute::Other(source)) = attribute_arguments
        .index
        .get_mut(&alpha_id)
        .expect("attribute argument fixture")
        .attrs
        .last_mut()
    else {
        panic!("synthetic attribute must be retained");
    };
    *source = "#[cfg(any(windows, unix))]".to_string();
    assert_ne!(snapshot(&attribute_arguments)?, first);

    let mut discriminants = original.clone();
    let variant = discriminants
        .index
        .values_mut()
        .find(|item| item.name.as_deref() == Some("One"))
        .expect("discriminant fixture");
    let ItemEnum::Variant(variant) = &mut variant.inner else {
        panic!("One must be a variant");
    };
    variant
        .discriminant
        .as_mut()
        .expect("explicit discriminant")
        .expr = "11".to_string();
    assert_ne!(snapshot(&discriminants)?, expected);

    let mut documentation = original;
    let function = documentation
        .index
        .values_mut()
        .find(|item| item.name.as_deref() == Some("ordered_parameters"))
        .expect("documented function");
    function.docs = Some("Second documentation line.\nFirst documentation line.".to_string());
    assert_ne!(snapshot(&documentation)?, expected);
    Ok(())
}

#[test]
fn snapshot_renders_proc_macros_and_rejects_unsupported_public_items() -> Result<()> {
    let root = fixture()?;
    let original = inspect_fixture(&root.package)?;

    let mut proc_macro = original.clone();
    let item = proc_macro
        .index
        .values_mut()
        .find(|item| item.name.as_deref() == Some("ordered_parameters"))
        .expect("function used as proc-macro fixture");
    item.name = Some("derive_api".to_string());
    item.docs = None;
    item.attrs.clear();
    item.inner = ItemEnum::ProcMacro(ProcMacro {
        kind: MacroKind::Derive,
        helpers: vec!["helper".to_string()],
    });
    let rendered = snapshot(&proc_macro)?;
    assert!(rendered.contains("#[proc_macro_derive(derive_api, attributes(helper))]"));

    let mut unsupported = original.clone();
    unsupported
        .index
        .values_mut()
        .find(|item| item.name.as_deref() == Some("ordered_parameters"))
        .expect("public unsupported fixture")
        .inner = ItemEnum::ExternType;
    let error = snapshot(&unsupported).expect_err("reachable extern type must fail");
    assert!(error.to_string().contains("does not support reachable"));

    let mut unresolved = original;
    let public_use = unresolved
        .index
        .values_mut()
        .find(|item| matches!(item.inner, ItemEnum::Use(_)))
        .expect("public re-export fixture");
    let ItemEnum::Use(import) = &mut public_use.inner else {
        panic!("fixture item must be a use");
    };
    import.id = None;
    import.is_glob = true;
    let error = snapshot(&unresolved).expect_err("unresolved public glob must fail");
    assert!(error.to_string().contains("cannot resolve public export"));
    Ok(())
}
