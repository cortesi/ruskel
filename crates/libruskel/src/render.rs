use rust_format::{Config, Formatter, RustFmt};
use rustdoc_types::{
    Crate, Id, Impl, Item, ItemEnum, MacroKind, StructKind, VariantKind, Visibility,
};

use crate::crateutils::*;
use crate::error::Result;

fn must_get<'a>(crate_data: &'a Crate, id: &Id) -> &'a Item {
    crate_data.index.get(id).unwrap()
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
        // The root item is always a module
        let output = self.render_item(
            "",
            must_get(crate_data, &crate_data.root),
            crate_data,
            false,
        );

        Ok(self.formatter.format_str(&output)?)
    }

    fn is_visible(&self, item: &Item) -> bool {
        self.render_private_items || matches!(item.visibility, Visibility::Public)
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

    fn should_filter(&self, module_path: &str, item: &Item) -> bool {
        if self.filter.is_empty() {
            return false;
        }
        let item_path = if module_path.is_empty() {
            render_name(item).to_string()
        } else {
            format!("{}::{}", module_path, render_name(item))
        };

        let filter_components: Vec<&str> = self.filter.split("::").collect();
        let item_components: Vec<&str> = item_path.split("::").collect();

        // If the item path matches the filter exactly, don't filter
        if item_path == self.filter {
            return false;
        }

        // If the item path is a prefix of the filter, don't filter
        if filter_components.starts_with(&item_components) {
            return false;
        }
        if item_components.starts_with(&filter_components) {
            return false;
        }

        // Otherwise, filter the item
        true
    }

    fn render_item(
        &self,
        module_path: &str,
        item: &Item,
        crate_data: &Crate,
        force_private: bool,
    ) -> String {
        if self.should_filter(module_path, item) {
            return String::new();
        }

        let output = match &item.inner {
            ItemEnum::Module(_) => self.render_module(module_path, item, crate_data),
            ItemEnum::Struct(_) => self.render_struct(item, crate_data),
            ItemEnum::Enum(_) => self.render_enum(item, crate_data),
            ItemEnum::Trait(_) => self.render_trait(item, crate_data),
            ItemEnum::Import(_) => self.render_import(module_path, item, crate_data),
            ItemEnum::Function(_) => self.render_function(item, false),
            ItemEnum::Constant { .. } => self.render_constant(item),
            ItemEnum::TypeAlias(_) => self.render_type_alias(item),
            ItemEnum::Macro(_) => self.render_macro(item),
            ItemEnum::ProcMacro(_) => self.render_proc_macro(item),
            _ => String::new(),
        };

        if !force_private && !self.is_visible(item) {
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

    fn render_import(&self, module_path: &str, item: &Item, crate_data: &Crate) -> String {
        let import = extract_item!(item, ItemEnum::Import);

        if import.glob {
            if let Some(source_id) = &import.id {
                if let Some(source_item) = crate_data.index.get(source_id) {
                    let module = extract_item!(source_item, ItemEnum::Module);
                    let mut output = String::new();
                    for item_id in &module.items {
                        if let Some(item) = crate_data.index.get(item_id) {
                            if self.is_visible(item) {
                                output.push_str(&self.render_item(
                                    module_path,
                                    item,
                                    crate_data,
                                    true,
                                ));
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
            return self.render_item(module_path, imported_item, crate_data, true);
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
                if !self.is_visible(trait_item) {
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
                if is_trait_impl || self.is_visible(item) {
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
            ItemEnum::AssocType { .. } => render_associated_type(item),
            ItemEnum::TypeAlias(_) => self.render_type_alias(item),
            _ => String::new(),
        }
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
            let variant_item = must_get(crate_data, variant_id);
            output.push_str(&self.render_enum_variant(variant_item, crate_data));
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
                            let field_item = must_get(crate_data, id);
                            let ty = extract_item!(field_item, ItemEnum::StructField);
                            render_type(ty)
                        })
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                output.push_str(&format!("({})", fields_str));
            }
            VariantKind::Struct { fields, .. } => {
                output.push_str(" {\n");
                for field in fields {
                    output.push_str(&self.render_struct_field(crate_data, field));
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
            format!(": {}", render_generic_bounds(&trait_.bounds))
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
            let item = must_get(crate_data, item_id);
            output.push_str(&self.render_trait_item(item));
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
                    format!(": {}", render_generic_bounds(bounds))
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
                            let field_item = must_get(crate_data, id);
                            let ty = extract_item!(field_item, ItemEnum::StructField);
                            if !self.is_visible(field_item) {
                                "_".to_string()
                            } else {
                                format!("{}{}", render_vis(field_item), render_type(ty))
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
            let impl_item = must_get(crate_data, impl_id);
            let impl_ = extract_item!(impl_item, ItemEnum::Impl);
            if self.should_render_impl(impl_) {
                output.push_str(&self.render_impl(impl_item, crate_data));
            }
        }

        output
    }

    fn render_struct_field(&self, crate_data: &Crate, field_id: &Id) -> String {
        let field_item = must_get(crate_data, field_id);
        if self.is_visible(field_item) {
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

    fn render_module(&self, module_path: &str, item: &Item, crate_data: &Crate) -> String {
        let module_path = if module_path.is_empty() {
            render_name(item).to_string()
        } else {
            format!("{}::{}", module_path, render_name(item))
        };
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
            let item = must_get(crate_data, item_id);
            output.push_str(&self.render_item(&module_path, item, crate_data, false));
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
}
