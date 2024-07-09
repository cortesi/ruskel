use rust_format::{Config, Formatter, RustFmt};
use rustdoc_types::{
    Crate, GenericBound, GenericParamDef, GenericParamDefKind, Id, Impl, Item, ItemEnum, MacroKind,
    PolyTrait, StructKind, TraitBoundModifier, VariantKind, Visibility,
};

use crate::crateutils::*;
use crate::error::Result;

macro_rules! extract_item {
    ($item:expr, $variant:path) => {
        match &$item.inner {
            $variant(inner) => inner,
            _ => panic!("Expected {}, found {:?}", stringify!($variant), $item.inner),
        }
    };
    ($item:expr, $variant:path { $($field:ident),+ }) => {
        match &$item.inner {
            $variant { $($field,)+ .. } => ($($field,)+),
            _ => panic!("Expected {}, found {:?}", stringify!($variant), $item.inner),
        }
    };
}

pub struct Renderer {
    formatter: RustFmt,
    render_auto_impls: bool,
    render_private_items: bool,
    render_blanket_impls: bool,
    filter: String,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    fn new() -> Self {
        let config = Config::new_str().option("brace_style", "PreferSameLine");

        Self {
            formatter: RustFmt::from_config(config),
            render_auto_impls: false,
            render_private_items: false,
            render_blanket_impls: false,
            filter: String::new(),
        }
    }

    pub fn with_filter(mut self, filter: &str) -> Self {
        self.filter = filter.to_string();
        self
    }

    pub fn with_blanket_impls(mut self, render_blanket_impls: bool) -> Self {
        self.render_blanket_impls = render_blanket_impls;
        self
    }

    pub fn with_auto_impls(mut self, render_auto_impls: bool) -> Self {
        self.render_auto_impls = render_auto_impls;
        self
    }

    pub fn with_private_items(mut self, render_private_items: bool) -> Self {
        self.render_private_items = render_private_items;
        self
    }

    pub fn render(&self, crate_data: &Crate) -> Result<String> {
        let mut output = String::new();

        if let Some(root_item) = crate_data.index.get(&crate_data.root) {
            let unformatted = self.render_item(root_item, crate_data, false);
            output.push_str(&unformatted);
        }

        Ok(self.formatter.format_str(&output)?)
    }

    fn should_show(&self, item: &Item, crate_data: &Crate) -> bool {
        if !self.render_private_items && !matches!(item.visibility, Visibility::Public) {
            return false;
        }

        if !self.filter.is_empty() {
            if let Some(summary) = crate_data.paths.get(&item.id) {
                let item_path = summary.path.join("::");
                if !self.filter.starts_with(&item_path) && !item_path.starts_with(&self.filter) {
                    return false;
                }
            } else {
                return false;
            }
        }

        true
    }

    fn should_render_impl(&self, impl_: &Impl) -> bool {
        if impl_.synthetic && !self.render_auto_impls {
            return false;
        }

        let is_blanket = impl_.blanket_impl.is_some();
        if is_blanket && !self.render_blanket_impls {
            return false;
        }

        if !self.render_auto_impls {
            // List of traits that we don't want to render by default
            const FILTERED_TRAITS: &[&str] = &[
                "Any",
                "Send",
                "Sync",
                "Unpin",
                "UnwindSafe",
                "RefUnwindSafe",
                "Borrow",
                "BorrowMut",
                "From",
                "Into",
                "TryFrom",
                "TryInto",
                "AsRef",
                "AsMut",
                "Default",
                "Debug",
                "PartialEq",
                "Eq",
                "PartialOrd",
                "Ord",
                "Hash",
                "Deref",
                "DerefMut",
                "Drop",
                "IntoIterator",
                "CloneToUninit",
                "ToOwned",
            ];

            if let Some(trait_path) = &impl_.trait_ {
                let trait_name = trait_path
                    .name
                    .split("::")
                    .last()
                    .unwrap_or(&trait_path.name);
                if FILTERED_TRAITS.contains(&trait_name) && is_blanket {
                    return false;
                }
            }
        }

        true
    }

    fn render_item(&self, item: &Item, crate_data: &Crate, force_private: bool) -> String {
        let output = match &item.inner {
            ItemEnum::Module(_) => self.render_module(item, crate_data),
            ItemEnum::Struct(_) => self.render_struct(item, crate_data),
            ItemEnum::Enum(_) => self.render_enum(item, crate_data),
            ItemEnum::Trait(_) => self.render_trait(item, crate_data),
            ItemEnum::Import(_) => self.render_import(item, crate_data),
            ItemEnum::Function(_) => self.render_function(item, false),
            ItemEnum::Constant { .. } => self.render_constant(item),
            ItemEnum::TypeAlias(_) => self.render_type_alias(item),
            ItemEnum::Macro(_) => self.render_macro(item),
            ItemEnum::ProcMacro(_) => self.render_proc_macro(item),
            _ => String::new(),
        };

        if !force_private && !self.should_show(item, crate_data) {
            String::new()
        } else {
            output
        }
    }

    fn render_proc_macro(&self, item: &Item) -> String {
        let mut output = docs(item);

        let fn_name = render_name(item);

        let proc_macro = extract_item!(item, ItemEnum::ProcMacro);
        match proc_macro.kind {
            MacroKind::Derive => {
                if !proc_macro.helpers.is_empty() {
                    output.push_str(&format!(
                        "#[proc_macro_derive({}, attributes({}))]\n",
                        fn_name,
                        proc_macro.helpers.join(", ")
                    ));
                } else {
                    output.push_str(&format!("#[proc_macro_derive({})]\n", fn_name));
                }
            }
            MacroKind::Attr => {
                output.push_str("#[proc_macro_attribute]\n");
            }
            MacroKind::Bang => {
                output.push_str("#[proc_macro]\n");
            }
        }
        let (args, return_type) = match proc_macro.kind {
            MacroKind::Attr => (
                "attr: proc_macro::TokenStream, item: proc_macro::TokenStream",
                "proc_macro::TokenStream",
            ),
            _ => ("input: proc_macro::TokenStream", "proc_macro::TokenStream"),
        };

        output.push_str(&format!(
            "pub fn {}({}) -> {} {{}}\n",
            fn_name, args, return_type
        ));

        output
    }

    fn render_macro(&self, item: &Item) -> String {
        let mut output = docs(item);

        let macro_def = extract_item!(item, ItemEnum::Macro);
        // Add #[macro_export] for public macros
        output.push_str("#[macro_export]\n");
        output.push_str(&format!("{}\n", macro_def));

        output
    }

    fn render_type_alias(&self, item: &Item) -> String {
        let type_alias = extract_item!(item, ItemEnum::TypeAlias);
        let mut output = docs(item);

        output.push_str(&format!(
            "{}type {}{}{}",
            render_vis(item),
            render_name(item),
            render_generics(&type_alias.generics),
            render_where_clause(&type_alias.generics),
        ));

        output.push_str(&format!("= {};\n\n", render_type(&type_alias.type_)));

        output
    }

    fn render_import(&self, item: &Item, crate_data: &Crate) -> String {
        let import = extract_item!(item, ItemEnum::Import);

        if import.glob {
            if let Some(source_id) = &import.id {
                if let Some(source_item) = crate_data.index.get(source_id) {
                    let module = extract_item!(source_item, ItemEnum::Module);
                    let mut output = String::new();
                    for item_id in &module.items {
                        if let Some(item) = crate_data.index.get(item_id) {
                            if self.should_show(item, crate_data) {
                                output.push_str(&self.render_item(item, crate_data, true));
                            }
                        }
                    }
                    return output;
                }
            }
            // If we can't resolve the glob import, fall back to rendering it as-is
            return format!("pub use {}::*;\n", import.source);
        }

        if let Some(imported_item) = import.id.as_ref().and_then(|id| crate_data.index.get(id)) {
            return self.render_item(imported_item, crate_data, true);
        }

        let mut output = docs(item);
        if import.name != import.source.split("::").last().unwrap_or(&import.source) {
            output.push_str(&format!("pub use {} as {};\n", import.source, import.name));
        } else {
            output.push_str(&format!("pub use {};\n", import.source));
        }

        output
    }

    fn render_impl(&self, item: &Item, crate_data: &Crate) -> String {
        let mut output = docs(item);
        let impl_ = extract_item!(item, ItemEnum::Impl);

        if !self.should_render_impl(impl_) {
            return String::new();
        }

        if let Some(trait_) = &impl_.trait_ {
            if let Some(trait_item) = crate_data.index.get(&trait_.id) {
                if !self.should_show(trait_item, crate_data) {
                    return String::new();
                }
            }
        }

        let where_clause = render_where_clause(&impl_.generics);

        let trait_part = if let Some(trait_) = &impl_.trait_ {
            let trait_path = render_path(trait_);
            if !trait_path.is_empty() {
                format!("{} for ", trait_path)
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        output.push_str(&format!(
            "{}impl{} {}{}",
            if impl_.is_unsafe { "unsafe " } else { "" },
            render_generics(&impl_.generics),
            trait_part,
            render_type(&impl_.for_)
        ));

        if !where_clause.is_empty() {
            output.push_str(&format!("\n{}", where_clause));
        }

        output.push_str(" {\n");

        for item_id in &impl_.items {
            if let Some(item) = crate_data.index.get(item_id) {
                let is_trait_impl = impl_.trait_.is_some();
                if is_trait_impl || self.should_show(item, crate_data) {
                    output.push_str(&self.render_impl_item(item));
                }
            }
        }

        output.push_str("}\n\n");

        output
    }

    fn render_impl_item(&self, item: &Item) -> String {
        match &item.inner {
            ItemEnum::Function(_) => self.render_function(item, false),
            ItemEnum::Constant { .. } => self.render_constant(item),
            ItemEnum::AssocType { .. } => Self::render_associated_type(item),
            ItemEnum::TypeAlias(_) => self.render_type_alias(item),
            _ => String::new(),
        }
    }

    fn render_associated_type(item: &Item) -> String {
        let (bounds, default) = extract_item!(item, ItemEnum::AssocType { bounds, default });

        let bounds_str = if !bounds.is_empty() {
            format!(": {}", Self::render_generic_bounds(bounds))
        } else {
            String::new()
        };
        let default_str = default
            .as_ref()
            .map(|d| format!(" = {}", render_type(d)))
            .unwrap_or_default();
        format!("type {}{}{};\n", render_name(item), bounds_str, default_str)
    }

    fn render_enum(&self, item: &Item, crate_data: &Crate) -> String {
        let mut output = docs(item);

        let enum_ = extract_item!(item, ItemEnum::Enum);

        let generics = render_generics(&enum_.generics);
        let where_clause = render_where_clause(&enum_.generics);

        output.push_str(&format!(
            "{}enum {}{}{} {{\n",
            render_vis(item),
            render_name(item),
            generics,
            where_clause
        ));

        for variant_id in &enum_.variants {
            if let Some(variant_item) = crate_data.index.get(variant_id) {
                output.push_str(&self.render_enum_variant(variant_item, crate_data));
            }
        }

        output.push_str("}\n\n");

        output
    }

    fn render_enum_variant(&self, item: &Item, crate_data: &Crate) -> String {
        let mut output = docs(item);

        let variant = extract_item!(item, ItemEnum::Variant);

        output.push_str(&format!("    {}", render_name(item),));

        match &variant.kind {
            VariantKind::Plain => {}
            VariantKind::Tuple(fields) => {
                let fields_str = fields
                    .iter()
                    .filter_map(|field| {
                        field.as_ref().map(|id| {
                            if let Some(field_item) = crate_data.index.get(id) {
                                let ty = extract_item!(field_item, ItemEnum::StructField);
                                render_type(ty)
                            } else {
                                "".to_string()
                            }
                        })
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                output.push_str(&format!("({})", fields_str));
            }
            VariantKind::Struct { fields, .. } => {
                output.push_str(" {\n");
                for field in fields {
                    if let Some(_field_item) = crate_data.index.get(field) {
                        output.push_str(&format!(
                            "        {}\n",
                            self.render_struct_field(crate_data, field)
                        ));
                    }
                }
                output.push_str("    }");
            }
        }

        if let Some(discriminant) = &variant.discriminant {
            output.push_str(&format!(" = {}", discriminant.expr));
        }

        output.push_str(",\n");

        output
    }

    fn render_trait(&self, item: &Item, crate_data: &Crate) -> String {
        let mut output = docs(item);

        let trait_ = extract_item!(item, ItemEnum::Trait);

        let generics = render_generics(&trait_.generics);
        let where_clause = render_where_clause(&trait_.generics);

        let bounds = if !trait_.bounds.is_empty() {
            format!(": {}", Self::render_generic_bounds(&trait_.bounds))
        } else {
            String::new()
        };

        let unsafe_prefix = if trait_.is_unsafe { "unsafe " } else { "" };

        output.push_str(&format!(
            "{}{}trait {}{}{}{} {{\n",
            render_vis(item),
            unsafe_prefix,
            render_name(item),
            generics,
            bounds,
            where_clause
        ));

        for item_id in &trait_.items {
            if let Some(item) = crate_data.index.get(item_id) {
                output.push_str(&self.render_trait_item(item));
            }
        }

        output.push_str("}\n\n");

        output
    }

    fn render_trait_item(&self, item: &Item) -> String {
        match &item.inner {
            ItemEnum::Function(_) => self.render_function(item, true),
            ItemEnum::AssocConst { type_, default } => {
                let default_str = default
                    .as_ref()
                    .map(|d| format!(" = {}", d))
                    .unwrap_or_default();
                format!(
                    "const {}: {}{};\n",
                    render_name(item),
                    render_type(type_),
                    default_str
                )
            }
            ItemEnum::AssocType {
                bounds,
                generics,
                default,
            } => {
                let bounds_str = if !bounds.is_empty() {
                    format!(": {}", Self::render_generic_bounds(bounds))
                } else {
                    String::new()
                };
                let generics_str = render_generics(generics);
                let default_str = default
                    .as_ref()
                    .map(|d| format!(" = {}", render_type(d)))
                    .unwrap_or_default();
                format!(
                    "type {}{}{}{};\n",
                    render_name(item),
                    generics_str,
                    bounds_str,
                    default_str
                )
            }
            _ => String::new(),
        }
    }

    fn render_generic_bounds(bounds: &[GenericBound]) -> String {
        bounds
            .iter()
            .map(Self::render_generic_bound)
            .collect::<Vec<_>>()
            .join(" + ")
    }

    fn render_struct(&self, item: &Item, crate_data: &Crate) -> String {
        let mut output = docs(item);

        let struct_ = extract_item!(item, ItemEnum::Struct);

        let generics = render_generics(&struct_.generics);
        let where_clause = render_where_clause(&struct_.generics);

        match &struct_.kind {
            StructKind::Unit => {
                output.push_str(&format!(
                    "{}struct {}{}{};\n\n",
                    render_vis(item),
                    render_name(item),
                    generics,
                    where_clause
                ));
            }
            StructKind::Tuple(fields) => {
                let fields_str = fields
                    .iter()
                    .filter_map(|field| {
                        field.as_ref().map(|id| {
                            if let Some(field_item) = crate_data.index.get(id) {
                                let ty = extract_item!(field_item, ItemEnum::StructField);
                                if !self.should_show(field_item, crate_data) {
                                    "_".to_string()
                                } else {
                                    format!("{}{}", render_vis(field_item), render_type(ty))
                                }
                            } else {
                                "".to_string()
                            }
                        })
                    })
                    .collect::<Vec<_>>()
                    .join(", ");

                output.push_str(&format!(
                    "{}struct {}{}({}){};\n\n",
                    render_vis(item),
                    render_name(item),
                    generics,
                    fields_str,
                    where_clause
                ));
            }
            StructKind::Plain { fields, .. } => {
                output.push_str(&format!(
                    "{}struct {}{}{} {{\n",
                    render_vis(item),
                    render_name(item),
                    generics,
                    where_clause
                ));
                for field in fields {
                    output.push_str(&self.render_struct_field(crate_data, field));
                }
                output.push_str("}\n\n");
            }
        }

        // Render impl blocks
        for impl_id in &struct_.impls {
            if let Some(impl_item) = crate_data.index.get(impl_id) {
                let impl_ = extract_item!(impl_item, ItemEnum::Impl);
                if self.should_render_impl(impl_) {
                    output.push_str(&self.render_impl(impl_item, crate_data));
                }
            }
        }

        output
    }

    fn render_struct_field(&self, crate_data: &Crate, field_id: &Id) -> String {
        if let Some(field_item) = crate_data.index.get(field_id) {
            if self.should_show(field_item, crate_data) {
                let ty = extract_item!(field_item, ItemEnum::StructField);
                format!(
                    "{}{}: {},\n",
                    render_vis(field_item),
                    render_name(field_item),
                    render_type(ty)
                )
            } else {
                String::new()
            }
        } else {
            String::new()
        }
    }

    fn render_constant(&self, item: &Item) -> String {
        let mut output = docs(item);

        let (type_, const_) = extract_item!(item, ItemEnum::Constant { type_, const_ });
        output.push_str(&format!(
            "{}const {}: {} = {};\n\n",
            render_vis(item),
            render_name(item),
            render_type(type_),
            const_.expr
        ));

        output
    }

    fn render_module(&self, item: &Item, crate_data: &Crate) -> String {
        let mut output = format!("{}mod {} {{\n", render_vis(item), render_name(item));
        // Add module doc comment if present
        if let Some(docs) = &item.docs {
            for line in docs.lines() {
                output.push_str(&format!("    //! {}\n", line));
            }
            output.push('\n');
        }

        let module = extract_item!(item, ItemEnum::Module);

        for item_id in &module.items {
            if let Some(item) = crate_data.index.get(item_id) {
                // Handle public imports differently
                if let ItemEnum::Import(_) = &item.inner {
                    if self.should_show(item, crate_data) {
                        output.push_str(&self.render_import(item, crate_data));
                        continue;
                    }
                }
                output.push_str(&self.render_item(item, crate_data, false))
            }
        }

        output.push_str("}\n\n");
        output
    }

    fn render_function(&self, item: &Item, is_trait_method: bool) -> String {
        let mut output = docs(item);
        let function = extract_item!(item, ItemEnum::Function);

        // Handle const, async, and unsafe keywords in the correct order
        let mut prefixes = Vec::new();
        if function.header.const_ {
            prefixes.push("const");
        }
        if function.header.async_ {
            prefixes.push("async");
        }
        if function.header.unsafe_ {
            prefixes.push("unsafe");
        }

        output.push_str(&format!(
            "{} {} fn {}{}({}){}{}",
            render_vis(item),
            prefixes.join(" "),
            render_name(item),
            render_generics(&function.generics),
            render_function_args(&function.decl),
            render_return_type(&function.decl),
            render_where_clause(&function.generics)
        ));

        // Use semicolon for trait method declarations, empty body for implementations
        if is_trait_method && !function.has_body {
            output.push_str(";\n\n");
        } else {
            output.push_str(" {}\n\n");
        }

        output
    }

    fn render_poly_trait(poly_trait: &PolyTrait) -> String {
        let generic_params = if poly_trait.generic_params.is_empty() {
            String::new()
        } else {
            let params = poly_trait
                .generic_params
                .iter()
                .filter_map(Self::render_generic_param_def)
                .collect::<Vec<_>>();

            if params.is_empty() {
                String::new()
            } else {
                format!("for<{}> ", params.join(", "))
            }
        };

        format!("{}{}", generic_params, render_path(&poly_trait.trait_))
    }

    fn render_generic_bound(bound: &GenericBound) -> String {
        match bound {
            GenericBound::TraitBound {
                trait_,
                generic_params,
                modifier,
            } => {
                let modifier = match modifier {
                    TraitBoundModifier::None => "",
                    TraitBoundModifier::Maybe => "?",
                    TraitBoundModifier::MaybeConst => "~const",
                };
                let poly_trait = PolyTrait {
                    trait_: trait_.clone(),
                    generic_params: generic_params.clone(),
                };
                format!("{}{}", modifier, Self::render_poly_trait(&poly_trait))
            }
            GenericBound::Outlives(lifetime) => lifetime.clone(),
        }
    }

    fn render_generic_param_def(param: &GenericParamDef) -> Option<String> {
        match &param.kind {
            GenericParamDefKind::Lifetime { outlives } => {
                let outlives = if outlives.is_empty() {
                    String::new()
                } else {
                    format!(": {}", outlives.join(" + "))
                };
                Some(format!("{}{}", param.name, outlives))
            }
            GenericParamDefKind::Type {
                bounds,
                default,
                synthetic,
            } => {
                if *synthetic {
                    None
                } else {
                    let bounds = if bounds.is_empty() {
                        String::new()
                    } else {
                        format!(
                            ": {}",
                            bounds
                                .iter()
                                .map(Self::render_generic_bound)
                                .collect::<Vec<_>>()
                                .join(" + ")
                        )
                    };
                    let default = default
                        .as_ref()
                        .map(|ty| format!(" = {}", render_type(ty)))
                        .unwrap_or_default();
                    Some(format!("{}{}{}", param.name, bounds, default))
                }
            }
            GenericParamDefKind::Const { type_, default } => {
                let default = default
                    .as_ref()
                    .map(|expr| format!(" = {}", expr))
                    .unwrap_or_default();
                Some(format!(
                    "const {}: {}{}",
                    param.name,
                    render_type(type_),
                    default
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ruskel;
    use pretty_assertions::assert_eq;
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

    fn render(renderer: &Renderer, source: &str, expected_output: &str, is_proc_macro: bool) {
        // Create a temporary directory for our dummy crate
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

                [dependencies]
                proc-macro2 = "1.0"
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

        // Parse the crate using Ruskel
        let ruskel = Ruskel::new(lib_rs_path.to_str().unwrap()).unwrap();
        let crate_data = ruskel.json().unwrap();

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
    fn rt_idemp(source: &str) {
        render(&Renderer::default(), source, source, false);
    }

    /// Idempotent rendering test with private items
    fn rt_priv_idemp(source: &str) {
        render(
            &Renderer::default().with_private_items(true),
            source,
            source,
            false,
        );
    }

    /// Render roundtrip
    fn rt(source: &str, expected_output: &str) {
        render(&Renderer::default(), source, expected_output, false);
    }

    /// Render roundtrip with private items
    fn rt_private(source: &str, expected_output: &str) {
        render(
            &Renderer::default().with_private_items(true),
            source,
            expected_output,
            false,
        );
    }

    fn rt_procmacro(source: &str, expected_output: &str) {
        render(&Renderer::default(), source, expected_output, true);
    }

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
                    renderer: $renderer:expr,
                    input: $rt_custom_input:expr,
                    output: $rt_custom_output:expr
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
                        let custom_renderer = $renderer;
                        render(&custom_renderer, $rt_custom_input, $rt_custom_output, false);
                    }
                )*
            }
        };
    }

    #[test]
    fn test_render_constant() {
        rt(
            r#"
                /// This is a documented constant.
                pub const CONSTANT: u32 = 42;
                const PRIVATE_CONSTANT: &str = "Hello, world!";
            "#,
            r#"
                /// This is a documented constant.
                pub const CONSTANT: u32 = 42;
            "#,
        );
        rt_priv_idemp(
            r#"
                /// This is a documented constant.
                pub const CONSTANT: u32 = 42;
                const PRIVATE_CONSTANT: &str = "Hello, world!";
            "#,
        );
    }

    #[test]
    fn test_render_imports() {
        rt(
            r#"
                use std::collections::HashMap;
                pub use std::rc::Rc;
                pub use std::sync::{Arc, Mutex};
            "#,
            r#"
                pub use std::rc::Rc;
                pub use std::sync::Arc;
                pub use std::sync::Mutex;
            "#,
        );
    }

    #[test]
    fn test_render_imports_inline() {
        let input = r#"
                mod private {
                    pub struct PrivateStruct;
                }

                pub use private::PrivateStruct;
            "#;

        rt(
            input,
            r#"
                pub struct PrivateStruct;
            "#,
        );
        rt_private(
            input,
            r#"
                mod private {
                    pub struct PrivateStruct;
                }

                pub struct PrivateStruct;
            "#,
        );
    }

    #[test]
    fn test_render_type_alias_with_bounds() {
        rt_idemp(
            r#"
            pub trait Trait<T> {
                fn as_ref(&self) -> &T;
            }

            pub type Alias<T> = dyn Trait<T> + Send + 'static;

            pub fn use_alias<T: 'static>(value: Box<Alias<T>>) -> &'static T { }
            "#,
        );
    }

    #[test]
    fn test_render_type_alias() {
        rt_idemp(
            r#"
                /// A simple type alias
                pub type SimpleAlias = Vec<String>;

                /// A type alias with generics
                pub type GenericAlias<T> = Result<T, std::io::Error>;

                /// A type alias with generics and where clause
                pub type ComplexAlias<T, U> where T: Clone, U: Default = Result<Vec<(T, U)>, Box<dyn std::error::Error>>;
            "#,
        );
    }

    #[test]
    fn test_reserved_word() {
        rt_idemp(
            r#"
                pub fn r#try() { }
            "#,
        );
    }

    #[test]
    fn test_render_macro() {
        let source = r#"
            /// A simple macro for creating a vector
            #[macro_export]
            macro_rules! myvec {
                ( $( $x:expr ),* ) => {
                    {
                        let mut temp_vec = Vec::new();
                        $(
                            temp_vec.push($x);
                        )*
                        temp_vec
                    }
                };
            }

            // A private macro
            macro_rules! private_macro {
                ($x:expr) => {
                    $x + 1
                };
            }
        "#;

        let expected_output = r#"
            /// A simple macro for creating a vector
            #[macro_export]
            macro_rules! myvec {
                ( $( $x:expr ),* ) => { ... };
            }
        "#;

        rt(source, expected_output);
    }

    #[test]
    fn test_render_macro_in_module() {
        let source = r#"
            pub mod macros {
                /// A public macro in a module
                #[macro_export]
                macro_rules! public_macro {
                    ($x:expr) => {
                        $x * 2
                    };
                }

                // A private macro in a module
                macro_rules! private_macro {
                    ($x:expr) => {
                        $x + 1
                    };
                }
            }
        "#;

        // #[macro_export] pulls the macro to the top of the crate
        let expected_output = r#"
            pub mod macros {
            }
            /// A public macro in a module
            #[macro_export]
            macro_rules! public_macro {
                ($x:expr) => { ... };
            }
        "#;

        rt(source, expected_output);
    }

    #[test]
    fn test_render_proc_macro() {
        let source = r#"
            extern crate proc_macro;

            use proc_macro::TokenStream;

            /// Expands to the function `answer` that returns `42`.
            #[proc_macro]
            pub fn make_answer(_input: TokenStream) -> TokenStream {
                "fn answer() -> u32 { 42 }".parse().unwrap()
            }

            /// Derives the HelloMacro trait for the input type.
            #[proc_macro_derive(HelloMacro)]
            pub fn hello_macro_derive(input: TokenStream) -> TokenStream {
                // Implementation here
                input
            }

            /// Attribute macro for routing.
            #[proc_macro_attribute]
            pub fn route(attr: TokenStream, item: TokenStream) -> TokenStream {
                // Implementation here
                item
            }
        "#;

        let expected_output = r#"
            /// Expands to the function `answer` that returns `42`.
            #[proc_macro]
            pub fn make_answer(input: proc_macro::TokenStream) -> proc_macro::TokenStream {}

            /// Derives the HelloMacro trait for the input type.
            #[proc_macro_derive(HelloMacro)]
            pub fn HelloMacro(input: proc_macro::TokenStream) -> proc_macro::TokenStream {}

            /// Attribute macro for routing.
            #[proc_macro_attribute]
            pub fn route(attr: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {}
        "#;

        rt_procmacro(source, expected_output);
    }

    #[test]
    fn test_render_proc_macro_with_attributes() {
        let source = r#"
            extern crate proc_macro;
            use proc_macro::TokenStream;

            /// A derive macro for generating Debug implementations.
            #[proc_macro_derive(MyDebug, attributes(debug_format))]
            pub fn my_debug(input: TokenStream) -> TokenStream {}

            /// An attribute macro for timing function execution.
            #[proc_macro_attribute]
            pub fn debug_format(attr: TokenStream, item: TokenStream) -> TokenStream {}
        "#;

        let expected_output = r#"
            /// A derive macro for generating Debug implementations.
            #[proc_macro_derive(MyDebug, attributes(debug_format))]
            pub fn MyDebug(input: proc_macro::TokenStream) -> proc_macro::TokenStream {}

            /// An attribute macro for timing function execution.
            #[proc_macro_attribute]
            pub fn debug_format(attr: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {}
        "#;

        rt_procmacro(source, expected_output);
    }

    gen_tests! {
        plain_struct, {
            idemp {
                empty: r#"
                    pub struct EmptyStruct {}
                "#
            }
            idemp {
                basic: r#"
                    pub struct BasicStruct {
                        pub field1: i32,
                        field2: String,
                    }
                "#
            }
            idemp {
                generic: r#"
                    pub struct GenericStruct<T, U> {
                        pub field1: T,
                        field2: U,
                    }
                "#
            }
            idemp {
                with_lifetime: r#"
                    pub struct LifetimeStruct<'a> {
                        field: &'a str,
                    }
                "#
            }
            idemp {
                with_lifetime_and_generic: r#"
                    pub struct MixedStruct<'a, T> {
                        reference: &'a str,
                        value: T,
                    }
                "#
            }
            idemp {
                with_where_clause: r#"
                    pub struct WhereStruct<T, U>
                    where
                        T: Clone,
                        U: Default,
                    {
                        pub field1: T,
                        field2: U,
                    }
                "#
            }
            rt {
                with_private_fields: {
                    input: r#"
                        pub struct PrivateFieldStruct {
                            pub field1: i32,
                            field2: String,
                        }
                    "#,
                    output: r#"
                        pub struct PrivateFieldStruct {
                            pub field1: i32,
                        }
                    "#
                }
            }
            rt {
                generic_with_private_fields: {
                    input: r#"
                        pub struct GenericPrivateFieldStruct<T, U> {
                            pub field1: T,
                            field2: U,
                        }
                    "#,
                    output: r#"
                        pub struct GenericPrivateFieldStruct<T, U> {
                            pub field1: T,
                        }
                    "#
                }
            }
            rt {
                where_clause_with_private_fields: {
                    input: r#"
                        pub struct WherePrivateFieldStruct<T, U>
                        where
                            T: Clone,
                            U: Default,
                        {
                            pub field1: T,
                            field2: U,
                        }
                    "#,
                    output: r#"
                        pub struct WherePrivateFieldStruct<T, U>
                        where
                            T: Clone,
                            U: Default,
                        {
                            pub field1: T,
                        }
                    "#
                }
            }
            rt {
                only_private_fields: {
                    input: r#"
                        pub struct OnlyPrivateFieldStruct {
                            field: String,
                        }
                    "#,
                    output: r#"
                        pub struct OnlyPrivateFieldStruct {}
                    "#
                }
            }
        }
    }

    gen_tests! {
        unit_struct, {
            idemp {
                basic: r#"
                    pub struct UnitStruct;
                "#
            }
            rt {
                private: {
                    input: r#"
                        struct PrivateUnitStruct;
                    "#,
                    output: r#"
                    "#
                }
            }
        }
    }

    gen_tests! {
        tuple_struct, {
            idemp {
                basic: r#"
                    pub struct BasicTuple(i32, String);
                "#
            }
            idemp {
                with_pub_fields: r#"
                    pub struct PubFieldsTuple(pub i32, pub String);
                "#
            }
            idemp {
                mixed_visibility: r#"
                    pub struct MixedVisibilityTuple(pub i32, String, pub bool);
                "#
            }
            idemp {
                generic: r#"
                    pub struct GenericTuple<T, U>(T, U);
                "#
            }
            idemp {
                with_lifetime: r#"
                    pub struct LifetimeTuple<'a>(&'a str, String);
                "#
            }
            idemp {
                with_lifetime_and_generic: r#"
                    pub struct MixedTuple<'a, T>(&'a str, T);
                "#
            }
            idemp {
                with_where_clause: r#"
                    pub struct WhereTuple<T, U>(T, U)
                    where
                        T: Clone,
                        U: Default;
                "#
            }
            idemp {
                complex: r#"
                    pub struct ComplexTuple<'a, T, U>(&'a str, T, U, i32)
                    where
                        T: Clone,
                        U: Default + 'a;
                "#
            }
            rt {
                with_private_fields: {
                    input: r#"
                        pub struct PrivateFieldsTuple(pub i32, String, pub bool);
                    "#,
                    output: r#"
                        pub struct PrivateFieldsTuple(pub i32, _, pub bool);
                    "#
                }
            }
            rt {
                generic_with_private_fields: {
                    input: r#"
                        pub struct GenericPrivateTuple<T, U>(pub T, U);
                    "#,
                    output: r#"
                        pub struct GenericPrivateTuple<T, U>(pub T, _);
                    "#
                }
            }
            rt {
                only_private_fields: {
                    input: r#"
                        pub struct OnlyPrivateTuple(String, i32);
                    "#,
                    output: r#"
                        pub struct OnlyPrivateTuple(_, _);
                    "#
                }
            }
            rt {
                private_struct: {
                    input: r#"
                        struct PrivateTuple(i32, String);
                    "#,
                    output: r#"
                    "#
                }
            }
        }
    }

    gen_tests! {
        enums, {
            idemp {
                basic: r#"
                    pub enum BasicEnum {
                        Variant1,
                        Variant2,
                        Variant3,
                    }
                "#
            }
            idemp {
                with_tuple_variants: r#"
                    pub enum TupleEnum {
                        Variant1(i32, String),
                        Variant2(bool),
                    }
                "#
            }
            idemp {
                with_struct_variants: r#"
                    pub enum StructEnum {
                        Variant1 {
                            field1: i32,
                            field2: String,
                        },
                        Variant2 {
                            field: bool,
                        },
                    }
                "#
            }
            idemp {
                mixed_variants: r#"
                    pub enum MixedEnum {
                        Variant1,
                        Variant2(i32, String),
                        Variant3 {
                            field: bool,
                        },
                    }
                "#
            }
            idemp {
                with_discriminants: r#"
                    pub enum DiscriminantEnum {
                        Variant1 = 1,
                        Variant2 = 2,
                        Variant3 = 4,
                    }
                "#
            }
            idemp {
                generic: r#"
                    pub enum GenericEnum<T, U> {
                        Variant1(T),
                        Variant2(U),
                        Variant3(T, U),
                    }
                "#
            }
            idemp {
                with_lifetime: r#"
                    pub enum LifetimeEnum<'a> {
                        Variant1(&'a str),
                        Variant2(String),
                    }
                "#
            }
            idemp {
                with_where_clause: r#"
                    pub enum WhereEnum<T, U>
                    where
                        T: Clone,
                        U: Default,
                    {
                        Variant1(T),
                        Variant2(U),
                        Variant3 {
                            field1: T,
                            field2: U,
                        },
                    }
                "#
            }
            rt {
                private_enum: {
                    input: r#"
                        enum PrivateEnum {
                            Variant1,
                            Variant2(i32),
                        }
                    "#,
                    output: r#"
                    "#
                }
            }
            rt {
                private_variants: {
                    input: r#"
                        pub enum PrivateVariantsEnum {
                            Variant1,
                            #[doc(hidden)]
                            Variant2,
                        }
                    "#,
                    output: r#"
                        pub enum PrivateVariantsEnum {
                            Variant1,
                        }
                    "#
                }
            }
        }
    }

    gen_tests! {
        traits, {
            idemp {
                basic: r#"
                    pub trait BasicTrait {
                        fn method(&self);
                        fn default_method(&self) {
                        }
                    }
                "#
            }
            idemp {
                with_associated_types: r#"
                    pub trait TraitWithAssocTypes {
                        type Item;
                        type Container<T>;
                        fn get_item(&self) -> Self::Item;
                    }
                "#
            }
            idemp {
                with_associated_consts: r#"
                    pub trait TraitWithAssocConsts {
                        const CONSTANT: i32;
                        const DEFAULT_CONSTANT: bool = true;
                    }
                "#
            }
            idemp {
                generic: r#"
                    pub trait GenericTrait<T, U> {
                        fn process(&self, t: T) -> U;
                    }
                "#
            }
            idemp {
                with_lifetime: r#"
                    pub trait LifetimeTrait<'a> {
                        fn process(&self, data: &'a str) -> &'a str;
                    }
                "#
            }
            idemp {
                with_supertraits: r#"
                    pub trait SuperTrait: std::fmt::Debug + Clone {
                        fn super_method(&self);
                    }
                "#
            }
            idemp {
                with_where_clause: r#"
                    pub trait WhereTraitMulti<T, U>
                    where
                        T: Clone,
                        U: Default,
                    {
                        fn process(&self, t: T, u: U);
                    }
                "#
            }
            idemp {
                unsafe_trait: r#"
                    pub unsafe trait UnsafeTrait {
                        unsafe fn unsafe_method(&self);
                    }
                "#
            }
            idemp {
                with_associated_type_bounds: r#"
                    pub trait BoundedAssocType {
                        type Item: Clone + 'static;
                        fn get_item(&self) -> Self::Item;
                    }
                "#
            }
            idemp {
                with_self_type: r#"
                    pub trait WithSelfType {
                        fn as_ref(&self) -> &Self;
                        fn into_owned(self) -> Self;
                    }
                "#
            }
            rt {
                private_items: {
                    input: r#"
                        pub trait TraitWithPrivateItems {
                            fn public_method(&self);
                            #[doc(hidden)]
                            fn private_method(&self);
                            type PublicType;
                            #[doc(hidden)]
                            type PrivateType;
                        }
                    "#,
                    output: r#"
                        pub trait TraitWithPrivateItems {
                            fn public_method(&self);
                            type PublicType;
                        }
                    "#
                }
            }
            rt {
                private_trait: {
                    input: r#"
                        trait PrivateTrait {
                            fn method(&self);
                        }
                    "#,
                    output: r#"
                    "#
                }
            }
        }
    }

    gen_tests! {
        modules, {
            idemp {
                basic: r#"
                    pub mod basic_module {
                        pub fn public_function() {}
                        fn private_function() {}
                    }
                "#
            }
            idemp {
                nested: r#"
                    pub mod outer {
                        pub mod inner {
                            pub fn nested_function() {}
                        }
                        pub fn outer_function() {}
                    }
                "#
            }
            idemp {
                with_structs_and_enums: r#"
                    pub mod types {
                        pub struct PublicStruct {
                            pub field: i32,
                        }
                        
                        struct PrivateStruct {
                            field: i32,
                        }
                        
                        pub enum PublicEnum {
                            Variant1,
                            Variant2,
                        }
                        
                        enum PrivateEnum {
                            Variant1,
                            Variant2,
                        }
                    }
                "#
            }
            idemp {
                with_traits: r#"
                    pub mod traits {
                        pub trait PublicTrait {
                            fn public_method(&self);
                        }
                        
                        trait PrivateTrait {
                            fn private_method(&self);
                        }
                    }
                "#
            }
            idemp {
                with_constants: r#"
                    pub mod constants {
                        pub const PUBLIC_CONSTANT: i32 = 42;
                        const PRIVATE_CONSTANT: i32 = 7;
                    }
                "#
            }
            idemp {
                with_type_aliases: r#"
                    pub mod aliases {
                        pub type PublicAlias = Vec<String>;
                        type PrivateAlias = std::collections::HashMap<i32, String>;
                    }
                "#
            }
            idemp {
                with_doc_comments_inner: r#"
                    pub mod documented {
                        //! This is an inner module-level doc comment
                        
                        /// This is a documented function
                        pub fn documented_function() {}
                    }
                "#
            }
            rt {
                module_with_inline_imports: {
                    input: r#"
                        mod private_module {
                            pub struct PrivateStruct1;
                            pub struct PrivateStruct2;
                            struct NonPublicStruct;
                        }

                        pub mod public_module {
                            pub struct PublicStruct1;
                            pub struct PublicStruct2;
                            pub use super::private_module::PrivateStruct1;
                            pub use super::private_module::PrivateStruct2;
                        }

                        pub use self::public_module::PublicStruct1;
                        pub use self::public_module::PublicStruct2;
                        pub use self::public_module::PrivateStruct1;
                        pub use self::public_module::PrivateStruct2;
                    "#,
                    output: r#"
                        pub mod public_module {
                            pub struct PublicStruct1;
                            pub struct PublicStruct2;
                            pub struct PrivateStruct1;
                            pub struct PrivateStruct2;
                        }
                        pub struct PublicStruct1;
                        pub struct PublicStruct2;
                        pub struct PrivateStruct1;
                        pub struct PrivateStruct2;
                    "#
                }
            }
            rt {
                module_with_glob_imports: {
                    input: r#"
                        mod private_module {
                            pub struct PrivateStruct1;
                            pub struct PrivateStruct2;
                            struct NonPublicStruct;
                        }

                        pub mod public_module {
                            pub struct PublicStruct1;
                            pub struct PublicStruct2;
                            pub use super::private_module::*;
                        }

                        pub use self::public_module::*;
                    "#,
                    output: r#"
                        pub mod public_module {
                            pub struct PublicStruct1;
                            pub struct PublicStruct2;
                            pub struct PrivateStruct1;
                            pub struct PrivateStruct2;
                        }
                        pub struct PublicStruct1;
                        pub struct PublicStruct2;
                        pub struct PrivateStruct1;
                        pub struct PrivateStruct2;
                    "#
                }
            }
            rt {
                with_doc_comments_outer: {
                    input: r#"
                        /// This is a documented module, with outer comments
                        pub mod documented {
                            
                            /// This is a documented function
                            pub fn documented_function() {}
                        }
                    "#,
                    output: r#"
                        pub mod documented {
                            //! This is a documented module, with outer comments
                            
                            /// This is a documented function
                            pub fn documented_function() {}
                        }
                    "#
                }
            }
            rt {
                with_multi_doc_comments: {
                    input: r#"
                        /// This is a documented module, with duplicate comments
                        pub mod documented {
                            //! This is a module-level doc comment
                            
                            /// This is a documented function
                            pub fn documented_function() {}
                        }
                    "#,
                    output: r#"
                        pub mod documented {
                            //! This is a documented module, with duplicate comments
                            //! This is a module-level doc comment
                            
                            /// This is a documented function
                            pub fn documented_function() {}
                        }
                    "#
                }
            }
            rt {
                with_use_statements: {
                    input: r#"
                        pub mod use_module {
                            use std::collections::HashMap;
                            pub use std::vec::Vec;
                            
                            pub fn use_hash_map() -> HashMap<String, i32> {
                                HashMap::new()
                            }
                        }
                    "#,
                    output: r#"
                        pub mod use_module {
                            pub use std::vec::Vec;
                            
                            pub fn use_hash_map() -> std::collections::HashMap<String, i32> { }
                        }
                    "#
                }
            }
            rt {
                private_module: {
                    input: r#"
                        mod private_module {
                            pub fn function_in_private_module() {}
                        }
                    "#,
                    output: r#"
                    "#
                }
            }
            rt {
                mixed_visibility: {
                    input: r#"
                        pub mod mixed {
                            pub fn public_function() {}
                            fn private_function() {}
                            pub struct PublicStruct;
                            struct PrivateStruct;
                        }
                    "#,
                    output: r#"
                        pub mod mixed {
                            pub fn public_function() {}
                            pub struct PublicStruct;
                        }
                    "#
                }
            }
            rt {
                re_exports: {
                    input: r#"
                        mod private {
                            pub struct ReExported;
                        }
                        
                        pub mod public {
                            pub use super::private::ReExported;
                        }
                    "#,
                    output: r#"
                        pub mod public {
                            pub struct ReExported;
                        }
                    "#
                }
            }
            rt {
                private_and_public_imports: {
                    input: r#"
                        pub mod import_visibility {
                            use std::collections::HashMap;
                            pub use std::vec::Vec;
                            use std::fmt::Debug;
                            pub use std::fmt::Display;

                            pub fn public_vec() -> Vec<i32> {
                                Vec::new()
                            }

                            fn private_hash_map() -> HashMap<String, i32> {
                                HashMap::new()
                            }
                        }
                    "#,
                    output: r#"
                        pub mod import_visibility {
                            pub use std::vec::Vec;
                            pub use std::fmt::Display;

                            pub fn public_vec() -> Vec<i32> { }
                        }
                    "#
                }
            }
            rt {
                re_exports_with_glob: {
                    input: r#"
                        mod private {
                            pub struct ReExported1;
                            pub struct ReExported2;
                        }

                        pub mod public {
                            pub use super::private::*;
                            pub use std::collections::HashMap;
                        }
                    "#,
                    output: r#"
                        pub mod public {
                            pub use std::collections::HashMap;
                            pub struct ReExported1;
                            pub struct ReExported2;
                        }
                    "#
                }
            }
            rt {
                nested_re_exports: {
                    input: r#"
                        mod level1 {
                            pub mod level2 {
                                pub struct DeepStruct;
                            }
                        }

                        pub mod re_export {
                            pub use super::level1::level2::*;
                        }

                        pub use re_export::DeepStruct;
                    "#,
                    output: r#"
                        pub mod re_export {
                            pub struct DeepStruct;
                        }

                        pub struct DeepStruct;
                    "#
                }
            }
        }
    }

    gen_tests! {
        impl_tests, {
            idemp {
                basic: r#"
                    struct BasicStruct;
                    
                    impl BasicStruct {
                        pub fn new() -> Self {}
                        
                        pub fn public_method(&self) {}
                        
                        fn private_method(&self) {}
                    }
                "#
            }
            idemp {
                trait_impl: r#"
                    trait SomeTrait {
                        fn trait_method(&self);
                    }
                    
                    struct TraitStruct;
                    
                    impl SomeTrait for TraitStruct {
                        fn trait_method(&self) {}
                    }
                "#
            }
            idemp {
                generic_impl: r#"
                    struct GenericStruct<T>(T);
                    
                    impl<T> GenericStruct<T> {
                        pub fn new(value: T) -> Self {}
                    }
                "#
            }
            idemp {
                impl_with_where_clause: r#"
                    struct WhereStruct<T>(T);
                    
                    impl<T> WhereStruct<T>
                    where
                        T: Clone,
                    {
                        pub fn cloned(&self) -> Self {}
                    }
                "#
            }
            idemp {
                impl_for_generic_trait: r#"
                    pub trait GenericTrait<T> {
                        fn generic_method(&self, value: T);
                    }
                    
                    struct GenericTraitStruct;
                    
                    impl<U> GenericTrait<U> for GenericTraitStruct {
                        fn generic_method(&self, value: U) {}
                    }
                "#
            }
            idemp {
                associated_types_impl: r#"
                    struct AssocTypeStruct;
                    
                    impl TraitWithAssocType for AssocTypeStruct {
                        type Item = i32;
                        fn get_item(&self) -> Self::Item {
                        }
                    }

                    trait TraitWithAssocType {
                        type Item;
                        fn get_item(&self) -> Self::Item;
                    }
                "#
            }
            idemp {
                assoicated_type_bounds: r#"
                    struct BoundedAssocTypeStruct;
                    
                    impl BoundedAssocType for BoundedAssocTypeStruct {
                        type Item = i32;
                        fn get_item(&self) -> Self::Item {
                        }
                    }

                    trait BoundedAssocType {
                        type Item: Clone + 'static;
                        fn get_item(&self) -> Self::Item;
                    }
                "#
            }
            idemp {
                default_impl: r#"
                    trait DefaultTrait {
                        fn default_method(&self) { }
                    }
                    
                    struct DefaultImpl;
                    
                    impl DefaultTrait for DefaultImpl {}
                "#
            }
            idemp {
                impl_with_const_fn: r#"
                    struct ConstStruct;
                    
                    impl ConstStruct {
                        pub const fn const_method(&self) -> i32 { }
                    }
                "#
            }
            idemp {
                impl_with_async_fn: r#"
                    struct AsyncStruct;
                    
                    impl AsyncStruct {
                        pub async fn async_method(&self) {}
                    }
                "#
            }
            idemp {
                deserialize: r#"
                pub trait Deserialize<'de>: Sized {
                    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
                    where
                        D: Deserializer<'de>;
                }

                pub trait Deserializer<'de>: Sized {
                    type Error;
                }

                pub struct Message;

                impl<'de> Deserialize<'de> for Message {
                    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
                    where
                        D: Deserializer<'de>
                    {
                    }
                }
                "#
            }
            // FIXME: This appears to be a bug in rustdoc - unsafe is not set on the unsafe impl block.
            rt {
                unsafe_impl: {
                    input: r#"
                        pub unsafe trait UnsafeTrait {
                            unsafe fn unsafe_method(&self);
                        }

                        pub struct UnsafeStruct;

                        unsafe impl UnsafeTrait for UnsafeStruct {
                            unsafe fn unsafe_method(&self) {}
                        }
                    "#,
                    output: r#"
                        pub unsafe trait UnsafeTrait {
                            unsafe fn unsafe_method(&self);
                        }

                        pub struct UnsafeStruct;

                        impl UnsafeTrait for UnsafeStruct {
                            unsafe fn unsafe_method(&self) {}
                        }
                    "#
                }
            }
            rt {
                private_impl: {
                    input: r#"
                        pub struct PublicStruct;
                        
                        impl PublicStruct {
                            pub fn public_method(&self) {}
                            fn private_method(&self) {}
                        }
                    "#,
                    output: r#"
                        pub struct PublicStruct;
                        
                        impl PublicStruct {
                            pub fn public_method(&self) {}
                        }
                    "#
                }
            }
            rt {
                private_trait_impl: {
                    input: r#"
                        trait PrivateTrait {
                            fn trait_method(&self);
                        }
                        
                        pub struct PublicStruct;
                        
                        impl PrivateTrait for PublicStruct {
                            fn trait_method(&self) {}
                        }
                    "#,
                    output: r#"
                        pub struct PublicStruct;
                    "#
                }
            }
            rt {
                blanket_impl_disabled: {
                    input: r#"
                        pub trait SomeTrait {
                            fn trait_method(&self);
                        }
                        
                        impl<T: Clone> SomeTrait for T {
                            fn trait_method(&self) {}
                        }
                    "#,
                    output: r#"
                        pub trait SomeTrait {
                            fn trait_method(&self);
                        }
                    "#
                }
            }
            rt_custom {
                blanket_impl_enabled: {
                    renderer: Renderer::default().with_blanket_impls(true),
                    input: r#"
                        pub trait MyTrait {
                            fn trait_method(&self);
                        }

                        impl<T: Clone> MyTrait for T {
                            fn trait_method(&self) {}
                        }

                        pub struct MyStruct;

                        impl Clone for MyStruct {
                            fn clone(&self) -> Self {
                                MyStruct
                            }
                        }
                    "#,
                    output: r#"
                        pub trait MyTrait {
                            fn trait_method(&self);
                        }

                        pub struct MyStruct;

                        impl<T> MyTrait for MyStruct
                        where
                            T: Clone,
                        {
                            fn trait_method(&self) {}
                        }

                        impl Clone for MyStruct {
                            fn clone(&self) -> Self {}
                        }
                    "#
                }
            }
        }
    }

    gen_tests! {
        functions, {
            idemp {
                basic: r#"
                    pub fn basic_function() {}
                "#
            }
            idemp {
                with_args: r#"
                    pub fn function_with_args(x: i32, y: String) {}
                "#
            }
            idemp {
                with_return: r#"
                    pub fn function_with_return() -> i32 {
                    }
                "#
            }
            idemp {
                generic: r#"
                    pub fn generic_function<T>(value: T) -> T {
                    }
                "#
            }
            idemp {
                with_lifetime: r#"
                    pub fn lifetime_function<'a>(x: &'a str) -> &'a str {
                    }
                "#
            }
            idemp {
                with_where_clause: r#"
                    pub fn where_function<T>(value: T) -> T
                    where
                        T: Clone,
                    {
                    }
                "#
            }
            idemp {
                async_function: r#"
                    pub async fn async_function() {}
                "#
            }
            idemp {
                const_function: r#"
                    pub const fn const_function() -> i32 {
                    }
                "#
            }
            idemp {
                unsafe_function: r#"
                    pub unsafe fn unsafe_function() {}
                "#
            }
            idemp {
                complex: r#"
                    pub async unsafe fn complex_function<'a, T, U>(x: &'a T, y: U) -> Result<T, U>
                    where
                        T: Clone + Send + 'a,
                        U: std::fmt::Debug,
                    {
                    }
                "#
            }
            idemp {
                function_pointer: r#"
                    pub fn function_with_fn_pointer(f: fn(arg1: i32, arg2: String) -> bool) {
                    }
                "#
            }
            idemp {
                hrtb: r#"
                    pub fn hrtb_function<F>(f: F)
                    where
                        for<'a> F: Fn(&'a str) -> bool,
                    {
                    }
                "#
            }
            idemp {
                dyn_trait_arg: r#"
                    pub fn function_with_dyn_trait(arg: &dyn std::fmt::Debug) {}
                "#
            }
            idemp {
                multiple_dyn_trait_args: r#"
                    pub fn function_with_multiple_dyn_traits(
                        arg1: &dyn std::fmt::Debug,
                        arg2: Box<dyn std::fmt::Display>,
                    ) {}
                "#
            }
            idemp {
                dyn_trait_with_lifetime: r#"
                    pub fn function_with_dyn_trait_lifetime<'a>(arg: &'a dyn std::fmt::Debug) {}
                "#
            }
            idemp {
                dyn_trait_return: r#"
                    pub fn function_returning_dyn_trait() -> Box<dyn std::fmt::Debug> { }
                "#
            }
            idemp {
                dyn_trait_parens: r#"
                    pub fn myfn() -> &'static (dyn std::any::Any + 'static) { }
                "#
            }
            idemp {
                dyn_trait_with_associated_type: r#"
                    pub trait Iterator {
                        type Item;
                        fn next(&mut self) -> Option<Self::Item>;
                    }
                    pub fn function_with_dyn_iterator(iter: &mut dyn Iterator<Item = i32>) {}
                "#
            }
            rt {
                private_function: {
                    input: r#"
                        fn private_function() {}
                    "#,
                    output: r#"
                    "#
                }
            }
            rt {
                with_doc_comments: {
                    input: r#"
                        /// This is a documented function.
                        /// It has multiple lines of documentation.
                        pub fn documented_function() {}
                    "#,
                    output: r#"
                        /// This is a documented function.
                        /// It has multiple lines of documentation.
                        pub fn documented_function() {}
                    "#
                }
            }
            rt {
               with_attributes: {
                    input: r#"
                        #[inline]
                        #[cold]
                        pub fn function_with_attributes() {}
                    "#,
                    output: r#"
                        pub fn function_with_attributes() {}
                    "#
                }
            }
            rt_custom {
                render_private: {
                    renderer: Renderer::default().with_private_items(true),
                    input: r#"
                        fn private_function() {}
                        pub fn public_function() {}
                    "#,
                    output: r#"
                        fn private_function() {}
                        pub fn public_function() {}
                    "#
                }
            }
        }
    }
}
