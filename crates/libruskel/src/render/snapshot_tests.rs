use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

use rustdoc_types::{Crate, ItemEnum, MacroKind, ProcMacro, Type};
use tempfile::TempDir;

use super::{SNAPSHOT_RUSTFMT_V1, snapshot_rustfmt_command};
use crate::{
    Renderer, Result,
    cache::CacheHandle,
    cargo_env,
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
///
/// More detail.
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

impl Alpha {
    pub const VERSION: u8 = 1;
}

#[derive(Clone, Copy)]
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
    const KIND: u8;
    type Item: Clone + Send;
    type Container<T>
    where
        T: Clone;
    fn zed(&self);
    fn alpha(&self);
}

impl Surface for Alpha {
    const KIND: u8 = 1;
    type Item = u8;
    type Container<T> = Vec<T>
    where
        T: Clone;

    fn zed(&self) {}
    fn alpha(&self) {}
}

pub trait Alias = Sync + Send;

pub type SingletonTuple = (u32,);
pub type CCallback = unsafe extern "C" fn(value: i32) -> i32;
pub type HrtbCallback = for<'a> fn(value: &'a i32) -> &'a i32;

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
    let status = cargo_env::command("cargo")
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

/// Build a locked crate from one source string for focused catalogue checks.
fn fixture_from_source(source: &str) -> Result<Fixture> {
    let root = tempfile::tempdir()?;
    let package = root.path().join("project");
    fs::create_dir_all(package.join("src"))?;
    fs::write(
        package.join("Cargo.toml"),
        "[package]\nname = \"stage2-render-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )?;
    fs::write(package.join("src/lib.rs"), source)?;
    let status = cargo_env::command("cargo")
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

/// Public paths, impl ownership, and boundary markers in one source crate.
const STAGE2_SOURCE: &str = r#"
pub trait Marker {}

impl<T: Clone> Marker for T {}

pub struct LocalError;

impl From<LocalError> for std::io::Error {
    fn from(_: LocalError) -> Self {
        std::io::Error::other("local")
    }
}

pub struct Dual;

impl Dual {
    pub fn first(&self) {}
}

impl Dual {
    pub fn second(&self) {}
}

pub mod first {
    pub struct Shared;

    impl Shared {
        pub fn shared(&self) {}
    }
}

pub mod second {
    pub use crate::first::Shared;
}

pub use first::Shared;
pub use first::Shared as Alias;
pub use second::*;

mod private_home {
    pub struct Internal;

    impl Internal {
        pub fn ping(&self) {}
    }

    pub trait Sealed {}
}

pub use private_home::Internal;

pub trait Exposed: private_home::Sealed {}

pub struct ThreadBound {
    pub value: std::rc::Rc<()>,
}

/// A type with one unwind marker.
pub struct UnwindOnly<'a> {
    pub value: &'a mut u8,
}

pub trait Send {}

pub struct Named;

impl Send for Named {}

pub struct Partial {
    pub visible: u8,
    hidden: u8,
}

pub trait StaticOnly {
    fn make() -> Self;
}

pub fn free_function() {}
"#;

/// Share the compiled rustdoc fixture across focused tests in this process.
fn stage2_crate() -> &'static Crate {
    static FIXTURE: OnceLock<(Fixture, Crate)> = OnceLock::new();
    let (_, crate_data) = FIXTURE.get_or_init(|| {
        let fixture = fixture_from_source(STAGE2_SOURCE).expect("stage 2 fixture files");
        let crate_data = inspect_fixture(&fixture.package).expect("stage 2 rustdoc JSON");
        (fixture, crate_data)
    });
    crate_data
}

/// Return canonical trait paths for compiler-generated negative impls.
fn negative_auto_traits(crate_data: &Crate, name: &str) -> Vec<String> {
    let item = crate_data
        .index
        .values()
        .find(|item| item.name.as_deref() == Some(name))
        .expect("fixture struct");
    let ItemEnum::Struct(struct_) = &item.inner else {
        panic!("fixture item must be a struct");
    };
    struct_
        .impls
        .iter()
        .filter_map(|id| {
            let ItemEnum::Impl(impl_) = &crate_data.index.get(id)?.inner else {
                return None;
            };
            (impl_.is_synthetic && impl_.is_negative)
                .then_some(impl_.trait_.as_ref())
                .flatten()
                .and_then(|trait_| crate_data.paths.get(&trait_.id))
                .map(|summary| summary.path.join("::"))
        })
        .collect()
}

#[test]
fn snapshot_places_reexports_and_impls_once() -> Result<()> {
    let output = snapshot(stage2_crate())?;

    // Alias wins the equal-length public-path tie by lexical order.
    assert_eq!(output.matches("pub struct Alias").count(), 1, "{output}");
    assert_eq!(output.matches("pub struct Shared").count(), 0, "{output}");
    assert_eq!(
        output.matches("pub use crate::Alias as Shared;").count(),
        3,
        "{output}"
    );
    assert_eq!(output.matches("pub struct Internal").count(), 1, "{output}");
    assert_eq!(
        output.matches("pub fn shared(&self)").count(),
        1,
        "{output}"
    );
    assert_eq!(output.matches("pub fn ping(&self)").count(), 1, "{output}");
    assert!(!output.contains("pub mod private_home"), "{output}");
    assert!(output.contains("pub mod first"), "{output}");
    assert!(output.contains("pub mod second"), "{output}");
    assert!(output.contains("pub fn first(&self);"), "{output}");
    assert!(output.contains("pub fn second(&self);"), "{output}");
    assert_eq!(output.matches("impl Dual").count(), 2, "{output}");

    let local = output
        .find("pub struct LocalError")
        .expect("local trait argument");
    let from = output
        .find("impl From<LocalError> for")
        .expect("foreign impl");
    let named = output.find("pub struct Named").expect("next local type");
    assert!(local < from && from < named, "{output}");
    assert_eq!(
        output.matches("impl From<LocalError> for").count(),
        1,
        "{output}"
    );

    let marker = output.find("pub trait Marker").expect("local trait");
    let blanket = output
        .find("impl<T: Clone> Marker for T")
        .expect("blanket impl");
    let send = output.find("pub trait Send").expect("next local trait");
    assert!(marker < blanket && blanket < send, "{output}");
    Ok(())
}

#[test]
fn private_self_impl_does_not_leak_through_public_trait_argument() -> Result<()> {
    let fixture = fixture_from_source(
        "pub struct PublicError;\nmod private {\n    pub(crate) struct Hidden;\n    impl From<super::PublicError> for Hidden {\n        fn from(_: super::PublicError) -> Self { Self }\n    }\n}\n",
    )?;
    let output = snapshot(&inspect_fixture(&fixture.package)?)?;
    assert!(output.contains("pub struct PublicError;"), "{output}");
    assert!(!output.contains("Hidden"), "{output}");
    Ok(())
}

#[test]
fn snapshot_marks_boundary_facts_without_other_auto_traits() -> Result<()> {
    let crate_data = stage2_crate();
    let thread_traits = negative_auto_traits(crate_data, "ThreadBound");
    assert!(thread_traits.contains(&"core::marker::Send".to_string()));
    assert!(thread_traits.contains(&"core::marker::Sync".to_string()));
    assert!(
        negative_auto_traits(crate_data, "UnwindOnly")
            .contains(&"core::panic::unwind_safe::UnwindSafe".to_string())
    );

    let output = snapshot(crate_data)?;
    assert!(
        output.contains("pub visible: u8,\n    /* private fields */"),
        "{output}"
    );
    assert!(output.contains("impl !Send for ThreadBound {}"), "{output}");
    assert!(output.contains("impl !Sync for ThreadBound {}"), "{output}");
    assert!(
        output.contains("impl !Sync for ThreadBound {}\n\n/// A type with one unwind marker."),
        "{output}"
    );
    assert!(!output.contains("impl !UnwindSafe"), "{output}");
    assert!(output.contains("pub trait Send"), "{output}");
    assert!(output.contains("impl crate::Send for Named {}"), "{output}");
    assert!(
        output.contains("// Not dyn-compatible.\npub trait StaticOnly"),
        "{output}"
    );
    assert!(output.contains("// Sealed.\npub trait Exposed"), "{output}");
    assert!(output.contains("pub fn free_function();"), "{output}");
    Ok(())
}

#[test]
fn snapshot_stage2_is_stable_across_processes() -> Result<()> {
    const OUTPUT_PATH: &str = "RUSKEL_STAGE2_SNAPSHOT_CHILD_OUTPUT";
    if let Some(path) = env::var_os(OUTPUT_PATH) {
        fs::write(path, snapshot(stage2_crate())?)?;
        return Ok(());
    }

    let output_dir = tempfile::tempdir()?;
    let binary = env::current_exe()?;
    let mut captures = Vec::new();
    for index in 0..2 {
        let path = output_dir.path().join(format!("capture-{index}.rs"));
        let child = Command::new(&binary)
            .arg("--exact")
            .arg("render::snapshot_tests::snapshot_stage2_is_stable_across_processes")
            .env(OUTPUT_PATH, &path)
            .output()?;
        assert!(
            child.status.success(),
            "child snapshot failed: {}",
            String::from_utf8_lossy(&child.stderr)
        );
        captures.push(fs::read(path)?);
    }
    assert_eq!(captures[0], captures[1]);
    Ok(())
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
        b"brace_style = \"PreferSameLine\"\nnewline_style = \"Unix\"\ngroup_imports = \"StdExternalCrate\"\nimports_granularity = \"Crate\"\n"
    );
}

#[test]
fn snapshot_is_stable_across_unordered_rustdoc_sequences() -> Result<()> {
    let root = fixture()?;
    let original = inspect_fixture(&root.package)?;
    let expected = snapshot(&original)?;
    assert!(!expected.contains("TrivialClone"), "{expected}");
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
    assert!(expected.contains("/// An ordered data type.\n///\n/// More detail.\n#[derive(Clone, Display)]\n#[repr(C)]\n#[doc(hidden)]\npub struct Alpha"));
    assert!(!expected.contains("impl Clone for Alpha"));
    assert!(!expected.contains("impl Display for Alpha"));
    assert!(expected.contains("#[derive(Clone, Copy)]\npub union Choice"));
    assert!(!expected.contains("impl Clone for Choice"));
    assert!(expected.contains("impl Renamed"));
    assert!(expected.contains("pub const VERSION: u8 = 1;"));
    assert!(expected.contains("const KIND: u8;"));
    assert!(expected.contains("const KIND: u8 = 1;"));
    assert!(expected.contains("pub type SingletonTuple = (u32,);"));
    assert!(expected.contains("pub type CCallback = unsafe extern \"C\" fn(value: i32) -> i32;"));
    assert!(expected.contains("pub type HrtbCallback = for<'a> fn(value: &'a i32) -> &'a i32;"));
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

    let mut associated_type = original.clone();
    let container = associated_type
        .index
        .values_mut()
        .find(|item| {
            item.name.as_deref() == Some("Container")
                && matches!(&item.inner, ItemEnum::AssocType { .. })
        })
        .expect("generic associated type");
    let ItemEnum::AssocType { generics, .. } = &mut container.inner else {
        panic!("Container must be an associated type");
    };
    generics.params[0].name = "Changed".to_string();
    assert_ne!(snapshot(&associated_type)?, expected);

    let mut associated_type_where = original.clone();
    let container = associated_type_where
        .index
        .values_mut()
        .find(|item| {
            item.name.as_deref() == Some("Container")
                && matches!(&item.inner, ItemEnum::AssocType { .. })
        })
        .expect("generic associated type where clause");
    let ItemEnum::AssocType { generics, .. } = &mut container.inner else {
        panic!("Container must be an associated type");
    };
    let Some(rustdoc_types::WherePredicate::BoundPredicate { bounds, .. }) =
        generics.where_predicates.first_mut()
    else {
        panic!("Container must have a where predicate");
    };
    let Some(rustdoc_types::GenericBound::TraitBound { trait_, .. }) = bounds.first_mut() else {
        panic!("Container where predicate must have a trait bound");
    };
    trait_.path = "Send".to_string();
    // Name resolution follows the item ID, so a written-path change has no
    // effect.
    assert_eq!(snapshot(&associated_type_where)?, expected);

    let mut associated_const = original.clone();
    let version = associated_const
        .index
        .values_mut()
        .find(|item| item.name.as_deref() == Some("VERSION"))
        .expect("associated constant");
    let ItemEnum::AssocConst { value, .. } = &mut version.inner else {
        panic!("VERSION must be an associated constant");
    };
    *value = Some("2".to_string());
    assert_ne!(snapshot(&associated_const)?, expected);

    let mut abi = original.clone();
    let callback = abi
        .index
        .values_mut()
        .find(|item| item.name.as_deref() == Some("CCallback"))
        .expect("function pointer alias");
    let ItemEnum::TypeAlias(alias) = &mut callback.inner else {
        panic!("CCallback must be a type alias");
    };
    let Type::FunctionPointer(pointer) = &mut alias.type_ else {
        panic!("CCallback must refer to a function pointer");
    };
    pointer.header.abi = rustdoc_types::Abi::C { unwind: true };
    assert_ne!(snapshot(&abi)?, expected);

    let mut variadic = original.clone();
    let callback = variadic
        .index
        .values_mut()
        .find(|item| item.name.as_deref() == Some("CCallback"))
        .expect("function pointer alias");
    let ItemEnum::TypeAlias(alias) = &mut callback.inner else {
        panic!("CCallback must be a type alias");
    };
    let Type::FunctionPointer(pointer) = &mut alias.type_ else {
        panic!("CCallback must refer to a function pointer");
    };
    pointer.sig.is_c_variadic = true;
    assert_ne!(snapshot(&variadic)?, expected);

    let mut singleton_tuple = original.clone();
    let singleton_item = singleton_tuple
        .index
        .values_mut()
        .find(|item| item.name.as_deref() == Some("SingletonTuple"))
        .expect("singleton tuple alias");
    let ItemEnum::TypeAlias(alias) = &mut singleton_item.inner else {
        panic!("SingletonTuple must be a type alias");
    };
    alias.type_ = Type::Tuple(vec![Type::Primitive("u64".to_string())]);
    assert_ne!(snapshot(&singleton_tuple)?, expected);

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
fn snapshot_preserves_external_reexports_without_inline_definitions() -> Result<()> {
    let root = fixture()?;
    let mut crate_data = inspect_fixture(&root.package)?;
    let external_id = rustdoc_types::Id(u32::MAX);
    let use_id = crate_data
        .index
        .values()
        .find(|item| matches!(item.inner, ItemEnum::Use(_)))
        .unwrap()
        .id;
    crate_data.paths.insert(
        external_id,
        rustdoc_types::ItemSummary {
            crate_id: 1,
            path: vec!["dependency".into()],
            kind: rustdoc_types::ItemKind::Module,
        },
    );
    let item = crate_data.index.get_mut(&use_id).unwrap();
    item.docs = Some("External API.".into());
    item.inner = ItemEnum::Use(rustdoc_types::Use {
        source: "dependency".into(),
        name: "type".into(),
        id: Some(external_id),
        is_glob: false,
    });
    let rendered = snapshot(&crate_data)?;
    assert!(rendered.contains("/// External API."), "{rendered}");
    assert!(
        rendered.contains("pub use dependency as r#type;"),
        "{rendered}"
    );

    let ItemEnum::Use(import) = &mut crate_data.index.get_mut(&use_id).unwrap().inner else {
        unreachable!()
    };
    import.is_glob = true;
    let rendered = snapshot(&crate_data)?;
    assert!(rendered.contains("/// External API."), "{rendered}");
    assert!(rendered.contains("pub use dependency::*;"), "{rendered}");

    crate_data.paths.get_mut(&external_id).unwrap().crate_id = 0;
    assert!(
        snapshot(&crate_data).is_err(),
        "missing local definitions must fail"
    );
    crate_data.paths.remove(&external_id);
    assert!(
        snapshot(&crate_data).is_err(),
        "unknown references must fail"
    );
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
