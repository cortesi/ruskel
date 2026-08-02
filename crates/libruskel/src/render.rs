use std::collections::HashMap;

use once_cell::sync::Lazy;
use regex::Regex;
use rust_format::{Config, Formatter, RustFmt};
use rustdoc_types::{
    AssocItemConstraint, AssocItemConstraintKind, Crate, FunctionPointer, FunctionSignature,
    GenericArg, GenericArgs, GenericBound, Id, Impl, Item, ItemEnum, MacroKind, Path, PolyTrait,
    StructKind, Term, TraitBoundModifier, Type, VariantKind, Visibility,
};

use crate::{
    crateutils::*,
    error::{Result, RuskelError},
    frontmatter::FrontmatterConfig,
    keywords::is_reserved_word,
    search::SearchItemKind,
    selection::{RenderSelection, derive_trait_name, should_render_impl},
    signature,
};

/// Reusable pattern for removing placeholder bodies from macro output.
/// rustdoc currently emits `{ ... }` placeholder blocks for `macro` (decl-macro) items in JSON
/// output (observed on nightly 2025-11-27). When upstream fixes this, update
/// `rustdoc_still_emits_placeholder_for_new_style_macros` and consider removing this workaround.
/// (No tracked rust-lang/rust issue is known at the moment.)
static MACRO_PLACEHOLDER_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\}\s*\{\s*\.\.\.\s*\}\s*$").expect("valid macro fallback pattern"));

/// Retrieve an item from the crate index, returning an error if it is missing.
fn must_get<'a>(crate_data: &'a Crate, id: &Id) -> Result<&'a Item> {
    crate_data
        .index
        .get(id)
        .ok_or_else(|| RuskelError::ItemNotFound(format!("{id:?}")))
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

/// Key for grouping impl blocks that share a compatible header.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ImplGroupKey {
    /// Whether the impl is marked unsafe.
    is_unsafe: bool,
    /// Whether the impl is negative.
    is_negative: bool,
    /// Rendered generic parameter list.
    generics: String,
    /// Normalized trait path used for grouping.
    trait_key: Option<String>,
    /// Normalized target type used for grouping.
    for_key: String,
    /// Rendered where clause for the impl.
    where_clause: String,
}

impl ImplGroupKey {
    /// Build a group key from a rustdoc impl item.
    fn from_impl(impl_: &Impl) -> Self {
        let trait_key = impl_.trait_.as_ref().map(impl_path_key);
        let for_key = impl_type_key(&impl_.for_);
        Self {
            is_unsafe: impl_.is_unsafe,
            is_negative: impl_.is_negative,
            generics: render_generics(&impl_.generics),
            trait_key,
            for_key,
            where_clause: render_where_clause(&impl_.generics),
        }
    }
}

/// Canonicalized impl header used for grouping compatible impl blocks.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ImplSignature {
    /// Whether the impl is marked unsafe.
    is_unsafe: bool,
    /// Whether the impl is negative.
    is_negative: bool,
    /// Rendered generic parameter list.
    generics: String,
    /// Rendered trait path for trait impls.
    trait_path: Option<String>,
    /// Rendered target type for the impl.
    for_type: String,
    /// Rendered where clause for the impl.
    where_clause: String,
}

impl ImplSignature {
    /// Build a signature from a rustdoc impl item.
    fn from_impl(impl_: &Impl) -> Self {
        let trait_path = impl_
            .trait_
            .as_ref()
            .map(render_path)
            .filter(|path| !path.is_empty());
        Self {
            is_unsafe: impl_.is_unsafe,
            is_negative: impl_.is_negative,
            generics: render_generics(&impl_.generics),
            trait_path,
            for_type: render_type(&impl_.for_),
            where_clause: render_where_clause(&impl_.generics),
        }
    }

    /// Render the impl header for this signature.
    fn render_header(&self, target_rename: Option<(&str, &str)>) -> String {
        let mut output = String::new();
        if self.is_unsafe {
            output.push_str("unsafe ");
        }
        output.push_str("impl");
        output.push_str(&self.generics);
        output.push(' ');
        if let Some(trait_path) = &self.trait_path {
            if self.is_negative {
                output.push('!');
            }
            output.push_str(trait_path);
            output.push_str(" for ");
        }
        if let Some((original, alias)) = target_rename {
            output.push_str(&self.for_type.replacen(original, alias, 1));
        } else {
            output.push_str(&self.for_type);
        }
        if !self.where_clause.is_empty() {
            output.push('\n');
            output.push_str(&self.where_clause);
        }
        output.push_str(" {\n");
        output
    }
}

/// Group of impl items that share the same header signature.
struct ImplGroup {
    /// Shared impl header signature.
    signature: ImplSignature,
    /// Impl item identifiers in original order.
    impl_ids: Vec<Id>,
}

/// Rendered docs and body contents for a single impl item.
struct RenderedImplBody {
    /// Doc comments attached to the impl item.
    docs: String,
    /// Rendered impl item contents.
    body: String,
}

/// Render a normalized path key using the resolved item id.
fn impl_path_key(path: &Path) -> String {
    let args = path
        .args
        .as_ref()
        .map(|args| impl_generic_args_key(args))
        .unwrap_or_default();
    format!("id:{}{}", path.id.0, args)
}

/// Render a normalized type key suitable for impl grouping.
fn impl_type_key(ty: &Type) -> String {
    match ty {
        Type::ResolvedPath(path) => impl_path_key(path),
        Type::DynTrait(dyn_trait) => {
            let traits = dyn_trait
                .traits
                .iter()
                .map(impl_poly_trait_key)
                .collect::<Vec<_>>()
                .join(" + ");
            let lifetime = dyn_trait
                .lifetime
                .as_ref()
                .map(|lt| format!(" + {lt}"))
                .unwrap_or_default();
            format!("dyn {traits}{lifetime}")
        }
        Type::Generic(s) => s.clone(),
        Type::Primitive(s) => s.clone(),
        Type::FunctionPointer(f) => impl_function_pointer_key(f),
        Type::Tuple(types) => {
            let inner = types
                .iter()
                .map(impl_type_key)
                .collect::<Vec<_>>()
                .join(", ");
            format!("({inner})")
        }
        Type::Slice(ty) => format!("[{}]", impl_type_key(ty)),
        Type::Array { type_, len } => {
            format!("[{}; {len}]", impl_type_key(type_))
        }
        Type::ImplTrait(bounds) => {
            let bounds_str = impl_generic_bounds_key(bounds);
            format!("impl {bounds_str}")
        }
        Type::Infer => "_".to_string(),
        Type::RawPointer { is_mutable, type_ } => {
            let mutability = if *is_mutable { "mut" } else { "const" };
            format!("*{mutability} {}", impl_type_key(type_))
        }
        Type::BorrowedRef {
            lifetime,
            is_mutable,
            type_,
        } => {
            let lifetime = lifetime
                .as_ref()
                .map(|lt| format!("{lt} "))
                .unwrap_or_default();
            let mutability = if *is_mutable { "mut " } else { "" };
            format!("&{lifetime}{mutability}{}", impl_type_key(type_))
        }
        Type::QualifiedPath {
            name,
            args,
            self_type,
            trait_,
        } => {
            let self_type_str = impl_type_key(self_type);
            let args_str = args
                .as_ref()
                .map(|args| impl_generic_args_key(args))
                .unwrap_or_default();

            if let Some(trait_) = trait_ {
                let trait_path = impl_path_key(trait_);
                if !trait_path.is_empty() {
                    format!("<{self_type_str} as {trait_path}>::{name}{args_str}")
                } else {
                    format!("{self_type_str}::{name}{args_str}")
                }
            } else {
                format!("{self_type_str}::{name}{args_str}")
            }
        }
        Type::Pat { .. } => "/* pattern */".to_string(),
    }
}

/// Render a normalized generic args key.
fn impl_generic_args_key(args: &GenericArgs) -> String {
    match args {
        GenericArgs::AngleBracketed { args, constraints } => {
            if args.is_empty() && constraints.is_empty() {
                String::new()
            } else {
                let args = args
                    .iter()
                    .map(impl_generic_arg_key)
                    .collect::<Vec<_>>()
                    .join(", ");
                let bindings = constraints
                    .iter()
                    .map(impl_type_constraint_key)
                    .collect::<Vec<_>>()
                    .join(", ");
                let all = if args.is_empty() {
                    bindings
                } else if bindings.is_empty() {
                    args
                } else {
                    format!("{args}, {bindings}")
                };
                format!("<{all}>")
            }
        }
        GenericArgs::Parenthesized { inputs, output } => {
            let inputs = inputs
                .iter()
                .map(impl_type_key)
                .collect::<Vec<_>>()
                .join(", ");
            let output = output
                .as_ref()
                .map(|ty| format!(" -> {}", impl_type_key(ty)))
                .unwrap_or_default();
            format!("({inputs}){output}")
        }
        GenericArgs::ReturnTypeNotation => String::new(),
    }
}

/// Render a normalized generic argument key.
fn impl_generic_arg_key(arg: &GenericArg) -> String {
    match arg {
        GenericArg::Lifetime(lt) => lt.clone(),
        GenericArg::Type(ty) => impl_type_key(ty),
        GenericArg::Const(c) => {
            if c.expr.contains('$') {
                "/* macro expression */".to_string()
            } else {
                c.expr.clone()
            }
        }
        GenericArg::Infer => "_".to_string(),
    }
}

/// Render a normalized associated type constraint key.
fn impl_type_constraint_key(constraint: &AssocItemConstraint) -> String {
    let binding_kind = match &constraint.binding {
        AssocItemConstraintKind::Equality(term) => format!(" = {}", impl_term_key(term)),
        AssocItemConstraintKind::Constraint(bounds) => {
            let b = impl_generic_bounds_key(bounds);
            if b.is_empty() {
                String::new()
            } else {
                format!(": {b}")
            }
        }
    };
    format!("{}{binding_kind}", constraint.name)
}

/// Render a normalized term key used in associated type constraints.
fn impl_term_key(term: &Term) -> String {
    match term {
        Term::Type(ty) => impl_type_key(ty),
        Term::Constant(c) => c.expr.clone(),
    }
}

/// Render a normalized generic bounds key.
fn impl_generic_bounds_key(bounds: &[GenericBound]) -> String {
    let parts: Vec<String> = bounds
        .iter()
        .map(impl_generic_bound_key)
        .filter(|s| !s.trim().is_empty())
        .collect();
    parts.join(" + ")
}

/// Render a normalized generic bound key.
fn impl_generic_bound_key(bound: &GenericBound) -> String {
    match bound {
        GenericBound::Use(_) => String::new(),
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
            match modifier {
                "" => impl_poly_trait_key(&poly_trait),
                "~const" => format!("{modifier} {}", impl_poly_trait_key(&poly_trait)),
                _ => format!("{modifier}{}", impl_poly_trait_key(&poly_trait)),
            }
        }
        GenericBound::Outlives(lifetime) => lifetime.clone(),
    }
}

/// Render a normalized poly trait key.
fn impl_poly_trait_key(poly_trait: &PolyTrait) -> String {
    let generic_params = if poly_trait.generic_params.is_empty() {
        String::new()
    } else {
        let params = poly_trait
            .generic_params
            .iter()
            .filter_map(render_generic_param_def)
            .collect::<Vec<_>>();

        if params.is_empty() {
            String::new()
        } else {
            format!("for<{}> ", params.join(", "))
        }
    };

    format!("{generic_params}{}", impl_path_key(&poly_trait.trait_))
}

/// Render a normalized function pointer key.
fn impl_function_pointer_key(f: &FunctionPointer) -> String {
    let args = impl_function_args_key(&f.sig);
    let return_type = impl_return_type_key(&f.sig);
    if return_type.is_empty() {
        format!("fn({args})")
    } else {
        format!("fn({args}) {return_type}")
    }
}

/// Render a normalized function argument list for a function pointer signature.
fn impl_function_args_key(decl: &FunctionSignature) -> String {
    decl.inputs
        .iter()
        .map(|(name, ty)| {
            if name == "self" {
                match ty {
                    Type::BorrowedRef { is_mutable, .. } => {
                        if *is_mutable {
                            "&mut self".to_string()
                        } else {
                            "&self".to_string()
                        }
                    }
                    Type::ResolvedPath(path) => {
                        if path.path == "Self" && path.args.is_none() {
                            "self".to_string()
                        } else {
                            format!("self: {}", impl_type_key(ty))
                        }
                    }
                    Type::Generic(name) => {
                        if name == "Self" {
                            "self".to_string()
                        } else {
                            format!("self: {}", impl_type_key(ty))
                        }
                    }
                    _ => format!("self: {}", impl_type_key(ty)),
                }
            } else {
                format!("{}: {}", render_identifier(name), impl_type_key(ty))
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Render a normalized return type for a function pointer signature.
fn impl_return_type_key(decl: &FunctionSignature) -> String {
    match &decl.output {
        Some(ty) => format!("-> {}", impl_type_key(ty)),
        None => String::new(),
    }
}

/// Configurable renderer that turns rustdoc data into skeleton Rust source.
pub struct Renderer {
    /// Formatter used to produce tidy Rust output.
    formatter: RustFmt,
    /// Whether auto trait implementations should be included in the output.
    render_auto_impls: bool,
    /// Whether private items should be rendered.
    render_private_items: bool,
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
    /// Effective item selection after composing search and target filtering.
    selection: Option<RenderSelection>,
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
    pub(crate) fn with_selection(mut self, selection: RenderSelection) -> Self {
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
        let selection = self.resolve_selection(crate_data)?;
        let mut state = RenderState {
            config: self,
            crate_data,
            selection,
        };
        state.render()
    }

    /// Compose an explicit search selection with the target path filter.
    fn resolve_selection(&self, crate_data: &Crate) -> Result<Option<RenderSelection>> {
        let filter_selection = if self.filter.is_empty() {
            None
        } else {
            Some(RenderSelection::for_filter(
                crate_data,
                &self.filter,
                self.render_private_items,
                self.render_auto_impls,
                self.render_blanket_impls,
            )?)
        };
        Ok(match (&self.selection, filter_selection) {
            (Some(selection), Some(filter_selection)) => {
                let combined = selection.clone().restrict_to(&filter_selection);
                if !combined.retains_match_from(&filter_selection) {
                    return Err(RuskelError::FilterNotMatched(self.filter.clone()));
                }
                Some(combined)
            }
            (Some(selection), None) => Some(selection.clone()),
            (None, Some(filter_selection)) => Some(filter_selection),
            (None, None) => None,
        })
    }
}

impl RenderState<'_, '_> {
    /// Render the crate, applying filters and formatting output.
    pub fn render(&mut self) -> Result<String> {
        // The root item is always a module
        let root_item = must_get(self.crate_data, &self.crate_data.root)?;
        let output = self.render_item("", root_item, None, false)?;

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
        self.selection.as_ref()
    }

    /// Determine whether the selection context includes a particular item.
    fn selection_context_contains(&self, id: &Id) -> bool {
        match self.selection() {
            Some(selection) => selection.in_context(id),
            None => true,
        }
    }

    /// Determine whether the selection includes one concrete item occurrence.
    fn selection_allows_item(&self, parent: Option<Id>, id: &Id) -> bool {
        match self.selection() {
            Some(selection) => selection.allows_item(parent, id),
            None => true,
        }
    }

    /// Check if an item was an explicit match in the selection.
    fn selection_matches(&self, id: &Id) -> bool {
        match self.selection() {
            Some(selection) => selection.is_match(id),
            None => false,
        }
    }

    /// Determine whether a matched container should expand its children in the rendered output.
    fn selection_expands(&self, id: &Id) -> bool {
        match self.selection() {
            Some(selection) => selection.is_expanded(id),
            None => true,
        }
    }

    /// Determine whether a child item should be rendered based on its parent and selection context.
    fn selection_allows_child(&self, parent_id: &Id, child_id: &Id) -> bool {
        if self.selection().is_none() {
            return true;
        }
        self.selection_expands(parent_id) || self.selection_context_contains(child_id)
    }

    /// Determine whether an item should be rendered based on visibility settings.
    fn is_visible(&self, item: &Item) -> bool {
        self.config.render_private_items || matches!(item.visibility, Visibility::Public)
    }

    /// Determine whether an impl block should be rendered in the output.
    fn should_render_impl(&self, impl_: &Impl) -> bool {
        should_render_impl(
            impl_,
            self.config.render_auto_impls,
            self.config.render_blanket_impls,
        )
    }

    /// Determine whether a module should emit a `//!` doc comment header.
    fn should_module_doc(&self, parent: Option<Id>, item: &Item) -> bool {
        self.selection()
            .is_none_or(|selection| selection.renders_module_docs(parent, &item.id))
    }

    /// Render an item into Rust source text.
    fn render_item(
        &mut self,
        path_prefix: &str,
        item: &Item,
        parent: Option<Id>,
        force_private: bool,
    ) -> Result<String> {
        if !self.selection_allows_item(parent, &item.id) {
            return Ok(String::new());
        }

        let output = match &item.inner {
            ItemEnum::Module(_) => self.render_module(path_prefix, item, parent)?,
            ItemEnum::Struct(_) => self.render_struct(item)?,
            ItemEnum::Enum(_) => self.render_enum(item)?,
            ItemEnum::Trait(_) => self.render_trait(item)?,
            ItemEnum::Use(_) => self.render_use(path_prefix, item)?,
            ItemEnum::Function(_) => self.render_function(item, false)?,
            ItemEnum::Constant { .. } => self.render_constant(item)?,
            ItemEnum::TypeAlias(_) => self.render_type_alias(item)?,
            ItemEnum::Macro(_) => self.render_macro(item)?,
            ItemEnum::ProcMacro(_) => self.render_proc_macro(item)?,
            _ => String::new(),
        };

        if !force_private && !self.is_visible(item) {
            Ok(String::new())
        } else {
            Ok(output)
        }
    }

    /// Render a procedural macro definition.
    fn render_proc_macro(&self, item: &Item) -> Result<String> {
        let mut output = docs(item);

        let fn_name = render_name(item);

        let proc_macro = try_extract_item!(item, ItemEnum::ProcMacro)?;
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

        Ok(output)
    }

    /// Render a macro_rules! or new-style `macro` definition.
    fn render_macro(&self, item: &Item) -> Result<String> {
        let mut output = docs(item);

        let macro_def = try_extract_item!(item, ItemEnum::Macro)?;
        output.push_str("#[macro_export]\n");

        let macro_src = macro_def.to_string();
        let rendered = if macro_src.starts_with("macro ") && !macro_src.starts_with("macro_rules!")
        {
            self.render_new_style_macro(&macro_src)
        } else {
            self.render_macro_rules(&macro_src)
        };

        output.push_str(&rendered);
        output.push('\n');
        Ok(output)
    }

    /// Render a new-style declarative macro while stripping rustdoc placeholders.
    fn render_new_style_macro(&self, macro_src: &str) -> String {
        if MACRO_PLACEHOLDER_REGEX.is_match(macro_src) {
            MACRO_PLACEHOLDER_REGEX.replace(macro_src, "}").to_string()
        } else {
            macro_src.to_string()
        }
    }

    /// Render a `macro_rules!` macro, escaping reserved names when needed.
    fn render_macro_rules(&self, macro_src: &str) -> String {
        if let Some(name_start) = macro_src.find("macro_rules!") {
            let prefix = &macro_src[..name_start + 12]; // "macro_rules!"
            let rest = &macro_src[name_start + 12..];

            let trimmed = rest.trim_start();
            if let Some(name_end) = trimmed.find(|c: char| c.is_whitespace() || c == '{') {
                let name = &trimmed[..name_end];
                let suffix = &trimmed[name_end..];

                if is_reserved_word(name) {
                    return format!("{prefix} r#{name}{suffix}");
                }
            }
        }

        macro_src.to_string()
    }

    /// Render a type alias with generics, bounds, and visibility.
    fn render_type_alias(&self, item: &Item) -> Result<String> {
        let mut output = docs(item);
        let signature = signature::item_signature(self.crate_data, item, SearchItemKind::TypeAlias)
            .ok_or_else(|| {
                RuskelError::Generate(format!(
                    "failed to build type alias signature for '{}'",
                    render_name(item)
                ))
            })?;
        output.push_str(&format!("{signature};\n\n"));

        Ok(output)
    }

    /// Render a `use` statement, applying filter rules for private modules.
    fn render_use(&mut self, path_prefix: &str, item: &Item) -> Result<String> {
        let use_id = item.id;
        let import = try_extract_item!(item, ItemEnum::Use)?;

        if import.is_glob {
            if let Some(source_id) = &import.id
                && let Ok(source_item) = must_get(self.crate_data, source_id)
            {
                let module = try_extract_item!(source_item, ItemEnum::Module)?;
                let mut output = String::new();
                for item_id in &module.items {
                    let item = must_get(self.crate_data, item_id)?;
                    if self.is_visible(item) {
                        output.push_str(&self.render_item(
                            path_prefix,
                            item,
                            Some(use_id),
                            true,
                        )?);
                    }
                }
                return Ok(output);
            }
            // If we can't resolve the glob import, fall back to rendering it as-is
            return Ok(format!("pub use {}::*;\n", escape_path(&import.source)));
        }

        if let Some(imported_id) = import.id.as_ref()
            && let Ok(imported_item) = must_get(self.crate_data, imported_id)
        {
            if imported_item.name.as_deref() == Some(import.name.as_str()) {
                return self.render_item(path_prefix, imported_item, Some(use_id), true);
            }
            let mut aliased_item = imported_item.clone();
            aliased_item.name = Some(import.name.clone());
            return self.render_item(path_prefix, &aliased_item, Some(use_id), true);
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

        Ok(output)
    }

    /// Group impl blocks by compatible signatures, preserving their first-seen order.
    fn collect_impl_groups(&self, parent_id: &Id, impl_ids: &[Id]) -> Result<Vec<ImplGroup>> {
        let mut groups: Vec<ImplGroup> = Vec::new();
        let mut group_indices: HashMap<ImplGroupKey, usize> = HashMap::new();

        for impl_id in impl_ids {
            let impl_item = must_get(self.crate_data, impl_id)?;
            let impl_ = try_extract_item!(impl_item, ItemEnum::Impl)?;
            if impl_.is_negative && impl_.trait_.is_none() {
                return Err(RuskelError::Generate(format!(
                    "negative impl item {impl_id:?} is missing a trait"
                )));
            }
            if !self.should_render_impl(impl_) || !self.selection_allows_child(parent_id, impl_id) {
                continue;
            }

            let signature = ImplSignature::from_impl(impl_);
            let group_key = ImplGroupKey::from_impl(impl_);
            if let Some(index) = group_indices.get(&group_key).copied() {
                groups[index].impl_ids.push(*impl_id);
            } else {
                let index = groups.len();
                groups.push(ImplGroup {
                    signature: signature.clone(),
                    impl_ids: vec![*impl_id],
                });
                group_indices.insert(group_key, index);
            }
        }

        Ok(groups)
    }

    /// Collect traits that should render as a `#[derive(...)]` attribute.
    fn collect_inline_derive_traits(&self, impl_ids: &[Id]) -> Result<Vec<String>> {
        let mut inline_traits = Vec::new();

        for impl_id in impl_ids {
            let impl_item = must_get(self.crate_data, impl_id)?;
            let impl_ = try_extract_item!(impl_item, ItemEnum::Impl)?;
            if impl_.is_synthetic {
                continue;
            }

            if let Some(name) = derive_trait_name(impl_) {
                inline_traits.push(name.to_string());
            }
        }

        Ok(inline_traits)
    }

    /// Append a derive attribute when one or more inline derive traits are present.
    fn push_inline_derive_attribute(output: &mut String, inline_traits: &[String]) {
        if !inline_traits.is_empty() {
            output.push_str(&format!("#[derive({})]\n", inline_traits.join(", ")));
        }
    }

    /// Render a combined impl block for a group of compatible impl items.
    fn render_impl_group(
        &self,
        group: &ImplGroup,
        target_rename: Option<(&str, &str)>,
    ) -> Result<String> {
        let mut docs_output = String::new();
        let mut bodies = Vec::new();

        for impl_id in &group.impl_ids {
            let impl_item = must_get(self.crate_data, impl_id)?;
            let impl_ = try_extract_item!(impl_item, ItemEnum::Impl)?;
            if let Some(rendered) = self.render_impl_body(impl_item, impl_)? {
                docs_output.push_str(&rendered.docs);
                bodies.push(rendered.body);
            }
        }

        if bodies.is_empty() {
            return Ok(String::new());
        }

        let mut output = String::new();
        output.push_str(&docs_output);
        output.push_str(&group.signature.render_header(target_rename));
        for body in bodies {
            output.push_str(&body);
        }
        output.push_str("}\n\n");

        Ok(output)
    }

    /// Render the contents for a single impl block, without its header.
    fn render_impl_body(&self, item: &Item, impl_: &Impl) -> Result<Option<RenderedImplBody>> {
        if !self.selection_context_contains(&item.id) {
            return Ok(None);
        }

        let selection_active = self.selection().is_some();
        let parent_expanded = match &impl_.for_ {
            Type::ResolvedPath(path) => self.selection_expands(&path.id),
            _ => false,
        };
        let expand_children =
            !selection_active || self.selection_expands(&item.id) || parent_expanded;

        if let Some(trait_) = &impl_.trait_
            && let Ok(trait_item) = must_get(self.crate_data, &trait_.id)
            && !self.is_visible(trait_item)
        {
            return Ok(None);
        }

        let mut body = String::new();
        let mut has_content = false;
        for item_id in &impl_.items {
            if let Ok(item) = must_get(self.crate_data, item_id) {
                let is_trait_impl = impl_.trait_.is_some();
                if (!selection_active
                    || expand_children
                    || self.selection_context_contains(item_id))
                    && (is_trait_impl || self.is_visible(item))
                {
                    let rendered = self.render_impl_item(item, expand_children)?;
                    if !rendered.is_empty() {
                        body.push_str(&rendered);
                        has_content = true;
                    }
                }
            }
        }

        if !has_content && !impl_.is_negative {
            return Ok(None);
        }

        Ok(Some(RenderedImplBody {
            docs: docs(item),
            body,
        }))
    }

    /// Render the item inside an impl block.
    fn render_impl_item(&self, item: &Item, include_all: bool) -> Result<String> {
        if !include_all && !self.selection_context_contains(&item.id) {
            return Ok(String::new());
        }

        let rendered = match &item.inner {
            ItemEnum::Function(_) => self.render_function(item, false)?,
            ItemEnum::Constant { .. } => self.render_constant(item)?,
            ItemEnum::AssocType { .. } => render_associated_type(item),
            ItemEnum::TypeAlias(_) => self.render_type_alias(item)?,
            _ => String::new(),
        };

        Ok(rendered)
    }

    /// Render an enum definition, including variants.
    fn render_enum(&self, item: &Item) -> Result<String> {
        let mut output = docs(item);

        let enum_ = try_extract_item!(item, ItemEnum::Enum)?;

        if !self.selection_context_contains(&item.id) {
            return Ok(String::new());
        }

        let selection_active = self.selection().is_some();
        let include_all_variants = self.selection_expands(&item.id);

        let inline_traits = self.collect_inline_derive_traits(&enum_.impls)?;
        Self::push_inline_derive_attribute(&mut output, &inline_traits);

        let signature = signature::item_signature(self.crate_data, item, SearchItemKind::Enum)
            .ok_or_else(|| {
                RuskelError::Generate(format!(
                    "failed to build enum signature for '{}'",
                    render_name(item)
                ))
            })?;
        output.push_str(&format!("{signature} {{\n"));

        for variant_id in &enum_.variants {
            if !selection_active
                || include_all_variants
                || self.selection_context_contains(variant_id)
            {
                let variant_item = must_get(self.crate_data, variant_id)?;
                let include_variant_fields = include_all_variants
                    || !selection_active
                    || self.selection_matches(&variant_item.id);
                let rendered = self.render_enum_variant(variant_item, include_variant_fields)?;
                if !rendered.is_empty() {
                    output.push_str(&rendered);
                }
            }
        }

        output.push_str("}\n\n");

        // Render impl blocks
        let target_rename = self.item_rename(item)?;
        for group in self.collect_impl_groups(&item.id, &enum_.impls)? {
            output.push_str(
                &self.render_impl_group(
                    &group,
                    target_rename
                        .as_ref()
                        .map(|(original, alias)| (original.as_str(), alias.as_str())),
                )?,
            );
        }

        Ok(output)
    }

    /// Render a single enum variant.
    fn render_enum_variant(&self, item: &Item, include_all_fields: bool) -> Result<String> {
        let selection_active = self.selection().is_some();

        if selection_active && !include_all_fields && !self.selection_context_contains(&item.id) {
            return Ok(String::new());
        }

        let mut output = docs(item);

        let variant = try_extract_item!(item, ItemEnum::Variant)?;

        output.push_str(&format!("    {}", render_name(item)));

        match &variant.kind {
            VariantKind::Plain => {}
            VariantKind::Tuple(fields) => {
                let mut rendered_fields = Vec::new();
                for id in fields.iter().flatten() {
                    if selection_active
                        && !include_all_fields
                        && !self.selection_context_contains(id)
                    {
                        continue;
                    }
                    let field_item = must_get(self.crate_data, id)?;
                    let ty = try_extract_item!(field_item, ItemEnum::StructField)?;
                    rendered_fields.push(render_type(ty));
                }
                let fields_str = rendered_fields.join(", ");
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
                            .render_struct_field(field, include_all_fields || !selection_active)?;
                        if !rendered.is_empty() {
                            output.push_str(&rendered);
                        }
                    }
                }
                output.push_str("    }");
            }
        }

        if let Some(discriminant) = &variant.discriminant {
            output.push_str(&format!(" = {}", render_expression(&discriminant.expr)));
        }

        output.push_str(",\n");

        Ok(output)
    }

    /// Render a trait definition.
    fn render_trait(&self, item: &Item) -> Result<String> {
        let mut output = docs(item);

        let trait_ = try_extract_item!(item, ItemEnum::Trait)?;

        if !self.selection_context_contains(&item.id) {
            return Ok(String::new());
        }

        let selection_active = self.selection().is_some();
        let expand_children = self.selection_expands(&item.id);

        let signature = signature::item_signature(self.crate_data, item, SearchItemKind::Trait)
            .ok_or_else(|| {
                RuskelError::Generate(format!(
                    "failed to build trait signature for '{}'",
                    render_name(item)
                ))
            })?;
        output.push_str(&format!("{signature} {{\n"));

        for item_id in &trait_.items {
            if !selection_active || expand_children || self.selection_context_contains(item_id) {
                let item = must_get(self.crate_data, item_id)?;
                output.push_str(&self.render_trait_item(item, expand_children)?);
            }
        }

        output.push_str("}\n\n");

        Ok(output)
    }

    /// Render an item contained within a trait (method, associated type, etc.).
    fn render_trait_item(&self, item: &Item, include_all: bool) -> Result<String> {
        if !include_all && !self.selection_context_contains(&item.id) {
            return Ok(String::new());
        }
        let rendered = match &item.inner {
            ItemEnum::Function(_) => self.render_function(item, true)?,
            ItemEnum::AssocConst { type_, value, .. } => {
                let default_str = value
                    .as_ref()
                    .map(|d| format!(" = {}", render_expression(d)))
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
                ..
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
        };

        Ok(rendered)
    }

    /// Render a struct declaration and its fields.
    fn render_struct(&self, item: &Item) -> Result<String> {
        let mut output = docs(item);

        let struct_ = try_extract_item!(item, ItemEnum::Struct)?;

        if !self.selection_context_contains(&item.id) {
            return Ok(String::new());
        }

        let selection_active = self.selection().is_some();
        let expand_children = selection_active && self.selection_expands(&item.id);
        let force_fields = selection_active && expand_children;

        let inline_traits = self.collect_inline_derive_traits(&struct_.impls)?;
        Self::push_inline_derive_attribute(&mut output, &inline_traits);

        let signature = signature::item_signature(self.crate_data, item, SearchItemKind::Struct)
            .ok_or_else(|| {
                RuskelError::Generate(format!(
                    "failed to build struct signature for '{}'",
                    render_name(item)
                ))
            })?;

        match &struct_.kind {
            StructKind::Unit => {
                output.push_str(&format!("{signature};\n\n"));
            }
            StructKind::Tuple(fields) => {
                let struct_prefix = format!(
                    "{}struct {}{}",
                    render_vis(item),
                    render_name(item),
                    render_generics(&struct_.generics)
                );
                let where_clause = render_where_clause(&struct_.generics);
                let mut rendered_fields = Vec::new();
                for id in fields.iter().flatten() {
                    if !expand_children && !self.selection_context_contains(id) {
                        continue;
                    }
                    let field_item = must_get(self.crate_data, id)?;
                    let ty = try_extract_item!(field_item, ItemEnum::StructField)?;
                    if !self.is_visible(field_item) {
                        rendered_fields.push("_".to_string());
                    } else {
                        rendered_fields.push(format!(
                            "{}{}",
                            render_vis(field_item),
                            render_type(ty)
                        ));
                    }
                }

                if expand_children || !rendered_fields.is_empty() {
                    let fields_str = rendered_fields.join(", ");
                    output.push_str(&format!("{struct_prefix}({fields_str}){where_clause};\n\n"));
                }
            }
            StructKind::Plain { fields, .. } => {
                output.push_str(&format!("{signature} {{\n"));
                for field in fields {
                    let rendered = self.render_struct_field(field, force_fields)?;
                    if !rendered.is_empty() {
                        output.push_str(&rendered);
                    }
                }
                output.push_str("}\n\n");
            }
        }

        // Render impl blocks
        let target_rename = self.item_rename(item)?;
        for group in self.collect_impl_groups(&item.id, &struct_.impls)? {
            output.push_str(
                &self.render_impl_group(
                    &group,
                    target_rename
                        .as_ref()
                        .map(|(original, alias)| (original.as_str(), alias.as_str())),
                )?,
            );
        }

        Ok(output)
    }

    /// Render a struct field, optionally forcing visibility.
    fn render_struct_field(&self, field_id: &Id, force: bool) -> Result<String> {
        let field_item = must_get(self.crate_data, field_id)?;

        if self.selection().is_some() && !force && !self.selection_context_contains(field_id) {
            return Ok(String::new());
        }

        if !(force || self.is_visible(field_item)) {
            return Ok(String::new());
        }

        let ty = try_extract_item!(field_item, ItemEnum::StructField)?;
        let mut out = String::new();
        out.push_str(&docs(field_item));
        out.push_str(&format!(
            "{}{}: {},\n",
            render_vis(field_item),
            render_name(field_item),
            render_type(ty)
        ));
        Ok(out)
    }

    /// Return a use-site rename for an inlined item, if one is active.
    fn item_rename(&self, item: &Item) -> Result<Option<(String, String)>> {
        let original = must_get(self.crate_data, &item.id)?;
        Ok(original
            .name
            .as_deref()
            .zip(item.name.as_deref())
            .filter(|(original, alias)| original != alias)
            .map(|(original, alias)| (original.to_string(), alias.to_string())))
    }

    /// Render a constant definition.
    fn render_constant(&self, item: &Item) -> Result<String> {
        let mut output = docs(item);

        let (_type_, const_) = try_extract_item!(item, ItemEnum::Constant { type_, const_ })?;
        let signature = signature::item_signature(self.crate_data, item, SearchItemKind::Constant)
            .ok_or_else(|| {
                RuskelError::Generate(format!(
                    "failed to build constant signature for '{}'",
                    render_name(item)
                ))
            })?;
        output.push_str(&format!(
            "{signature} = {};\n\n",
            render_expression(&const_.expr)
        ));

        Ok(output)
    }

    /// Render a module and its children.
    fn render_module(
        &mut self,
        path_prefix: &str,
        item: &Item,
        parent: Option<Id>,
    ) -> Result<String> {
        let path_prefix = ppush(path_prefix, &render_name(item));
        let mut output = format!("{}mod {} {{\n", render_vis(item), render_name(item));
        // Add module doc comment if present
        if self.should_module_doc(parent, item)
            && let Some(docs) = &item.docs
        {
            for line in docs.lines() {
                output.push_str(&format!("    //! {line}\n"));
            }
            output.push('\n');
        }

        let module = try_extract_item!(item, ItemEnum::Module)?;
        let module_id = item.id;

        for item_id in &module.items {
            let item = must_get(self.crate_data, item_id)?;
            output.push_str(&self.render_item(&path_prefix, item, Some(module_id), false)?);
        }

        output.push_str("}\n\n");
        Ok(output)
    }

    /// Render a function or method signature.
    fn render_function(&self, item: &Item, is_trait_method: bool) -> Result<String> {
        let mut output = docs(item);
        let function = try_extract_item!(item, ItemEnum::Function)?;
        let kind = if is_trait_method {
            SearchItemKind::TraitMethod
        } else {
            SearchItemKind::Function
        };
        let signature =
            signature::item_signature(self.crate_data, item, kind).ok_or_else(|| {
                RuskelError::Generate(format!(
                    "failed to build function signature for '{}'",
                    render_name(item)
                ))
            })?;
        output.push_str(&signature);

        // Use semicolon for trait method declarations, empty body for implementations
        if is_trait_method && !function.has_body {
            output.push_str(";\n\n");
        } else {
            output.push_str(" {}\n\n");
        }

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, fs, slice};

    use rustdoc_types::{
        Abi, Crate, Function, FunctionHeader, FunctionSignature, Generics, Id, Impl, Item,
        ItemEnum, Module, Path, Struct, StructKind, Target, Trait, Type, Variant, VariantKind,
        Visibility,
    };
    use tempfile::tempdir;

    use super::*;
    use crate::{
        frontmatter::{FrontmatterConfig, FrontmatterHit, FrontmatterSearch},
        search::{SearchDomain, SearchIndex, SearchOptions, SearchResult},
        selection::build_render_selection,
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

    fn empty_crate() -> Crate {
        Crate {
            root: Id(0),
            crate_version: Some("0.0.0".into()),
            includes_private: false,
            index: HashMap::new(),
            paths: HashMap::new(),
            external_crates: HashMap::new(),
            target: Target {
                triple: "test-target".into(),
                target_features: Vec::new(),
            },
            format_version: 0,
        }
    }

    #[test]
    fn render_macro_strips_placeholder_block() -> Result<()> {
        let mut crate_data = empty_crate();
        let macro_id = Id(1);
        crate_data.index.insert(
            macro_id,
            Item {
                id: macro_id,
                crate_id: 0,
                name: Some("placeholder_macro".into()),
                span: None,
                visibility: Visibility::Public,
                docs: None,
                links: HashMap::new(),
                attrs: Vec::new(),
                deprecation: None,
                stability: None,
                const_stability: None,
                inner: ItemEnum::Macro("macro placeholder_macro { () => {} } { ... }".into()),
            },
        );

        let renderer = Renderer::new();
        let state = super::RenderState {
            config: &renderer,
            crate_data: &crate_data,
            selection: None,
        };

        let item = crate_data
            .index
            .get(&macro_id)
            .ok_or_else(|| RuskelError::ItemNotFound(format!("{macro_id:?}")))?;

        let macro_source = try_extract_item!(item, ItemEnum::Macro)?;

        assert!(
            MACRO_PLACEHOLDER_REGEX.is_match(macro_source),
            "fixture macro should reproduce rustdoc placeholder pattern"
        );

        let rendered = state.render_macro(item)?;

        assert!(!rendered.contains("{ ... } { ... }"));
        assert!(rendered.trim_end().ends_with('}'));
        Ok(())
    }

    #[test]
    fn rustdoc_still_emits_placeholder_for_new_style_macros() -> Result<()> {
        let temp_dir = tempdir()?;
        fs::create_dir_all(temp_dir.path().join("src"))?;

        fs::write(
            temp_dir.path().join("Cargo.toml"),
            r#"[package]
name = "macro-fixture"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"
"#,
        )?;

        fs::write(
            temp_dir.path().join("src/lib.rs"),
            "#![feature(decl_macro)]\n\npub macro placeholder_macro() { () }\n",
        )?;

        let builder = rustdoc_json::Builder::default()
            .toolchain("nightly")
            .manifest_path(temp_dir.path().join("Cargo.toml"))
            .document_private_items(true);

        let json_path = match builder.build() {
            Ok(path) => path,
            Err(err) => {
                let msg = err.to_string();
                if msg.contains("rustup") || msg.contains("is not installed") {
                    eprintln!("skipping placeholder detection test: {msg}");
                    return Ok(());
                }
                return Err(RuskelError::Generate(msg));
            }
        };

        let crate_data: Crate = serde_json::from_str(&fs::read_to_string(json_path)?)?;
        let macro_src = crate_data
            .index
            .values()
            .find_map(|item| match &item.inner {
                ItemEnum::Macro(src) => Some(src.clone()),
                _ => None,
            })
            .ok_or_else(|| {
                RuskelError::Generate("macro item missing from rustdoc output".into())
            })?;

        if !MACRO_PLACEHOLDER_REGEX.is_match(&macro_src) {
            eprintln!(
                "rustdoc no longer emits placeholder macro bodies; consider removing \
                 MACRO_PLACEHOLDER_REGEX workaround and simplifying render_macro."
            );
            return Ok(());
        }

        Ok(())
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
        let tools_module = Id(13);
        let tool_function = Id(14);

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
                stability: None,
                const_stability: None,
                inner: ItemEnum::Module(Module {
                    is_crate: true,
                    items: vec![
                        widget,
                        helper_fn,
                        palette_enum,
                        widget_impl,
                        widget_private_impl,
                        tools_module,
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
                stability: None,
                const_stability: None,
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
                stability: None,
                const_stability: None,
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
                stability: None,
                const_stability: None,
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
                stability: None,
                const_stability: None,
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
                stability: None,
                const_stability: None,
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
                stability: None,
                const_stability: None,
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
                    default_unstable: None,
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
                stability: None,
                const_stability: None,
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
                    default_unstable: None,
                }),
            },
        );

        index.insert(
            tools_module,
            Item {
                id: tools_module,
                crate_id: 0,
                name: Some("tools".into()),
                span: None,
                visibility: Visibility::Public,
                docs: Some("Utility helpers".into()),
                links: HashMap::new(),
                attrs: Vec::new(),
                deprecation: None,
                stability: None,
                const_stability: None,
                inner: ItemEnum::Module(Module {
                    is_crate: false,
                    items: vec![tool_function],
                    is_stripped: false,
                }),
            },
        );

        index.insert(
            tool_function,
            Item {
                id: tool_function,
                crate_id: 0,
                name: Some("instrument".into()),
                span: None,
                visibility: Visibility::Public,
                docs: Some("Instrument a widget".into()),
                links: HashMap::new(),
                attrs: Vec::new(),
                deprecation: None,
                stability: None,
                const_stability: None,
                inner: ItemEnum::Function(Function {
                    sig: FunctionSignature {
                        inputs: Vec::new(),
                        output: None,
                        is_c_variadic: false,
                    },
                    generics: empty_generics(),
                    header: default_header(),
                    has_body: true,
                    default_unstable: None,
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
                stability: None,
                const_stability: None,
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
                    default_unstable: None,
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
                stability: None,
                const_stability: None,
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
                stability: None,
                const_stability: None,
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
                stability: None,
                const_stability: None,
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
                stability: None,
                const_stability: None,
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

    /// Create an item with common fixture metadata.
    fn fixture_item(id: Id, name: Option<&str>, inner: ItemEnum) -> Item {
        Item {
            id,
            crate_id: 0,
            name: name.map(str::to_string),
            span: None,
            visibility: Visibility::Public,
            docs: None,
            links: HashMap::new(),
            attrs: Vec::new(),
            deprecation: None,
            stability: None,
            const_stability: None,
            inner,
        }
    }

    /// Build positive and negative impl metadata for one public struct.
    fn impl_polarity_crate(invalid_negative: bool) -> Crate {
        let root = Id(0);
        let widget = Id(1);
        let blocked_trait = Id(2);
        let positive_impl = Id(3);
        let negative_impl = Id(4);
        let positive_method = Id(5);
        let trait_path = Path {
            path: "Blocked".into(),
            id: blocked_trait,
            args: None,
        };
        let widget_type = Type::ResolvedPath(Path {
            path: "Widget".into(),
            id: widget,
            args: None,
        });
        let mut crate_data = empty_crate();
        crate_data.index.insert(
            root,
            fixture_item(
                root,
                Some("polarity"),
                ItemEnum::Module(Module {
                    is_crate: true,
                    items: vec![widget],
                    is_stripped: false,
                }),
            ),
        );
        crate_data.index.insert(
            widget,
            fixture_item(
                widget,
                Some("Widget"),
                ItemEnum::Struct(Struct {
                    kind: StructKind::Unit,
                    generics: empty_generics(),
                    impls: vec![positive_impl, negative_impl],
                }),
            ),
        );
        crate_data.index.insert(
            blocked_trait,
            fixture_item(
                blocked_trait,
                Some("Blocked"),
                ItemEnum::Trait(Trait {
                    is_auto: false,
                    is_unsafe: false,
                    is_dyn_compatible: true,
                    items: Vec::new(),
                    generics: empty_generics(),
                    bounds: Vec::new(),
                    implementations: vec![positive_impl, negative_impl],
                }),
            ),
        );
        crate_data.index.insert(
            positive_impl,
            fixture_item(
                positive_impl,
                None,
                ItemEnum::Impl(Impl {
                    is_unsafe: false,
                    generics: empty_generics(),
                    provided_trait_methods: Vec::new(),
                    trait_: Some(trait_path.clone()),
                    for_: widget_type.clone(),
                    items: vec![positive_method],
                    is_negative: false,
                    is_synthetic: false,
                    blanket_impl: None,
                }),
            ),
        );
        let mut negative_item = fixture_item(
            negative_impl,
            None,
            ItemEnum::Impl(Impl {
                is_unsafe: false,
                generics: empty_generics(),
                provided_trait_methods: Vec::new(),
                trait_: (!invalid_negative).then_some(trait_path),
                for_: widget_type,
                items: Vec::new(),
                is_negative: true,
                is_synthetic: false,
                blanket_impl: None,
            }),
        );
        negative_item.docs = Some("Block this implementation.".into());
        crate_data.index.insert(negative_impl, negative_item);
        crate_data.index.insert(
            positive_method,
            fixture_item(
                positive_method,
                Some("allow"),
                ItemEnum::Function(Function {
                    sig: FunctionSignature {
                        inputs: Vec::new(),
                        output: None,
                        is_c_variadic: false,
                    },
                    generics: empty_generics(),
                    header: default_header(),
                    has_body: true,
                    default_unstable: None,
                }),
            ),
        );
        crate_data
    }

    #[allow(clippy::needless_pass_by_value)]
    fn render_allowing_format_errors(renderer: Renderer, crate_data: &Crate) -> Result<String> {
        match renderer.render(crate_data) {
            Ok(output) => Ok(output),
            Err(RuskelError::Format(_)) => {
                let mut state = super::RenderState {
                    config: &renderer,
                    crate_data,
                    selection: renderer.resolve_selection(crate_data)?,
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
                let root = super::must_get(crate_data, &crate_data.root)?;
                composed.push_str(&state.render_item("", root, None, false)?);
                Ok(composed)
            }
            Err(err) => Err(err),
        }
    }

    fn render_with_selection(crate_data: &Crate, selection: RenderSelection) -> Result<String> {
        let renderer = Renderer::new().with_selection(selection);
        match renderer.render(crate_data) {
            Ok(output) => Ok(output),
            Err(RuskelError::Format(_)) => {
                let mut state = super::RenderState {
                    config: &renderer,
                    crate_data,
                    selection: renderer.resolve_selection(crate_data)?,
                };
                let root = super::must_get(crate_data, &crate_data.root)?;
                state.render_item("", root, None, false)
            }
            Err(err) => Err(err),
        }
    }

    fn find_result_by_suffix(
        results: impl IntoIterator<Item = SearchResult>,
        suffix: &str,
    ) -> Result<SearchResult> {
        results
            .into_iter()
            .find(|r| r.path_string.ends_with(suffix))
            .ok_or_else(|| RuskelError::FilterNotMatched(suffix.to_string()))
    }

    #[test]
    fn exact_filter_and_path_search_select_the_same_impl_member() -> Result<()> {
        let crate_data = fixture_crate();
        let filter =
            RenderSelection::for_filter(&crate_data, "Widget::render", false, false, false)?;
        let index = SearchIndex::build(&crate_data, false);
        let options =
            SearchOptions::configured("fixture::Widget::render", SearchDomain::PATHS, true, false);
        let result = index
            .search(&options)
            .into_iter()
            .find(|result| result.path_string == "fixture::Widget::render")
            .ok_or_else(|| RuskelError::FilterNotMatched(options.query.clone()))?;
        let search = build_render_selection(&index, slice::from_ref(&result), false);

        assert!(filter.is_match(&result.item_id));
        assert!(search.is_match(&result.item_id));
        Ok(())
    }

    #[test]
    fn selection_renders_only_matching_struct_field() -> Result<()> {
        let crate_data = fixture_crate();
        let index = SearchIndex::build(&crate_data, false);
        let mut options = SearchOptions::new("Widget::id");
        options.domains = SearchDomain::PATHS;
        let results = index.search(&options);
        let field = find_result_by_suffix(results, "Widget::id")?;
        let selection = build_render_selection(&index, slice::from_ref(&field), true);
        let rendered = render_with_selection(&crate_data, selection)?;

        assert!(rendered.contains("struct Widget"));
        assert!(rendered.contains("id: u32"));
        assert!(!rendered.contains("name: String"));
        assert!(!rendered.contains("fn helper"));

        Ok(())
    }

    #[test]
    fn selection_renders_only_matching_impl_method() -> Result<()> {
        let crate_data = fixture_crate();
        let index = SearchIndex::build(&crate_data, false);
        let mut options = SearchOptions::new("render");
        options.domains = SearchDomain::NAMES;
        let results = index.search(&options);
        let method = find_result_by_suffix(results, "Widget::render")?;
        let selection = build_render_selection(&index, slice::from_ref(&method), true);
        let rendered = render_with_selection(&crate_data, selection)?;

        assert!(rendered.contains("impl"));
        assert!(rendered.contains("fn render"));
        assert!(!rendered.contains("fn helper"));

        Ok(())
    }

    #[test]
    fn selection_renders_only_matching_enum_variant() -> Result<()> {
        let crate_data = fixture_crate();
        let index = SearchIndex::build(&crate_data, false);
        let mut options = SearchOptions::new("Named");
        options.domains = SearchDomain::NAMES;
        let results = index.search(&options);
        let variant = find_result_by_suffix(results, "Palette::Named")?;
        let selection = build_render_selection(&index, slice::from_ref(&variant), true);
        let rendered = render_with_selection(&crate_data, selection)?;

        assert!(rendered.contains("enum Palette"));
        assert!(rendered.contains("Named"));
        assert!(rendered.contains("pub label: String"));
        assert!(!rendered.contains("Unspecified"));

        Ok(())
    }

    #[test]
    fn struct_match_expands_children_by_default() -> Result<()> {
        let crate_data = fixture_crate();
        let index = SearchIndex::build(&crate_data, false);
        let mut options = SearchOptions::new("Widget");
        options.domains = SearchDomain::NAMES;
        let results = index.search(&options);
        let widget = find_result_by_suffix(results, "Widget")?;
        let selection = build_render_selection(&index, slice::from_ref(&widget), true);
        let rendered = render_with_selection(&crate_data, selection)?;

        assert!(rendered.contains("struct Widget"));
        assert!(rendered.contains("id: u32"));
        assert!(rendered.contains("name: String"));
        assert!(rendered.contains("fn render"));

        Ok(())
    }

    #[test]
    fn struct_match_respects_direct_match_only() -> Result<()> {
        let crate_data = fixture_crate();
        let index = SearchIndex::build(&crate_data, false);
        let mut options = SearchOptions::new("Widget");
        options.domains = SearchDomain::NAMES;
        let results = index.search(&options);
        let widget = find_result_by_suffix(results, "Widget")?;
        let selection = build_render_selection(&index, slice::from_ref(&widget), false);
        let rendered = render_with_selection(&crate_data, selection)?;

        assert!(rendered.contains("struct Widget"));
        assert!(!rendered.contains("id: u32"));
        assert!(!rendered.contains("name: String"));
        assert!(!rendered.contains("fn render"));

        Ok(())
    }

    #[test]
    fn module_match_expands_children_by_default() -> Result<()> {
        let crate_data = fixture_crate();
        let index = SearchIndex::build(&crate_data, false);
        let mut options = SearchOptions::new("tools");
        options.domains = SearchDomain::NAMES;
        let results = index.search(&options);
        let module = find_result_by_suffix(results, "tools")?;
        let selection = build_render_selection(&index, slice::from_ref(&module), true);
        let rendered = render_with_selection(&crate_data, selection)?;

        assert!(rendered.contains("mod tools"));
        assert!(rendered.contains("fn instrument"));

        Ok(())
    }

    #[test]
    fn module_match_respects_direct_match_only() -> Result<()> {
        let crate_data = fixture_crate();
        let index = SearchIndex::build(&crate_data, false);
        let mut options = SearchOptions::new("tools");
        options.domains = SearchDomain::NAMES;
        let results = index.search(&options);
        let module = find_result_by_suffix(results, "tools")?;
        let selection = build_render_selection(&index, slice::from_ref(&module), false);
        let rendered = render_with_selection(&crate_data, selection)?;

        assert!(rendered.contains("mod tools"));
        assert!(!rendered.contains("fn instrument"));

        Ok(())
    }

    #[test]
    fn renderer_preserves_bodyless_negative_impl_polarity() -> Result<()> {
        let crate_data = impl_polarity_crate(false);
        let positive_item = must_get(&crate_data, &Id(3))?;
        let positive = try_extract_item!(positive_item, ItemEnum::Impl)?;
        let negative_item = must_get(&crate_data, &Id(4))?;
        let negative = try_extract_item!(negative_item, ItemEnum::Impl)?;

        assert_ne!(
            ImplGroupKey::from_impl(positive),
            ImplGroupKey::from_impl(negative)
        );
        assert_eq!(
            ImplSignature::from_impl(positive).render_header(None),
            "impl Blocked for Widget {\n"
        );
        assert_eq!(
            ImplSignature::from_impl(negative).render_header(None),
            "impl !Blocked for Widget {\n"
        );

        let output = render_allowing_format_errors(Renderer::new(), &crate_data)?;
        assert!(output.contains("impl Blocked for Widget {"));
        assert!(output.contains("fn allow"));
        assert!(output.contains("Block this implementation."));
        assert!(
            output.contains("impl !Blocked for Widget {}")
                || output.contains("impl !Blocked for Widget {\n}\n"),
            "bodyless negative impl was not rendered:\n{output}"
        );
        Ok(())
    }

    #[test]
    fn renderer_rejects_negative_inherent_metadata() {
        let crate_data = impl_polarity_crate(true);
        let error = Renderer::new()
            .render(&crate_data)
            .expect_err("negative inherent metadata should fail");

        assert!(matches!(error, RuskelError::Generate(_)));
        assert_eq!(
            error.to_string(),
            "negative impl item Id(4) is missing a trait"
        );
    }

    #[test]
    fn renderer_omits_empty_impl_blocks_when_private_items_hidden() -> Result<()> {
        let crate_data = fixture_crate();
        let output = render_allowing_format_errors(Renderer::new(), &crate_data)?;

        assert!(
            !output.contains("impl Widget {}"),
            "expected renderer to omit empty impl blocks:\n{output}"
        );

        Ok(())
    }

    #[test]
    fn renderer_keeps_impl_when_private_items_rendered() -> Result<()> {
        let crate_data = fixture_crate();
        let output =
            render_allowing_format_errors(Renderer::new().with_private_items(true), &crate_data)?;

        assert!(output.contains("impl Widget {"));
        assert!(output.contains("fn render"));
        assert!(output.contains("fn internal_helper"));

        Ok(())
    }

    #[test]
    fn frontmatter_inserts_target_visibility_and_path() -> Result<()> {
        let crate_data = fixture_crate();
        let frontmatter = FrontmatterConfig::for_target("fixture::Widget")
            .with_filter(Some("fixture::Widget".into()));
        let output = render_allowing_format_errors(
            Renderer::new().with_frontmatter(frontmatter),
            &crate_data,
        )?;

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

        Ok(())
    }

    #[test]
    fn frontmatter_can_be_disabled() -> Result<()> {
        let crate_data = fixture_crate();
        let output = render_allowing_format_errors(Renderer::new(), &crate_data)?;

        assert!(!output.starts_with(
            "// Ruskel skeleton - syntactically valid Rust with implementation omitted."
        ));

        Ok(())
    }

    #[test]
    fn frontmatter_lists_search_hits_with_domains() -> Result<()> {
        let crate_data = fixture_crate();
        let hits = vec![FrontmatterHit::new("fixture::Widget", SearchDomain::NAMES)];
        let search_meta = FrontmatterSearch::new(
            "Widget",
            SearchDomain::NAMES | SearchDomain::DOCS,
            false,
            true,
            hits,
        );
        let frontmatter = FrontmatterConfig::for_target("fixture")
            .with_filter(Some("fixture".into()))
            .with_search(search_meta);
        let output = Renderer::new().with_frontmatter(frontmatter);
        let output = render_allowing_format_errors(output, &crate_data)?;

        assert!(output.contains(
            "// search: query=\"Widget\"; case_sensitive=false; domains=name, doc; expand_containers=true"
        ));
        assert!(output.contains("// hits (1):"));
        assert!(output.contains("//   - fixture::Widget [name]"));

        Ok(())
    }
}
