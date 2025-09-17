use std::collections::HashSet;

use once_cell::sync::Lazy;
use regex::Regex;
use rust_format::{Config, Formatter, RustFmt};
use rustdoc_types::{
    Crate, Id, Impl, Item, ItemEnum, MacroKind, StructKind, VariantKind, Visibility,
};

use crate::{
    crateutils::*,
    error::{Result, RuskelError},
    frontmatter::FrontmatterConfig,
    keywords::is_reserved_word,
};

/// Traits that we render via `#[derive(...)]` annotations instead of explicit impl blocks.
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

/// Reusable pattern for removing placeholder bodies from macro output.
static MACRO_PLACEHOLDER_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\}\s*\{\s*\.\.\.\s*\}\s*$").expect("valid macro fallback pattern"));

/// Retrieve an item from the crate index, panicking if it is missing.
fn must_get<'a>(crate_data: &'a Crate, id: &Id) -> &'a Item {
    crate_data.index.get(id).unwrap()
}

/// Append `name` to a path prefix using `::` separators.
fn ppush(path_prefix: &str, name: &str) -> String {
    if path_prefix.is_empty() {
        name.to_string()
    } else {
        format!("{path_prefix}::{name}")
    }
}

/// Escape reserved keywords in a path by adding raw identifier prefixes when needed.
fn escape_path(path: &str) -> String {
    path.split("::")
        .map(|segment| {
            // Some keywords like 'crate', 'self', 'super' cannot be raw identifiers
            if segment == "crate" || segment == "self" || segment == "super" || segment == "Self" {
                segment.to_string()
            } else if is_reserved_word(segment) {
                format!("r#{}", segment)
            } else {
                segment.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("::")
}

/// Classification describing how a filter string matches a path.
#[derive(Debug, PartialEq)]
enum FilterMatch {
    /// The filter exactly matches the path.
    Hit,
    /// The filter matches a prefix of the path.
    Prefix,
    /// The filter matches a suffix of the path.
    Suffix,
    /// The filter does not match the path.
    Miss,
}

/// Selection of item identifiers used when rendering subsets of a crate.
#[derive(Debug, Clone, Default)]
pub struct RenderSelection {
    /// Item identifiers that directly satisfied the search query.
    matches: HashSet<Id>,
    /// Ancestor identifiers retained to preserve module hierarchy in output.
    context: HashSet<Id>,
}

impl RenderSelection {
    /// Create a selection from explicit match and context sets.
    pub fn new(matches: HashSet<Id>, mut context: HashSet<Id>) -> Self {
        for id in &matches {
            context.insert(*id);
        }
        Self { matches, context }
    }

    /// Identifiers for items that should be fully rendered.
    pub fn matches(&self) -> &HashSet<Id> {
        &self.matches
    }

    /// Identifiers for items that should be kept to preserve hierarchy context.
    pub fn context(&self) -> &HashSet<Id> {
        &self.context
    }
}

/// Configurable renderer that turns rustdoc data into skeleton Rust source.
pub struct Renderer {
    /// Formatter used to produce tidy Rust output.
    formatter: RustFmt,
    /// Whether auto trait implementations should be included in the output.
    pub render_auto_impls: bool,
    /// Whether private items should be rendered.
    pub render_private_items: bool,
    /// Whether blanket implementations (with generics over `T`) should be rendered.
    render_blanket_impls: bool,
    /// Filter path relative to the crate root.
    filter: String,
    /// Optional selection restricting which items are rendered.
    selection: Option<RenderSelection>,
    /// Optional frontmatter configuration rendered before crate content.
    frontmatter: Option<FrontmatterConfig>,
}

/// Mutable rendering context shared across helper functions.
struct RenderState<'a, 'b> {
    /// Reference to the immutable renderer configuration.
    config: &'a Renderer,
    /// Crate metadata produced by rustdoc.
    crate_data: &'b Crate,
    /// Tracks whether any item matched the configured filter.
    filter_matched: bool,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    /// Create a renderer with default configuration.
    pub fn new() -> Self {
        let config = Config::new_str().option("brace_style", "PreferSameLine");
        Self {
            formatter: RustFmt::from_config(config),
            render_auto_impls: false,
            render_private_items: false,
            render_blanket_impls: false,
            filter: String::new(),
            selection: None,
            frontmatter: None,
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

    /// Restrict rendering to the provided selection.
    pub fn with_selection(mut self, selection: RenderSelection) -> Self {
        self.selection = Some(selection);
        self
    }

    /// Attach optional frontmatter metadata to the rendered output.
    pub fn with_frontmatter(mut self, frontmatter: FrontmatterConfig) -> Self {
        self.frontmatter = Some(frontmatter);
        self
    }

    /// Render a crate into formatted Rust source text.
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
    /// Render the crate, applying filters and formatting output.
    pub fn render(&mut self) -> Result<String> {
        // The root item is always a module
        let output = self.render_item("", must_get(self.crate_data, &self.crate_data.root), false);

        if !self.config.filter.is_empty() && !self.filter_matched {
            return Err(RuskelError::FilterNotMatched(self.config.filter.clone()));
        }

        let mut composed = String::new();
        if let Some(frontmatter) = &self.config.frontmatter
            && let Some(prefix) = frontmatter.render(
                self.config.render_private_items,
                self.config.render_auto_impls,
                self.config.render_blanket_impls,
            )
        {
            composed.push_str(&prefix);
        }
        composed.push_str(&output);

        Ok(self.config.formatter.format_str(&composed)?)
    }

    /// Return the active render selection, if any.
    fn selection(&self) -> Option<&RenderSelection> {
        self.config.selection.as_ref()
    }

    /// Determine whether the selection context includes a particular item.
    fn selection_context_contains(&self, id: &Id) -> bool {
        match self.selection() {
            Some(selection) => selection.context().contains(id),
            None => true,
        }
    }

    /// Check if an item was an explicit match in the selection.
    fn selection_matches(&self, id: &Id) -> bool {
        match self.selection() {
            Some(selection) => selection.matches().contains(id),
            None => false,
        }
    }

    /// Determine whether a child item should be rendered based on its parent and selection context.
    fn selection_allows_child(&self, parent_id: &Id, child_id: &Id) -> bool {
        if self.selection().is_none() {
            return true;
        }
        self.selection_matches(parent_id) || self.selection_context_contains(child_id)
    }

    /// Determine whether an item should be rendered based on visibility settings.
    fn is_visible(&self, item: &Item) -> bool {
        self.config.render_private_items || matches!(item.visibility, Visibility::Public)
    }

    /// Determine whether an impl block should be rendered in the output.
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

    /// Determine whether an item is filtered out by the configured path filter.
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
    /// Evaluate how the current filter matches a candidate path.
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

    /// Determine whether a module should emit a `//!` doc comment header.
    fn should_module_doc(&self, path_prefix: &str, item: &Item) -> bool {
        if self.config.filter.is_empty() {
            return true;
        }
        matches!(
            self.filter_match(path_prefix, item),
            FilterMatch::Hit | FilterMatch::Suffix
        )
    }

    /// Render an item into Rust source text.
    fn render_item(&mut self, path_prefix: &str, item: &Item, force_private: bool) -> String {
        if !self.selection_context_contains(&item.id) {
            return String::new();
        }

        if self.should_filter(path_prefix, item) {
            return String::new();
        }

        let output = match &item.inner {
            ItemEnum::Module(_) => self.render_module(path_prefix, item),
            ItemEnum::Struct(_) => self.render_struct(path_prefix, item),
            ItemEnum::Enum(_) => self.render_enum(path_prefix, item),
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

    /// Render a procedural macro definition.
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

    /// Render a macro_rules! definition.
    fn render_macro(&self, item: &Item) -> String {
        let mut output = docs(item);

        let macro_def = extract_item!(item, ItemEnum::Macro);
        // Add #[macro_export] for public macros
        output.push_str("#[macro_export]\n");

        // Handle reserved keywords in macro names
        let macro_str = macro_def.to_string();

        // Fix rustdoc's incorrect rendering of new-style macro syntax
        // rustdoc produces "} {\n    ...\n}" which is invalid syntax
        // For new-style macros, we need to remove the extra block
        let fixed_macro_str =
            if macro_str.starts_with("macro ") && !macro_str.starts_with("macro_rules!") {
                // This is a new-style declarative macro
                // Look for the problematic pattern where we have "} { ... }" at the end
                if MACRO_PLACEHOLDER_REGEX.is_match(&macro_str) {
                    // Remove the invalid "{ ... }" part, just end after the pattern
                    MACRO_PLACEHOLDER_REGEX.replace(&macro_str, "}").to_string()
                } else {
                    macro_str
                }
            } else {
                macro_str
            };

        if let Some(name_start) = fixed_macro_str.find("macro_rules!") {
            let prefix = &fixed_macro_str[..name_start + 12]; // "macro_rules!"
            let rest = &fixed_macro_str[name_start + 12..];

            // Find the macro name (skip whitespace)
            let trimmed = rest.trim_start();
            if let Some(name_end) = trimmed.find(|c: char| c.is_whitespace() || c == '{') {
                let name = &trimmed[..name_end];
                let suffix = &trimmed[name_end..];

                // Check if the name is a reserved word
                if is_reserved_word(name) {
                    output.push_str(&format!("{prefix} r#{name}{suffix}\n"));
                } else {
                    output.push_str(&fixed_macro_str);
                    output.push('\n');
                }
            } else {
                output.push_str(&fixed_macro_str);
                output.push('\n');
            }
        } else {
            output.push_str(&fixed_macro_str);
            output.push('\n');
        }

        output
    }

    /// Render a type alias with generics, bounds, and visibility.
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

    /// Render a `use` statement, applying filter rules for private modules.
    fn render_use(&mut self, path_prefix: &str, item: &Item) -> String {
        let import = extract_item!(item, ItemEnum::Use);

        if import.is_glob {
            if let Some(source_id) = &import.id
                && let Some(source_item) = self.crate_data.index.get(source_id)
            {
                let module = extract_item!(source_item, ItemEnum::Module);
                let mut output = String::new();
                for item_id in &module.items {
                    if let Some(item) = self.crate_data.index.get(item_id)
                        && self.is_visible(item)
                    {
                        output.push_str(&self.render_item(path_prefix, item, true));
                    }
                }
                return output;
            }
            // If we can't resolve the glob import, fall back to rendering it as-is
            return format!("pub use {}::*;\n", escape_path(&import.source));
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
            // Check if the alias itself needs escaping
            let escaped_name = if is_reserved_word(import.name.as_str()) {
                format!("r#{}", import.name)
            } else {
                import.name.clone()
            };
            output.push_str(&format!(
                "pub use {} as {};\n",
                escape_path(&import.source),
                escaped_name
            ));
        } else {
            output.push_str(&format!("pub use {};\n", escape_path(&import.source)));
        }

        output
    }

    /// Render an implementation block, respecting filtering rules.
    fn render_impl(&mut self, path_prefix: &str, item: &Item) -> String {
        let mut output = docs(item);
        let impl_ = extract_item!(item, ItemEnum::Impl);

        if !self.selection_context_contains(&item.id) {
            return String::new();
        }

        let selection_active = self.selection().is_some();
        let include_all_items = selection_active && self.selection_matches(&item.id);

        if let Some(trait_) = &impl_.trait_
            && let Some(trait_item) = self.crate_data.index.get(&trait_.id)
            && !self.is_visible(trait_item)
        {
            return String::new();
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
        let mut has_content = false;
        for item_id in &impl_.items {
            if let Some(item) = self.crate_data.index.get(item_id) {
                let is_trait_impl = impl_.trait_.is_some();
                if (!selection_active
                    || include_all_items
                    || self.selection_context_contains(item_id))
                    && (is_trait_impl || self.is_visible(item))
                {
                    let rendered = self.render_impl_item(&path_prefix, item, include_all_items);
                    if !rendered.is_empty() {
                        output.push_str(&rendered);
                        has_content = true;
                    }
                }
            }
        }

        if !has_content {
            return String::new();
        }

        output.push_str("}\n\n");

        output
    }

    /// Render the item inside an impl block.
    fn render_impl_item(&mut self, path_prefix: &str, item: &Item, include_all: bool) -> String {
        if !include_all && !self.selection_context_contains(&item.id) {
            return String::new();
        }

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

    /// Render an enum definition, including variants.
    fn render_enum(&mut self, path_prefix: &str, item: &Item) -> String {
        let mut output = docs(item);

        let enum_ = extract_item!(item, ItemEnum::Enum);

        if !self.selection_context_contains(&item.id) {
            return String::new();
        }

        let selection_active = self.selection().is_some();
        let include_all_variants = selection_active && self.selection_matches(&item.id);

        // Collect inline traits
        let mut inline_traits = Vec::new();
        for impl_id in &enum_.impls {
            let impl_item = must_get(self.crate_data, impl_id);
            let impl_ = extract_item!(impl_item, ItemEnum::Impl);
            if impl_.is_synthetic {
                continue;
            }

            if let Some(trait_) = &impl_.trait_
                && let Some(name) = trait_.path.split("::").last()
                && DERIVE_TRAITS.contains(&name)
            {
                inline_traits.push(name);
            }
        }

        // Add derive attribute if we found any inline traits
        if !inline_traits.is_empty() {
            output.push_str(&format!("#[derive({})]\n", inline_traits.join(", ")));
        }

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
            if !selection_active
                || include_all_variants
                || self.selection_context_contains(variant_id)
            {
                let variant_item = must_get(self.crate_data, variant_id);
                let include_variant_fields = selection_active
                    && (include_all_variants || self.selection_matches(&variant_item.id));
                let rendered = self.render_enum_variant(variant_item, include_variant_fields);
                if !rendered.is_empty() {
                    output.push_str(&rendered);
                }
            }
        }

        output.push_str("}\n\n");

        // Render impl blocks
        for impl_id in &enum_.impls {
            let impl_item = must_get(self.crate_data, impl_id);
            let impl_ = extract_item!(impl_item, ItemEnum::Impl);
            if self.should_render_impl(impl_) && self.selection_allows_child(&item.id, impl_id) {
                output.push_str(&self.render_impl(path_prefix, impl_item));
            }
        }

        output
    }

    /// Render a single enum variant.
    fn render_enum_variant(&self, item: &Item, include_all_fields: bool) -> String {
        let selection_active = self.selection().is_some();

        if selection_active && !include_all_fields && !self.selection_context_contains(&item.id) {
            return String::new();
        }

        let mut output = docs(item);

        let variant = extract_item!(item, ItemEnum::Variant);

        output.push_str(&format!("    {}", render_name(item)));

        match &variant.kind {
            VariantKind::Plain => {}
            VariantKind::Tuple(fields) => {
                let fields_str = fields
                    .iter()
                    .filter_map(|field| {
                        field.as_ref().and_then(|id| {
                            if selection_active
                                && !include_all_fields
                                && !self.selection_context_contains(id)
                            {
                                return None;
                            }
                            let field_item = must_get(self.crate_data, id);
                            let ty = extract_item!(field_item, ItemEnum::StructField);
                            Some(render_type(ty))
                        })
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                output.push_str(&format!("({fields_str})"));
            }
            VariantKind::Struct { fields, .. } => {
                output.push_str(" {\n");
                for field in fields {
                    if !selection_active
                        || include_all_fields
                        || self.selection_context_contains(field)
                    {
                        let rendered = self
                            .render_struct_field(field, include_all_fields || !selection_active);
                        if !rendered.is_empty() {
                            output.push_str(&rendered);
                        }
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

    /// Render a trait definition.
    fn render_trait(&self, item: &Item) -> String {
        let mut output = docs(item);

        let trait_ = extract_item!(item, ItemEnum::Trait);

        if !self.selection_context_contains(&item.id) {
            return String::new();
        }

        let selection_active = self.selection().is_some();
        let include_all_items = selection_active && self.selection_matches(&item.id);

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
            if !selection_active || include_all_items || self.selection_context_contains(item_id) {
                let item = must_get(self.crate_data, item_id);
                output.push_str(&self.render_trait_item(item, include_all_items));
            }
        }

        output.push_str("}\n\n");

        output
    }

    /// Render an item contained within a trait (method, associated type, etc.).
    fn render_trait_item(&self, item: &Item, include_all: bool) -> String {
        if !include_all && !self.selection_context_contains(&item.id) {
            return String::new();
        }
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

    /// Render a struct declaration and its fields.
    fn render_struct(&mut self, path_prefix: &str, item: &Item) -> String {
        let mut output = docs(item);

        let struct_ = extract_item!(item, ItemEnum::Struct);

        if !self.selection_context_contains(&item.id) {
            return String::new();
        }

        let selection_active = self.selection().is_some();
        let include_all_fields = selection_active && self.selection_matches(&item.id);

        // Collect inline traits
        let mut inline_traits = Vec::new();
        for impl_id in &struct_.impls {
            let impl_item = must_get(self.crate_data, impl_id);
            let impl_ = extract_item!(impl_item, ItemEnum::Impl);
            if impl_.is_synthetic {
                continue;
            }

            if let Some(trait_) = &impl_.trait_
                && let Some(name) = trait_.path.split("::").last()
                && DERIVE_TRAITS.contains(&name)
            {
                inline_traits.push(name);
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
                        field.as_ref().and_then(|id| {
                            if !include_all_fields && !self.selection_context_contains(id) {
                                return None;
                            }
                            let field_item = must_get(self.crate_data, id);
                            let ty = extract_item!(field_item, ItemEnum::StructField);
                            if !self.is_visible(field_item) {
                                Some("_".to_string())
                            } else {
                                Some(format!("{}{}", render_vis(field_item), render_type(ty)))
                            }
                        })
                    })
                    .collect::<Vec<_>>()
                    .join(", ");

                if include_all_fields || !fields_str.is_empty() {
                    output.push_str(&format!(
                        "{}struct {}{}({}){};\n\n",
                        render_vis(item),
                        render_name(item),
                        generics,
                        fields_str,
                        where_clause
                    ));
                }
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
                    let rendered = self.render_struct_field(field, include_all_fields);
                    if !rendered.is_empty() {
                        output.push_str(&rendered);
                    }
                }
                output.push_str("}\n\n");
            }
        }

        // Render impl blocks
        for impl_id in &struct_.impls {
            let impl_item = must_get(self.crate_data, impl_id);
            let impl_ = extract_item!(impl_item, ItemEnum::Impl);
            if self.should_render_impl(impl_) && self.selection_allows_child(&item.id, impl_id) {
                output.push_str(&self.render_impl(path_prefix, impl_item));
            }
        }

        output
    }

    /// Render a struct field, optionally forcing visibility.
    fn render_struct_field(&self, field_id: &Id, force: bool) -> String {
        let field_item = must_get(self.crate_data, field_id);

        if self.selection().is_some() && !force && !self.selection_context_contains(field_id) {
            return String::new();
        }

        if !(force || self.is_visible(field_item)) {
            return String::new();
        }

        let ty = extract_item!(field_item, ItemEnum::StructField);
        let mut out = String::new();
        out.push_str(&docs(field_item));
        out.push_str(&format!(
            "{}{}: {},\n",
            render_vis(field_item),
            render_name(field_item),
            render_type(ty)
        ));
        out
    }

    /// Render a constant definition.
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

    /// Render a module and its children.
    fn render_module(&mut self, path_prefix: &str, item: &Item) -> String {
        let path_prefix = ppush(path_prefix, &render_name(item));
        let mut output = format!("{}mod {} {{\n", render_vis(item), render_name(item));
        // Add module doc comment if present
        if self.should_module_doc(&path_prefix, item)
            && let Some(docs) = &item.docs
        {
            for line in docs.lines() {
                output.push_str(&format!("    //! {line}\n"));
            }
            output.push('\n');
        }

        let module = extract_item!(item, ItemEnum::Module);

        for item_id in &module.items {
            let item = must_get(self.crate_data, item_id);
            output.push_str(&self.render_item(&path_prefix, item, false));
        }

        output.push_str("}\n\n");
        output
    }

    /// Render a function or method signature.
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use rustdoc_types::{
        Abi, Crate, Function, FunctionHeader, FunctionSignature, Generics, Id, Impl, Item,
        ItemEnum, Module, Path, Struct, StructKind, Target, Type, Variant, VariantKind, Visibility,
    };

    use super::*;
    use crate::{
        frontmatter::{FrontmatterConfig, FrontmatterHit, FrontmatterSearch},
        search::{SearchDomain, SearchIndex, SearchOptions, build_render_selection},
    };

    fn empty_generics() -> Generics {
        Generics {
            params: Vec::new(),
            where_predicates: Vec::new(),
        }
    }

    fn default_header() -> FunctionHeader {
        FunctionHeader {
            is_const: false,
            is_unsafe: false,
            is_async: false,
            abi: Abi::Rust,
        }
    }

    fn fixture_crate() -> Crate {
        let root = Id(0);
        let widget = Id(1);
        let widget_field_id = Id(2);
        let widget_field_name = Id(3);
        let widget_impl = Id(4);
        let render_method = Id(5);
        let helper_fn = Id(6);
        let palette_enum = Id(7);
        let named_variant = Id(8);
        let named_field = Id(9);
        let unspecified_variant = Id(10);
        let widget_private_impl = Id(11);
        let private_helper_method = Id(12);

        let mut index = HashMap::new();

        index.insert(
            root,
            Item {
                id: root,
                crate_id: 0,
                name: Some("fixture".into()),
                span: None,
                visibility: Visibility::Public,
                docs: None,
                links: HashMap::new(),
                attrs: Vec::new(),
                deprecation: None,
                inner: ItemEnum::Module(Module {
                    is_crate: true,
                    items: vec![
                        widget,
                        helper_fn,
                        palette_enum,
                        widget_impl,
                        widget_private_impl,
                    ],
                    is_stripped: false,
                }),
            },
        );

        index.insert(
            widget,
            Item {
                id: widget,
                crate_id: 0,
                name: Some("Widget".into()),
                span: None,
                visibility: Visibility::Public,
                docs: None,
                links: HashMap::new(),
                attrs: Vec::new(),
                deprecation: None,
                inner: ItemEnum::Struct(Struct {
                    kind: StructKind::Plain {
                        fields: vec![widget_field_id, widget_field_name],
                        has_stripped_fields: false,
                    },
                    generics: empty_generics(),
                    impls: vec![widget_impl, widget_private_impl],
                }),
            },
        );

        index.insert(
            widget_field_id,
            Item {
                id: widget_field_id,
                crate_id: 0,
                name: Some("id".into()),
                span: None,
                visibility: Visibility::Public,
                docs: None,
                links: HashMap::new(),
                attrs: Vec::new(),
                deprecation: None,
                inner: ItemEnum::StructField(Type::Primitive("u32".into())),
            },
        );

        index.insert(
            widget_field_name,
            Item {
                id: widget_field_name,
                crate_id: 0,
                name: Some("name".into()),
                span: None,
                visibility: Visibility::Public,
                docs: None,
                links: HashMap::new(),
                attrs: Vec::new(),
                deprecation: None,
                inner: ItemEnum::StructField(Type::Generic("String".into())),
            },
        );

        index.insert(
            widget_impl,
            Item {
                id: widget_impl,
                crate_id: 0,
                name: None,
                span: None,
                visibility: Visibility::Public,
                docs: None,
                links: HashMap::new(),
                attrs: Vec::new(),
                deprecation: None,
                inner: ItemEnum::Impl(Impl {
                    is_unsafe: false,
                    generics: empty_generics(),
                    provided_trait_methods: Vec::new(),
                    trait_: None,
                    for_: Type::ResolvedPath(Path {
                        path: "Widget".into(),
                        id: widget,
                        args: None,
                    }),
                    items: vec![render_method],
                    is_negative: false,
                    is_synthetic: false,
                    blanket_impl: None,
                }),
            },
        );

        index.insert(
            widget_private_impl,
            Item {
                id: widget_private_impl,
                crate_id: 0,
                name: None,
                span: None,
                visibility: Visibility::Public,
                docs: None,
                links: HashMap::new(),
                attrs: Vec::new(),
                deprecation: None,
                inner: ItemEnum::Impl(Impl {
                    is_unsafe: false,
                    generics: empty_generics(),
                    provided_trait_methods: Vec::new(),
                    trait_: None,
                    for_: Type::ResolvedPath(Path {
                        path: "Widget".into(),
                        id: widget,
                        args: None,
                    }),
                    items: vec![private_helper_method],
                    is_negative: false,
                    is_synthetic: false,
                    blanket_impl: None,
                }),
            },
        );

        index.insert(
            render_method,
            Item {
                id: render_method,
                crate_id: 0,
                name: Some("render".into()),
                span: None,
                visibility: Visibility::Public,
                docs: Some("Render the widget".into()),
                links: HashMap::new(),
                attrs: Vec::new(),
                deprecation: None,
                inner: ItemEnum::Function(Function {
                    sig: FunctionSignature {
                        inputs: vec![(
                            "self".into(),
                            Type::BorrowedRef {
                                lifetime: None,
                                is_mutable: false,
                                type_: Box::new(Type::Generic("Self".into())),
                            },
                        )],
                        output: Some(Type::Generic("String".into())),
                        is_c_variadic: false,
                    },
                    generics: empty_generics(),
                    header: default_header(),
                    has_body: true,
                }),
            },
        );

        index.insert(
            helper_fn,
            Item {
                id: helper_fn,
                crate_id: 0,
                name: Some("helper".into()),
                span: None,
                visibility: Visibility::Public,
                docs: None,
                links: HashMap::new(),
                attrs: Vec::new(),
                deprecation: None,
                inner: ItemEnum::Function(Function {
                    sig: FunctionSignature {
                        inputs: vec![(
                            "widget".into(),
                            Type::BorrowedRef {
                                lifetime: None,
                                is_mutable: false,
                                type_: Box::new(Type::ResolvedPath(Path {
                                    path: "Widget".into(),
                                    id: widget,
                                    args: None,
                                })),
                            },
                        )],
                        output: Some(Type::ResolvedPath(Path {
                            path: "Widget".into(),
                            id: widget,
                            args: None,
                        })),
                        is_c_variadic: false,
                    },
                    generics: empty_generics(),
                    header: default_header(),
                    has_body: true,
                }),
            },
        );

        index.insert(
            private_helper_method,
            Item {
                id: private_helper_method,
                crate_id: 0,
                name: Some("internal_helper".into()),
                span: None,
                visibility: Visibility::Default,
                docs: None,
                links: HashMap::new(),
                attrs: Vec::new(),
                deprecation: None,
                inner: ItemEnum::Function(Function {
                    sig: FunctionSignature {
                        inputs: vec![(
                            "self".into(),
                            Type::BorrowedRef {
                                lifetime: None,
                                is_mutable: true,
                                type_: Box::new(Type::Generic("Self".into())),
                            },
                        )],
                        output: None,
                        is_c_variadic: false,
                    },
                    generics: empty_generics(),
                    header: default_header(),
                    has_body: true,
                }),
            },
        );

        index.insert(
            palette_enum,
            Item {
                id: palette_enum,
                crate_id: 0,
                name: Some("Palette".into()),
                span: None,
                visibility: Visibility::Public,
                docs: None,
                links: HashMap::new(),
                attrs: Vec::new(),
                deprecation: None,
                inner: ItemEnum::Enum(rustdoc_types::Enum {
                    generics: empty_generics(),
                    has_stripped_variants: false,
                    variants: vec![named_variant, unspecified_variant],
                    impls: Vec::new(),
                }),
            },
        );

        index.insert(
            named_variant,
            Item {
                id: named_variant,
                crate_id: 0,
                name: Some("Named".into()),
                span: None,
                visibility: Visibility::Public,
                docs: None,
                links: HashMap::new(),
                attrs: Vec::new(),
                deprecation: None,
                inner: ItemEnum::Variant(Variant {
                    kind: VariantKind::Struct {
                        fields: vec![named_field],
                        has_stripped_fields: false,
                    },
                    discriminant: None,
                }),
            },
        );

        index.insert(
            named_field,
            Item {
                id: named_field,
                crate_id: 0,
                name: Some("label".into()),
                span: None,
                visibility: Visibility::Public,
                docs: None,
                links: HashMap::new(),
                attrs: Vec::new(),
                deprecation: None,
                inner: ItemEnum::StructField(Type::Generic("String".into())),
            },
        );

        index.insert(
            unspecified_variant,
            Item {
                id: unspecified_variant,
                crate_id: 0,
                name: Some("Unspecified".into()),
                span: None,
                visibility: Visibility::Public,
                docs: None,
                links: HashMap::new(),
                attrs: Vec::new(),
                deprecation: None,
                inner: ItemEnum::Variant(Variant {
                    kind: VariantKind::Plain,
                    discriminant: None,
                }),
            },
        );

        Crate {
            root,
            crate_version: Some("0.1.0".into()),
            includes_private: false,
            index,
            paths: HashMap::new(),
            external_crates: HashMap::new(),
            target: Target {
                triple: "test-target".into(),
                target_features: Vec::new(),
            },
            format_version: 0,
        }
    }

    #[allow(clippy::needless_pass_by_value)]
    fn render_allowing_format_errors(renderer: Renderer, crate_data: &Crate) -> String {
        match renderer.render(crate_data) {
            Ok(output) => output,
            Err(RuskelError::Format(_)) => {
                let mut state = super::RenderState {
                    config: &renderer,
                    crate_data,
                    filter_matched: false,
                };
                let mut composed = String::new();
                if let Some(frontmatter) = &renderer.frontmatter
                    && let Some(prefix) = frontmatter.render(
                        renderer.render_private_items,
                        renderer.render_auto_impls,
                        renderer.render_blanket_impls,
                    )
                {
                    composed.push_str(&prefix);
                }
                composed.push_str(&state.render_item(
                    "",
                    super::must_get(crate_data, &crate_data.root),
                    false,
                ));
                composed
            }
            Err(err) => panic!("unexpected render failure: {err}"),
        }
    }

    fn render_with_selection(crate_data: &Crate, selection: RenderSelection) -> String {
        let renderer = Renderer::new().with_selection(selection);
        match renderer.render(crate_data) {
            Ok(output) => output,
            Err(RuskelError::Format(_)) => {
                let mut state = super::RenderState {
                    config: &renderer,
                    crate_data,
                    filter_matched: false,
                };
                state.render_item("", super::must_get(crate_data, &crate_data.root), false)
            }
            Err(err) => panic!("unexpected render failure: {err}"),
        }
    }

    #[test]
    fn selection_renders_only_matching_struct_field() {
        let crate_data = fixture_crate();
        let index = SearchIndex::build(&crate_data, false);
        let mut options = SearchOptions::new("Widget::id");
        options.domains = SearchDomain::PATHS;
        let results = index.search(&options);
        let field = results
            .into_iter()
            .find(|r| r.path_string.ends_with("Widget::id"))
            .expect("field result");
        let selection = build_render_selection(&[field]);
        let rendered = render_with_selection(&crate_data, selection);

        assert!(rendered.contains("struct Widget"));
        assert!(rendered.contains("id: u32"));
        assert!(!rendered.contains("name: String"));
        assert!(!rendered.contains("fn helper"));
    }

    #[test]
    fn selection_renders_only_matching_impl_method() {
        let crate_data = fixture_crate();
        let index = SearchIndex::build(&crate_data, false);
        let mut options = SearchOptions::new("render");
        options.domains = SearchDomain::NAMES;
        let results = index.search(&options);
        let method = results
            .into_iter()
            .find(|r| r.path_string.ends_with("Widget::render"))
            .expect("method result");
        let selection = build_render_selection(&[method]);
        let rendered = render_with_selection(&crate_data, selection);

        assert!(rendered.contains("impl"));
        assert!(rendered.contains("fn render"));
        assert!(!rendered.contains("fn helper"));
    }

    #[test]
    fn selection_renders_only_matching_enum_variant() {
        let crate_data = fixture_crate();
        let index = SearchIndex::build(&crate_data, false);
        let mut options = SearchOptions::new("Named");
        options.domains = SearchDomain::NAMES;
        let results = index.search(&options);
        let variant = results
            .into_iter()
            .find(|r| r.path_string.ends_with("Palette::Named"))
            .expect("variant result");
        let selection = build_render_selection(&[variant]);
        let rendered = render_with_selection(&crate_data, selection);

        assert!(rendered.contains("enum Palette"));
        assert!(rendered.contains("Named"));
        assert!(rendered.contains("pub label: String"));
        assert!(!rendered.contains("Unspecified"));
    }

    #[test]
    fn renderer_omits_empty_impl_blocks_when_private_items_hidden() {
        let crate_data = fixture_crate();
        let output = render_allowing_format_errors(Renderer::new(), &crate_data);

        assert!(
            !output.contains("impl Widget {}"),
            "expected renderer to omit empty impl blocks:\n{output}"
        );
    }

    #[test]
    fn renderer_keeps_impl_when_private_items_rendered() {
        let crate_data = fixture_crate();
        let output =
            render_allowing_format_errors(Renderer::new().with_private_items(true), &crate_data);

        assert!(output.contains("impl Widget {"));
        assert!(output.contains("fn render"));
        assert!(output.contains("fn internal_helper"));
    }

    #[test]
    fn frontmatter_inserts_target_visibility_and_path() {
        let crate_data = fixture_crate();
        let frontmatter = FrontmatterConfig::for_target("fixture::Widget")
            .with_filter(Some("fixture::Widget".into()));
        let output = render_allowing_format_errors(
            Renderer::new().with_frontmatter(frontmatter),
            &crate_data,
        );

        assert!(output.starts_with(
            "// Ruskel skeleton - syntactically valid Rust with implementation omitted."
        ));
        assert!(output.contains("target=fixture::Widget"));
        assert!(output.contains("path=fixture::Widget"));
        assert!(output.contains("visibility=public"));
        assert!(output.contains("auto_impls=false"));
        assert!(output.contains("blanket_impls=false"));
        assert!(!output.contains("ruskel::frontmatter"));
        assert!(!output.contains("validity:"));
    }

    #[test]
    fn frontmatter_can_be_disabled() {
        let crate_data = fixture_crate();
        let output = render_allowing_format_errors(Renderer::new(), &crate_data);

        assert!(!output.starts_with(
            "// Ruskel skeleton - syntactically valid Rust with implementation omitted."
        ));
    }

    #[test]
    fn frontmatter_lists_search_hits_with_domains() {
        let crate_data = fixture_crate();
        let hits = vec![FrontmatterHit {
            path: "fixture::Widget".into(),
            domains: SearchDomain::NAMES,
        }];
        let search_meta = FrontmatterSearch {
            query: "Widget".into(),
            domains: SearchDomain::NAMES | SearchDomain::DOCS,
            case_sensitive: false,
            hits,
        };
        let frontmatter = FrontmatterConfig::for_target("fixture")
            .with_filter(Some("fixture".into()))
            .with_search(search_meta);
        let output = Renderer::new().with_frontmatter(frontmatter);
        let output = render_allowing_format_errors(output, &crate_data);

        assert!(
            output
                .contains("// search: query=\"Widget\"; case_sensitive=false; domains=names, docs")
        );
        assert!(output.contains("// hits (1):"));
        assert!(output.contains("//   - fixture::Widget [names]"));
    }
}
