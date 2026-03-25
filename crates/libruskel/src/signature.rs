//! Shared item signature rendering for search and skeleton output.

use rustdoc_types::{Crate, Item, ItemEnum, MacroKind, Variant, VariantKind};

use crate::{
    crateutils::{
        render_function_args, render_generic_bounds, render_generics, render_name,
        render_return_type, render_type, render_vis, render_where_clause,
    },
    search::SearchItemKind,
};

/// Render a compact item signature for search output and renderer headers.
pub fn item_signature(crate_data: &Crate, item: &Item, kind: SearchItemKind) -> Option<String> {
    match (&item.inner, kind) {
        (ItemEnum::Function(function), SearchItemKind::Function)
        | (ItemEnum::Function(function), SearchItemKind::Method)
        | (ItemEnum::Function(function), SearchItemKind::TraitMethod) => {
            Some(function_signature(item, function))
        }
        (ItemEnum::StructField(ty), SearchItemKind::Field) => Some(field_signature(item, ty)),
        (ItemEnum::Struct(struct_), SearchItemKind::Struct) => Some(
            format!(
                "{}struct {}{}{}",
                render_vis(item),
                render_name(item),
                render_generics(&struct_.generics),
                render_where_clause(&struct_.generics)
            )
            .trim()
            .to_string(),
        ),
        (ItemEnum::Union(union_), SearchItemKind::Union) => Some(
            format!(
                "{}union {}{}{}",
                render_vis(item),
                render_name(item),
                render_generics(&union_.generics),
                render_where_clause(&union_.generics)
            )
            .trim()
            .to_string(),
        ),
        (ItemEnum::Enum(enum_), SearchItemKind::Enum) => Some(
            format!(
                "{}enum {}{}{}",
                render_vis(item),
                render_name(item),
                render_generics(&enum_.generics),
                render_where_clause(&enum_.generics)
            )
            .trim()
            .to_string(),
        ),
        (ItemEnum::Trait(trait_), SearchItemKind::Trait) => {
            let mut signature = String::new();
            signature.push_str(&render_vis(item));
            if trait_.is_unsafe {
                signature.push_str("unsafe ");
            }
            signature.push_str("trait ");
            signature.push_str(&render_name(item));
            signature.push_str(&render_generics(&trait_.generics));
            if !trait_.bounds.is_empty() {
                let bounds = render_generic_bounds(&trait_.bounds);
                if !bounds.is_empty() {
                    signature.push_str(": ");
                    signature.push_str(&bounds);
                }
            }
            signature.push_str(&render_where_clause(&trait_.generics));
            Some(signature.trim().to_string())
        }
        (ItemEnum::TraitAlias(alias), SearchItemKind::TraitAlias) => {
            let mut signature = String::new();
            signature.push_str(&render_vis(item));
            signature.push_str("trait ");
            signature.push_str(&render_name(item));
            signature.push_str(&render_generics(&alias.generics));
            let bounds = render_generic_bounds(&alias.params);
            if !bounds.is_empty() {
                signature.push_str(" = ");
                signature.push_str(&bounds);
            }
            signature.push_str(&render_where_clause(&alias.generics));
            Some(signature.trim().to_string())
        }
        (ItemEnum::TypeAlias(type_alias), SearchItemKind::TypeAlias) => Some(
            format!(
                "{}type {}{}{} = {}",
                render_vis(item),
                render_name(item),
                render_generics(&type_alias.generics),
                render_where_clause(&type_alias.generics),
                render_type(&type_alias.type_)
            )
            .trim()
            .to_string(),
        ),
        (ItemEnum::Constant { type_, .. }, SearchItemKind::Constant) => Some(
            format!(
                "{}const {}: {}",
                render_vis(item),
                render_name(item),
                render_type(type_)
            )
            .trim()
            .to_string(),
        ),
        (ItemEnum::Static(static_), SearchItemKind::Static) => Some(
            format!(
                "{}static {}: {}",
                render_vis(item),
                render_name(item),
                render_type(&static_.type_)
            )
            .trim()
            .to_string(),
        ),
        (ItemEnum::AssocConst { type_, .. }, SearchItemKind::AssocConst) => Some(format!(
            "const {}: {}",
            render_name(item),
            render_type(type_)
        )),
        (ItemEnum::AssocType { bounds, type_, .. }, SearchItemKind::AssocType) => {
            if let Some(ty) = type_ {
                Some(format!("type {} = {}", render_name(item), render_type(ty)))
            } else if !bounds.is_empty() {
                Some(format!(
                    "type {}: {}",
                    render_name(item),
                    render_generic_bounds(bounds)
                ))
            } else {
                Some(format!("type {}", render_name(item)))
            }
        }
        (ItemEnum::Macro(_), SearchItemKind::Macro) => Some(format!("macro {}", render_name(item))),
        (ItemEnum::ProcMacro(proc_macro), SearchItemKind::ProcMacro) => {
            let prefix = match proc_macro.kind {
                MacroKind::Derive => "#[proc_macro_derive]",
                MacroKind::Attr => "#[proc_macro_attribute]",
                MacroKind::Bang => "#[proc_macro]",
            };
            Some(format!("{prefix} {}", render_name(item)))
        }
        (ItemEnum::Use(import), SearchItemKind::Use) => {
            let mut signature = String::new();
            signature.push_str(&render_vis(item));
            signature.push_str("use ");
            signature.push_str(&import.source);
            if import.name != import.source.split("::").last().unwrap_or(&import.source) {
                signature.push_str(" as ");
                signature.push_str(&import.name);
            }
            if import.is_glob {
                signature.push_str("::*");
            }
            Some(signature.trim().to_string())
        }
        (ItemEnum::Primitive(_), SearchItemKind::Primitive) => {
            Some(format!("primitive {}", render_name(item)))
        }
        (ItemEnum::Module(_), SearchItemKind::Module) => Some(
            format!("{}mod {}", render_vis(item), render_name(item))
                .trim()
                .to_string(),
        ),
        (ItemEnum::Module(_), SearchItemKind::Crate) => Some(render_name(item)),
        (ItemEnum::Variant(variant), SearchItemKind::EnumVariant) => {
            Some(variant_signature(crate_data, item, variant))
        }
        _ => None,
    }
}

/// Render a function-like signature shared by free functions, methods, and trait methods.
fn function_signature(item: &Item, function: &rustdoc_types::Function) -> String {
    let mut parts: Vec<String> = Vec::new();
    let vis = render_vis(item);
    if !vis.trim().is_empty() {
        parts.push(vis.trim().to_string());
    }

    let mut qualifiers = Vec::new();
    if function.header.is_const {
        qualifiers.push("const");
    }
    if function.header.is_async {
        qualifiers.push("async");
    }
    if function.header.is_unsafe {
        qualifiers.push("unsafe");
    }
    if !qualifiers.is_empty() {
        parts.push(qualifiers.join(" "));
    }
    parts.push("fn".to_string());

    let mut signature = parts.join(" ");
    if !signature.is_empty() {
        signature.push(' ');
    }
    signature.push_str(&render_name(item));
    signature.push_str(&render_generics(&function.generics));
    signature.push('(');
    signature.push_str(&render_function_args(&function.sig));
    signature.push(')');
    signature.push_str(&render_return_type(&function.sig));
    signature.push_str(&render_where_clause(&function.generics));
    signature
}

/// Render a field signature with visibility and type.
fn field_signature(item: &Item, ty: &rustdoc_types::Type) -> String {
    let mut signature = String::new();
    let vis = render_vis(item);
    if !vis.trim().is_empty() {
        signature.push_str(vis.trim());
        signature.push(' ');
    }
    if let Some(name) = item.name.as_deref() {
        signature.push_str(name);
        signature.push_str(": ");
    }
    signature.push_str(&render_type(ty));
    signature
}

/// Render an enum variant signature, including inline field types when available.
fn variant_signature(crate_data: &Crate, item: &Item, variant: &Variant) -> String {
    let mut signature = render_name(item);
    match &variant.kind {
        VariantKind::Plain => {}
        VariantKind::Tuple(fields) => {
            let mut parts = Vec::new();
            for field in fields {
                if let Some(field_id) = field
                    && let Some(field_item) = crate_data.index.get(field_id)
                    && let ItemEnum::StructField(ty) = &field_item.inner
                {
                    parts.push(render_type(ty));
                }
            }
            signature.push('(');
            signature.push_str(&parts.join(", "));
            signature.push(')');
        }
        VariantKind::Struct { fields, .. } => {
            let mut parts = Vec::new();
            for field_id in fields {
                if let Some(field_item) = crate_data.index.get(field_id)
                    && let ItemEnum::StructField(ty) = &field_item.inner
                {
                    let name = field_item
                        .name
                        .as_deref()
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| "_".to_string());
                    parts.push(format!("{name}: {}", render_type(ty)));
                }
            }
            signature.push_str(" { ");
            signature.push_str(&parts.join(", "));
            signature.push_str(" }");
        }
    }
    signature
}
