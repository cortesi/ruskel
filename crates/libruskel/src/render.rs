use rust_format::{Config, Formatter, RustFmt};
use rustdoc_types::{
    Crate, Id, Impl, Item, ItemEnum, MacroKind, StructKind, VariantKind, Visibility,
};

use crate::crateutils::*;
use crate::error::{Result, RuskelError};

// List of traits that we want to render as a derive inline, above a struct declaration
const DERIVE_TRAITS: &[&str] = &[
    "Clone",
    "Copy",
    "Debug",
    "Default",
    "Display",
    "Eq",
    "Error",
    "FromStr",
    "Hash",
    "Ord",
    "PartialEq",
    "PartialOrd",
    "Send",
    "StructuralPartialEq",
    "Sync",
    // These are not built-in but are "well known" enough to treat specially
    "Serialize",
    "Deserialize",
];

fn must_get<'a>(crate_data: &'a Crate, id: &Id) -> &'a Item {
    crate_data.index.get(id).unwrap()
}

fn ppush(path_prefix: &str, name: &str) -> String {
    if path_prefix.is_empty() {
        name.to_string()
    } else {
        format!("{path_prefix}::{name}")
    }
}

#[derive(Debug, PartialEq)]
enum FilterMatch {
    Hit,
    Prefix,
    Suffix,
    Miss,
}

pub struct Renderer {
    formatter: RustFmt,
    render_auto_impls: bool,
    render_private_items: bool,
    render_blanket_impls: bool,
    /// The filter is a path BELOW the outermost module.
    filter: String,
}

struct RenderState<'a, 'b> {
    config: &'a Renderer,
    crate_data: &'b Crate,
    filter_matched: bool,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    pub fn new() -> Self {
        let config = Config::new_str().option("brace_style", "PreferSameLine");
        Self {
            formatter: RustFmt::from_config(config),
            render_auto_impls: false,
            render_private_items: false,
            render_blanket_impls: false,
            filter: String::new(),
        }
    }

    /// Apply a filter to output. The filter is a path BELOW the outermost module.
    pub fn with_filter(mut self, filter: &str) -> Self {
        self.filter = filter.to_string();
        self
    }

    /// Render impl blocks for traits implemented for all types?
    pub fn with_blanket_impls(mut self, render_blanket_impls: bool) -> Self {
        self.render_blanket_impls = render_blanket_impls;
        self
    }

    /// Render impl blocks for auto traits like Send and Sync?
    pub fn with_auto_impls(mut self, render_auto_impls: bool) -> Self {
        self.render_auto_impls = render_auto_impls;
        self
    }

    /// Render private items?
    pub fn with_private_items(mut self, render_private_items: bool) -> Self {
        self.render_private_items = render_private_items;
        self
    }

    pub fn render(&self, crate_data: &Crate) -> Result<String> {
        let mut state = RenderState {
            config: self,
            filter_matched: false,
            crate_data,
        };
        state.render()
    }
}

impl RenderState<'_, '_> {
    pub fn render(&mut self) -> Result<String> {
        // The root item is always a module
        let output = self.render_item("", must_get(self.crate_data, &self.crate_data.root), false);

        if !self.config.filter.is_empty() && !self.filter_matched {
            return Err(RuskelError::FilterNotMatched(self.config.filter.clone()));
        }

        Ok(self.config.formatter.format_str(&output)?)
    }

    fn is_visible(&self, item: &Item) -> bool {
        self.config.render_private_items || matches!(item.visibility, Visibility::Public)
    }

    /// Should an impl be rendered in full?
    fn should_render_impl(&self, impl_: &Impl) -> bool {
        if impl_.is_synthetic && !self.config.render_auto_impls {
            return false;
        }

        if DERIVE_TRAITS.contains(&impl_.trait_.as_ref().map_or("", |t| t.path.as_str())) {
            return false;
        }

        let is_blanket = impl_.blanket_impl.is_some();
        if is_blanket && !self.config.render_blanket_impls {
            return false;
        }

        // if !self.config.render_auto_impls {
        //     if let Some(trait_path) = &impl_.trait_ {
        //         let trait_name = trait_path
        //             .name
        //             .split("::")
        //             .last()
        //             .unwrap_or(&trait_path.name);
        //         if FILTERED_AUTO_TRAITS.contains(&trait_name) && is_blanket {
        //             return false;
        //         }
        //     }
        // }

        true
    }

    /// Should we filter this item? If true, the item should not be rendered.
    fn should_filter(&mut self, path_prefix: &str, item: &Item) -> bool {
        // We never filter the root module - filters operate under the root.
        if item.id == self.crate_data.root {
            return false;
        }

        if self.config.filter.is_empty() {
            return false;
        }
        match self.filter_match(path_prefix, item) {
            FilterMatch::Hit => {
                self.filter_matched = true;
                false
            }
            FilterMatch::Prefix | FilterMatch::Suffix => false,
            FilterMatch::Miss => true,
        }
    }

    /// Does this item match the filter?
    fn filter_match(&self, path_prefix: &str, item: &Item) -> FilterMatch {
        let item_path = if let Some(name) = &item.name {
            ppush(path_prefix, name)
        } else {
            return FilterMatch::Prefix;
        };

        let filter_components: Vec<&str> = self.config.filter.split("::").collect();
        let item_components: Vec<&str> = item_path.split("::").skip(1).collect();

        if filter_components == item_components {
            FilterMatch::Hit
        } else if filter_components.starts_with(&item_components) {
            FilterMatch::Prefix
        } else if item_components.starts_with(&filter_components) {
            FilterMatch::Suffix
        } else {
            FilterMatch::Miss
        }
    }

    fn should_module_doc(&self, path_prefix: &str, item: &Item) -> bool {
        if self.config.filter.is_empty() {
            return true;
        }
        matches!(
            self.filter_match(path_prefix, item),
            FilterMatch::Hit | FilterMatch::Suffix
        )
    }

    fn render_item(&mut self, path_prefix: &str, item: &Item, force_private: bool) -> String {
        if self.should_filter(path_prefix, item) {
            return String::new();
        }

        let output = match &item.inner {
            ItemEnum::Module(_) => self.render_module(path_prefix, item),
            ItemEnum::Struct(_) => self.render_struct(path_prefix, item),
            ItemEnum::Enum(_) => self.render_enum(item),
            ItemEnum::Trait(_) => self.render_trait(item),
            ItemEnum::Use(_) => self.render_use(path_prefix, item),
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
                    output.push_str(&format!("#[proc_macro_derive({fn_name})]\n"));
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

        output.push_str(&format!("pub fn {fn_name}({args}) -> {return_type} {{}}\n"));

        output
    }

    fn render_macro(&self, item: &Item) -> String {
        let mut output = docs(item);

        let macro_def = extract_item!(item, ItemEnum::Macro);
        // Add #[macro_export] for public macros
        output.push_str("#[macro_export]\n");
        output.push_str(&format!("{macro_def}\n"));

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

    fn render_use(&mut self, path_prefix: &str, item: &Item) -> String {
        let import = extract_item!(item, ItemEnum::Use);

        if import.is_glob {
            if let Some(source_id) = &import.id {
                if let Some(source_item) = self.crate_data.index.get(source_id) {
                    let module = extract_item!(source_item, ItemEnum::Module);
                    let mut output = String::new();
                    for item_id in &module.items {
                        if let Some(item) = self.crate_data.index.get(item_id) {
                            if self.is_visible(item) {
                                output.push_str(&self.render_item(path_prefix, item, true));
                            }
                        }
                    }
                    return output;
                }
            }
            // If we can't resolve the glob import, fall back to rendering it as-is
            return format!("pub use {}::*;\n", import.source);
        }

        if let Some(imported_item) = import
            .id
            .as_ref()
            .and_then(|id| self.crate_data.index.get(id))
        {
            return self.render_item(path_prefix, imported_item, true);
        }

        let mut output = docs(item);
        if import.name != import.source.split("::").last().unwrap_or(&import.source) {
            output.push_str(&format!("pub use {} as {};\n", import.source, import.name));
        } else {
            output.push_str(&format!("pub use {};\n", import.source));
        }

        output
    }

    fn render_impl(&mut self, path_prefix: &str, item: &Item) -> String {
        let mut output = docs(item);
        let impl_ = extract_item!(item, ItemEnum::Impl);

        if let Some(trait_) = &impl_.trait_ {
            if let Some(trait_item) = self.crate_data.index.get(&trait_.id) {
                if !self.is_visible(trait_item) {
                    return String::new();
                }
            }
        }

        let where_clause = render_where_clause(&impl_.generics);

        let trait_part = if let Some(trait_) = &impl_.trait_ {
            let trait_path = render_path(trait_);
            if !trait_path.is_empty() {
                format!("{trait_path} for ")
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
            output.push_str(&format!("\n{where_clause}"));
        }

        output.push_str(" {\n");

        let path_prefix = ppush(path_prefix, &render_type(&impl_.for_));
        for item_id in &impl_.items {
            if let Some(item) = self.crate_data.index.get(item_id) {
                let is_trait_impl = impl_.trait_.is_some();
                if is_trait_impl || self.is_visible(item) {
                    output.push_str(&self.render_impl_item(&path_prefix, item));
                }
            }
        }

        output.push_str("}\n\n");

        output
    }

    fn render_impl_item(&mut self, path_prefix: &str, item: &Item) -> String {
        if self.should_filter(path_prefix, item) {
            return String::new();
        }

        match &item.inner {
            ItemEnum::Function(_) => self.render_function(item, false),
            ItemEnum::Constant { .. } => self.render_constant(item),
            ItemEnum::AssocType { .. } => render_associated_type(item),
            ItemEnum::TypeAlias(_) => self.render_type_alias(item),
            _ => String::new(),
        }
    }

    fn render_enum(&self, item: &Item) -> String {
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
            let variant_item = must_get(self.crate_data, variant_id);
            output.push_str(&self.render_enum_variant(variant_item));
        }

        output.push_str("}\n\n");

        output
    }

    fn render_enum_variant(&self, item: &Item) -> String {
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
                            let field_item = must_get(self.crate_data, id);
                            let ty = extract_item!(field_item, ItemEnum::StructField);
                            render_type(ty)
                        })
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                output.push_str(&format!("({fields_str})"));
            }
            VariantKind::Struct { fields, .. } => {
                output.push_str(" {\n");
                for field in fields {
                    output.push_str(&self.render_struct_field(field, true));
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

    fn render_trait(&self, item: &Item) -> String {
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
            let item = must_get(self.crate_data, item_id);
            output.push_str(&self.render_trait_item(item));
        }

        output.push_str("}\n\n");

        output
    }

    fn render_trait_item(&self, item: &Item) -> String {
        match &item.inner {
            ItemEnum::Function(_) => self.render_function(item, true),
            ItemEnum::AssocConst { type_, value } => {
                let default_str = value
                    .as_ref()
                    .map(|d| format!(" = {d}"))
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
                type_,
            } => {
                let bounds_str = if !bounds.is_empty() {
                    format!(": {}", render_generic_bounds(bounds))
                } else {
                    String::new()
                };
                let generics_str = render_generics(generics);
                let default_str = type_
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

    fn render_struct(&mut self, path_prefix: &str, item: &Item) -> String {
        let mut output = docs(item);

        let struct_ = extract_item!(item, ItemEnum::Struct);

        // Collect inline traits
        let mut inline_traits = Vec::new();
        for impl_id in &struct_.impls {
            let impl_item = must_get(self.crate_data, impl_id);
            let impl_ = extract_item!(impl_item, ItemEnum::Impl);
            if impl_.is_synthetic {
                continue;
            }

            if let Some(trait_) = &impl_.trait_ {
                if let Some(name) = trait_.path.split("::").last() {
                    if DERIVE_TRAITS.contains(&name) {
                        inline_traits.push(name);
                    }
                }
            }
        }

        // Add derive attribute if we found any inline traits
        if !inline_traits.is_empty() {
            output.push_str(&format!("#[derive({})]\n", inline_traits.join(", ")));
        }

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
                            let field_item = must_get(self.crate_data, id);
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
                    output.push_str(&self.render_struct_field(field, false));
                }
                output.push_str("}\n\n");
            }
        }

        // Render impl blocks
        for impl_id in &struct_.impls {
            let impl_item = must_get(self.crate_data, impl_id);
            let impl_ = extract_item!(impl_item, ItemEnum::Impl);
            if self.should_render_impl(impl_) {
                output.push_str(&self.render_impl(path_prefix, impl_item));
            }
        }

        output
    }

    fn render_struct_field(&self, field_id: &Id, force: bool) -> String {
        let field_item = must_get(self.crate_data, field_id);
        if force || self.is_visible(field_item) {
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

    fn render_module(&mut self, path_prefix: &str, item: &Item) -> String {
        let path_prefix = ppush(path_prefix, &render_name(item));
        let mut output = format!("{}mod {} {{\n", render_vis(item), render_name(item));
        // Add module doc comment if present
        if self.should_module_doc(&path_prefix, item) {
            if let Some(docs) = &item.docs {
                for line in docs.lines() {
                    output.push_str(&format!("    //! {line}\n"));
                }
                output.push('\n');
            }
        }

        let module = extract_item!(item, ItemEnum::Module);

        for item_id in &module.items {
            let item = must_get(self.crate_data, item_id);
            output.push_str(&self.render_item(&path_prefix, item, false));
        }

        output.push_str("}\n\n");
        output
    }

    fn render_function(&self, item: &Item, is_trait_method: bool) -> String {
        let mut output = docs(item);
        let function = extract_item!(item, ItemEnum::Function);

        // Handle const, async, and unsafe keywords in the correct order
        let mut prefixes = Vec::new();
        if function.header.is_const {
            prefixes.push("const");
        }
        if function.header.is_async {
            prefixes.push("async");
        }
        if function.header.is_unsafe {
            prefixes.push("unsafe");
        }

        output.push_str(&format!(
            "{} {} fn {}{}({}){}{}",
            render_vis(item),
            prefixes.join(" "),
            render_name(item),
            render_generics(&function.generics),
            render_function_args(&function.sig),
            render_return_type(&function.sig),
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
