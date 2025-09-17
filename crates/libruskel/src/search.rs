//! Internal search index implementation.
#![allow(clippy::missing_docs_in_private_items)]

use std::collections::{HashMap, HashSet};

use bitflags::bitflags;
use rustdoc_types::{Crate, Id, Item, ItemEnum, Module, Struct, StructKind, Visibility};

use crate::{
    crateutils::{
        render_function_args, render_generic_bounds, render_generics, render_name, render_path,
        render_return_type, render_type, render_vis, render_where_clause,
    },
    render::RenderSelection,
};

bitflags! {
    /// Domains that a search query can operate over.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SearchDomain: u32 {
        /// Match against item names.
        const NAMES = 1 << 0;
        /// Match against documentation strings.
        const DOCS = 1 << 1;
        /// Match against canonical module paths.
        const PATHS = 1 << 2;
        /// Match against rendered item signatures.
        const SIGNATURES = 1 << 3;
    }
}

impl Default for SearchDomain {
    fn default() -> Self {
        Self::NAMES | Self::DOCS | Self::SIGNATURES
    }
}

/// Options that control how a crate search should be performed.
#[derive(Debug, Clone)]
pub struct SearchOptions {
    /// Raw user query to evaluate.
    pub query: String,
    /// Domains to search across; defaults to [`SearchDomain::default`].
    pub domains: SearchDomain,
    /// Whether matching should respect letter casing.
    pub case_sensitive: bool,
    /// Whether to include private or crate-private items.
    pub include_private: bool,
    /// Whether matched container items should expand to include their children.
    pub expand_containers: bool,
}

impl SearchOptions {
    /// Create a new options struct with the provided query string.
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            domains: SearchDomain::default(),
            case_sensitive: false,
            include_private: false,
            expand_containers: true,
        }
    }

    /// Ensure the options have at least one domain selected.
    pub fn ensure_domains(&mut self) {
        if self.domains.is_empty() {
            self.domains = SearchDomain::default();
        }
    }
}

/// Classified kind associated with a search result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SearchItemKind {
    /// Synthetic crate root module.
    Crate,
    /// Regular module.
    Module,
    /// Struct definition.
    Struct,
    /// Union definition.
    Union,
    /// Enum definition.
    Enum,
    /// Variant within an enum.
    EnumVariant,
    /// Named or positional field within a struct or union.
    Field,
    /// Trait definition.
    Trait,
    /// Trait alias definition.
    TraitAlias,
    /// Free function.
    Function,
    /// Method inside an impl block.
    Method,
    /// Trait method declaration.
    TraitMethod,
    /// Associated constant.
    AssocConst,
    /// Associated type.
    AssocType,
    /// Top-level constant.
    Constant,
    /// Static item.
    Static,
    /// Type alias.
    TypeAlias,
    /// `use` declaration.
    Use,
    /// Macro_rules! definition.
    Macro,
    /// Procedural macro entrypoint.
    ProcMacro,
    /// Primitive type description.
    Primitive,
    /// Synthetic segment representing an impl target.
    ImplTarget,
}

impl SearchItemKind {
    /// Human-friendly label describing the item kind.
    pub fn label(self) -> &'static str {
        match self {
            Self::Crate => "crate",
            Self::Module => "module",
            Self::Struct => "struct",
            Self::Union => "union",
            Self::Enum => "enum",
            Self::EnumVariant => "enum variant",
            Self::Field => "field",
            Self::Trait => "trait",
            Self::TraitAlias => "trait alias",
            Self::Function => "function",
            Self::Method => "method",
            Self::TraitMethod => "trait method",
            Self::AssocConst => "assoc const",
            Self::AssocType => "assoc type",
            Self::Constant => "constant",
            Self::Static => "static",
            Self::TypeAlias => "type alias",
            Self::Use => "use",
            Self::Macro => "macro",
            Self::ProcMacro => "proc macro",
            Self::Primitive => "primitive",
            Self::ImplTarget => "impl target",
        }
    }
}

/// Component in a canonical path leading to an item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPathSegment {
    /// Raw identifier for the segment without keyword escaping.
    pub name: String,
    /// Display name used when rendering the path.
    pub display_name: String,
    /// Classification of the segment.
    pub kind: SearchItemKind,
    /// Whether the segment corresponds to a publicly visible item.
    pub is_public: bool,
}

/// Aggregated search response containing matches and rendered output.
#[derive(Debug, Clone)]
pub struct SearchResponse {
    /// Matched records returned by the index.
    pub results: Vec<SearchResult>,
    /// Rendered skeleton filtered to only include matched items.
    pub rendered: String,
}

/// Lightweight record describing an item for list mode output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    /// Kind classification for the item.
    pub kind: SearchItemKind,
    /// Canonical path rendered as a `::` separated string.
    pub path: String,
}

/// Result of performing a query against a crate index.
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Identifier of the matching item.
    pub item_id: Id,
    /// Kind of result item.
    pub kind: SearchItemKind,
    /// Canonical path segments to reach the item.
    pub path: Vec<SearchPathSegment>,
    /// Canonical path rendered as a `::` separated string.
    pub path_string: String,
    /// Raw identifier of the item.
    pub raw_name: String,
    /// Display name formatted for rendering.
    pub display_name: String,
    /// Documentation snippet if available.
    pub docs: Option<String>,
    /// Rendered signature used for matching and display.
    pub signature: Option<String>,
    /// Ancestor chain of items that must be rendered for context.
    pub ancestors: Vec<Id>,
    /// Domains that produced a match (empty when stored in the index).
    pub matched: SearchDomain,
}

impl SearchResult {
    /// Reset match metadata so the record can be reused for a new query.
    fn clear_match_info(&mut self) {
        self.matched = SearchDomain::empty();
    }
}

/// Index of crate items prepared for search queries.
#[derive(Debug, Default, Clone)]
pub struct SearchIndex {
    entries: Vec<SearchResult>,
    id_to_entry: HashMap<Id, usize>,
}

impl SearchIndex {
    /// Construct a new index by traversing the provided crate.
    pub fn build(crate_data: &Crate, include_private: bool) -> Self {
        let mut builder = IndexBuilder::new(crate_data, include_private);
        builder.traverse();
        builder.finish()
    }

    /// Retrieve the immutable list of indexed entries.
    pub fn entries(&self) -> &[SearchResult] {
        &self.entries
    }

    /// Look up an indexed entry by ID.
    pub fn get(&self, id: &Id) -> Option<&SearchResult> {
        self.id_to_entry.get(id).map(|idx| &self.entries[*idx])
    }

    /// Prepare the index for a new search by clearing cached match metadata.
    pub fn reset_matches(&mut self) {
        for entry in &mut self.entries {
            entry.clear_match_info();
        }
    }

    /// Execute a query against the index and return matching results.
    pub fn search(&self, options: &SearchOptions) -> Vec<SearchResult> {
        let mut opts = options.clone();
        opts.ensure_domains();
        let trimmed = opts.query.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }

        let normalized_query = if opts.case_sensitive {
            trimmed.to_string()
        } else {
            trimmed.to_lowercase()
        };

        let mut results = Vec::new();
        for entry in &self.entries {
            let mut matched = SearchDomain::empty();
            if opts.domains.contains(SearchDomain::NAMES)
                && contains(&entry.raw_name, &normalized_query, opts.case_sensitive)
            {
                matched |= SearchDomain::NAMES;
            }
            if opts.domains.contains(SearchDomain::DOCS)
                && entry
                    .docs
                    .as_ref()
                    .is_some_and(|docs| contains(docs, &normalized_query, opts.case_sensitive))
            {
                matched |= SearchDomain::DOCS;
            }
            if opts.domains.contains(SearchDomain::PATHS)
                && contains(&entry.path_string, &normalized_query, opts.case_sensitive)
            {
                matched |= SearchDomain::PATHS;
            }
            if opts.domains.contains(SearchDomain::SIGNATURES)
                && entry
                    .signature
                    .as_ref()
                    .is_some_and(|sig| contains(sig, &normalized_query, opts.case_sensitive))
            {
                matched |= SearchDomain::SIGNATURES;
            }

            if !matched.is_empty() {
                let mut clone = entry.clone();
                clone.matched = matched;
                results.push(clone);
            }
        }

        results
    }
}

#[derive(Clone)]
struct PathStackEntry {
    id: Option<Id>,
    segment: SearchPathSegment,
}

struct IndexBuilder<'a> {
    crate_data: &'a Crate,
    include_private: bool,
    stack: Vec<PathStackEntry>,
    entries: Vec<SearchResult>,
    visited: HashSet<Id>,
}

impl<'a> IndexBuilder<'a> {
    fn new(crate_data: &'a Crate, include_private: bool) -> Self {
        Self {
            crate_data,
            include_private,
            stack: Vec::new(),
            entries: Vec::new(),
            visited: HashSet::new(),
        }
    }

    fn traverse(&mut self) {
        if let Some(root) = self.crate_data.index.get(&self.crate_data.root) {
            self.visit_root(root);
        }
    }

    fn finish(self) -> SearchIndex {
        let entries: Vec<SearchResult> = self.entries;
        let mut id_to_entry = HashMap::with_capacity(entries.len());
        for (idx, entry) in entries.iter().enumerate() {
            id_to_entry.insert(entry.item_id, idx);
        }
        SearchIndex {
            entries,
            id_to_entry,
        }
    }

    fn visit_root(&mut self, item: &Item) {
        if let ItemEnum::Module(module) = &item.inner {
            let segment = self.make_segment(item, SearchItemKind::Crate, Some("crate"));
            self.record_item(item, SearchItemKind::Crate, &segment, true, &[]);
            self.stack.push(PathStackEntry {
                id: Some(item.id),
                segment,
            });
            self.visit_module_items(module);
            self.stack.pop();
        }
    }

    fn visit_item(&mut self, item_id: &Id) {
        if !self.visited.insert(*item_id) {
            return;
        }
        let Some(item) = self.crate_data.index.get(item_id) else {
            return;
        };
        match &item.inner {
            ItemEnum::Module(module) => self.visit_module(item, module),
            ItemEnum::Struct(struct_) => self.visit_struct(item, struct_),
            ItemEnum::Enum(enum_) => self.visit_enum(item, enum_),
            ItemEnum::Union(union_) => self.visit_union(item, union_),
            ItemEnum::Trait(trait_) => self.visit_trait(item, trait_),
            ItemEnum::Function(_) => self.visit_function(item),
            ItemEnum::TypeAlias(_) => self.record_simple(item, SearchItemKind::TypeAlias),
            ItemEnum::Constant { .. } => self.record_simple(item, SearchItemKind::Constant),
            ItemEnum::Static(_) => self.record_simple(item, SearchItemKind::Static),
            ItemEnum::Macro(_) => self.record_simple(item, SearchItemKind::Macro),
            ItemEnum::ProcMacro(_) => self.record_simple(item, SearchItemKind::ProcMacro),
            ItemEnum::TraitAlias(_) => self.record_simple(item, SearchItemKind::TraitAlias),
            ItemEnum::Use(_) => self.record_simple(item, SearchItemKind::Use),
            ItemEnum::Primitive(_) => self.record_simple(item, SearchItemKind::Primitive),
            ItemEnum::Variant(variant) => self.visit_variant(item, variant),
            ItemEnum::StructField(_) => self.record_simple(item, SearchItemKind::Field),
            ItemEnum::AssocConst { .. } => self.record_simple(item, SearchItemKind::AssocConst),
            ItemEnum::AssocType { .. } => self.record_simple(item, SearchItemKind::AssocType),
            ItemEnum::Impl(impl_) => self.visit_impl(item, impl_),
            ItemEnum::ExternCrate { .. } | ItemEnum::ExternType => {}
        }
    }

    fn visit_module(&mut self, item: &Item, module: &Module) {
        let segment = self.make_segment(item, SearchItemKind::Module, None);
        let _ = self.record_item(item, SearchItemKind::Module, &segment, module.is_crate, &[]);
        self.stack.push(PathStackEntry {
            id: Some(item.id),
            segment,
        });
        self.visit_module_items(module);
        self.stack.pop();
    }

    fn visit_module_items(&mut self, module: &Module) {
        for child in &module.items {
            self.visit_item(child);
        }
    }

    fn visit_struct(&mut self, item: &Item, struct_: &Struct) {
        let segment = self.make_segment(item, SearchItemKind::Struct, None);
        let include_children = self.record_item(item, SearchItemKind::Struct, &segment, false, &[]);
        self.stack.push(PathStackEntry {
            id: Some(item.id),
            segment,
        });
        if include_children {
            match &struct_.kind {
                StructKind::Unit => {}
                StructKind::Tuple(fields) => {
                    for field_id in fields.iter().flatten() {
                        self.visit_item(field_id);
                    }
                }
                StructKind::Plain { fields, .. } => {
                    for field in fields {
                        self.visit_item(field);
                    }
                }
            }
        }
        for impl_id in &struct_.impls {
            self.visit_item(impl_id);
        }
        self.stack.pop();
    }

    fn visit_union(&mut self, item: &Item, union_: &rustdoc_types::Union) {
        let segment = self.make_segment(item, SearchItemKind::Union, None);
        let include_children = self.record_item(item, SearchItemKind::Union, &segment, false, &[]);
        self.stack.push(PathStackEntry {
            id: Some(item.id),
            segment,
        });
        if include_children {
            for field in &union_.fields {
                self.visit_item(field);
            }
        }
        for impl_id in &union_.impls {
            self.visit_item(impl_id);
        }
        self.stack.pop();
    }

    fn visit_enum(&mut self, item: &Item, enum_: &rustdoc_types::Enum) {
        let segment = self.make_segment(item, SearchItemKind::Enum, None);
        let include_children = self.record_item(item, SearchItemKind::Enum, &segment, false, &[]);
        self.stack.push(PathStackEntry {
            id: Some(item.id),
            segment,
        });
        if include_children {
            for variant_id in &enum_.variants {
                self.visit_item(variant_id);
            }
        }
        for impl_id in &enum_.impls {
            self.visit_item(impl_id);
        }
        self.stack.pop();
    }

    fn visit_variant(&mut self, item: &Item, variant: &rustdoc_types::Variant) {
        let segment = self.make_segment(item, SearchItemKind::EnumVariant, None);
        let include_children =
            self.record_item(item, SearchItemKind::EnumVariant, &segment, false, &[]);
        self.stack.push(PathStackEntry {
            id: Some(item.id),
            segment,
        });
        if include_children {
            match &variant.kind {
                rustdoc_types::VariantKind::Plain => {}
                rustdoc_types::VariantKind::Tuple(fields) => {
                    for field_id in fields.iter().flatten() {
                        self.visit_item(field_id);
                    }
                }
                rustdoc_types::VariantKind::Struct { fields, .. } => {
                    for field_id in fields {
                        self.visit_item(field_id);
                    }
                }
            }
        }
        self.stack.pop();
    }

    fn visit_trait(&mut self, item: &Item, trait_: &rustdoc_types::Trait) {
        let segment = self.make_segment(item, SearchItemKind::Trait, None);
        let include_children = self.record_item(item, SearchItemKind::Trait, &segment, false, &[]);
        self.stack.push(PathStackEntry {
            id: Some(item.id),
            segment,
        });
        if include_children {
            for assoc_id in &trait_.items {
                if let Some(assoc) = self.crate_data.index.get(assoc_id) {
                    match &assoc.inner {
                        ItemEnum::Function(_) => {
                            self.record_trait_member(assoc, SearchItemKind::TraitMethod)
                        }
                        ItemEnum::AssocConst { .. } => {
                            self.record_trait_member(assoc, SearchItemKind::AssocConst)
                        }
                        ItemEnum::AssocType { .. } => {
                            self.record_trait_member(assoc, SearchItemKind::AssocType)
                        }
                        _ => self.visit_item(assoc_id),
                    }
                }
            }
        }
        for impl_id in &trait_.implementations {
            self.visit_item(impl_id);
        }
        self.stack.pop();
    }

    fn visit_function(&mut self, item: &Item) {
        self.record_simple(item, SearchItemKind::Function);
    }

    fn record_trait_member(&mut self, item: &Item, kind: SearchItemKind) {
        let segment = self.make_segment(item, kind, None);
        self.record_item(item, kind, &segment, false, &[]);
    }

    fn visit_impl(&mut self, impl_item: &Item, impl_: &rustdoc_types::Impl) {
        if impl_.is_synthetic {
            return;
        }

        let mut pushed: Vec<PathStackEntry> = Vec::new();

        if let Some(target_entry) = self.impl_target_entry(&impl_.for_) {
            let has_target = target_entry
                .id
                .and_then(|id| {
                    self.stack
                        .iter()
                        .find(|entry| entry.id == Some(id))
                        .map(|_| ())
                })
                .is_some();
            if !has_target {
                self.stack.push(target_entry.clone());
                pushed.push(target_entry);
            }
        }

        if let Some(trait_entry) = self.impl_trait_entry(&impl_.trait_) {
            self.stack.push(trait_entry.clone());
            pushed.push(trait_entry);
        }

        let is_trait_impl = impl_.trait_.is_some();
        for member_id in &impl_.items {
            if let Some(member) = self.crate_data.index.get(member_id) {
                match &member.inner {
                    ItemEnum::Function(_) => self.record_impl_member(
                        impl_item.id,
                        member,
                        SearchItemKind::Method,
                        is_trait_impl,
                    ),
                    ItemEnum::AssocConst { .. } => self.record_impl_member(
                        impl_item.id,
                        member,
                        SearchItemKind::AssocConst,
                        is_trait_impl,
                    ),
                    ItemEnum::AssocType { .. } => self.record_impl_member(
                        impl_item.id,
                        member,
                        SearchItemKind::AssocType,
                        is_trait_impl,
                    ),
                    ItemEnum::TypeAlias(_) => self.record_impl_member(
                        impl_item.id,
                        member,
                        SearchItemKind::TypeAlias,
                        is_trait_impl,
                    ),
                    ItemEnum::Constant { .. } => self.record_impl_member(
                        impl_item.id,
                        member,
                        SearchItemKind::Constant,
                        is_trait_impl,
                    ),
                    _ => self.visit_item(member_id),
                }
            }
        }

        for _ in 0..pushed.len() {
            self.stack.pop();
        }
    }

    fn record_impl_member(
        &mut self,
        impl_id: Id,
        item: &Item,
        kind: SearchItemKind,
        _is_trait_impl: bool,
    ) {
        let segment = self.make_segment(item, kind, None);
        self.record_item(item, kind, &segment, false, &[impl_id]);
    }

    fn impl_trait_entry(&self, trait_path: &Option<rustdoc_types::Path>) -> Option<PathStackEntry> {
        trait_path.as_ref().map(|path| {
            let display = render_path(path);
            let (id, kind, is_public) =
                if let Some(trait_item) = self.crate_data.index.get(&path.id) {
                    (
                        Some(path.id),
                        SearchItemKind::Trait,
                        matches!(
                            trait_item.visibility,
                            Visibility::Public | Visibility::Default
                        ),
                    )
                } else {
                    (None, SearchItemKind::Trait, true)
                };
            PathStackEntry {
                id,
                segment: SearchPathSegment {
                    name: display.clone(),
                    display_name: display,
                    kind,
                    is_public,
                },
            }
        })
    }

    fn impl_target_entry(&self, ty: &rustdoc_types::Type) -> Option<PathStackEntry> {
        match ty {
            rustdoc_types::Type::ResolvedPath(path) => {
                let name = render_type(ty);
                if let Some(item) = self.crate_data.index.get(&path.id) {
                    let kind = self
                        .kind_from_item(item)
                        .unwrap_or(SearchItemKind::ImplTarget);
                    let segment = self.make_segment(item, kind, None);
                    Some(PathStackEntry {
                        id: Some(item.id),
                        segment: SearchPathSegment {
                            name: name.clone(),
                            display_name: name,
                            kind: SearchItemKind::ImplTarget,
                            is_public: segment.is_public,
                        },
                    })
                } else {
                    Some(PathStackEntry {
                        id: None,
                        segment: SearchPathSegment {
                            name: name.clone(),
                            display_name: name,
                            kind: SearchItemKind::ImplTarget,
                            is_public: true,
                        },
                    })
                }
            }
            _ => {
                let name = render_type(ty);
                Some(PathStackEntry {
                    id: None,
                    segment: SearchPathSegment {
                        name: name.clone(),
                        display_name: name,
                        kind: SearchItemKind::ImplTarget,
                        is_public: true,
                    },
                })
            }
        }
    }

    fn kind_from_item(&self, item: &Item) -> Option<SearchItemKind> {
        match item.inner {
            ItemEnum::Module(_) => Some(SearchItemKind::Module),
            ItemEnum::Struct(_) => Some(SearchItemKind::Struct),
            ItemEnum::Enum(_) => Some(SearchItemKind::Enum),
            ItemEnum::Union(_) => Some(SearchItemKind::Union),
            ItemEnum::Trait(_) => Some(SearchItemKind::Trait),
            ItemEnum::TraitAlias(_) => Some(SearchItemKind::TraitAlias),
            ItemEnum::Function(_) => Some(SearchItemKind::Function),
            ItemEnum::TypeAlias(_) => Some(SearchItemKind::TypeAlias),
            ItemEnum::Constant { .. } => Some(SearchItemKind::Constant),
            ItemEnum::Static(_) => Some(SearchItemKind::Static),
            ItemEnum::Macro(_) => Some(SearchItemKind::Macro),
            ItemEnum::ProcMacro(_) => Some(SearchItemKind::ProcMacro),
            ItemEnum::Primitive(_) => Some(SearchItemKind::Primitive),
            ItemEnum::StructField(_) => Some(SearchItemKind::Field),
            ItemEnum::Variant(_) => Some(SearchItemKind::EnumVariant),
            _ => None,
        }
    }

    fn record_simple(&mut self, item: &Item, kind: SearchItemKind) {
        let segment = self.make_segment(item, kind, None);
        self.record_item(item, kind, &segment, false, &[]);
    }

    fn make_segment(
        &self,
        item: &Item,
        kind: SearchItemKind,
        fallback: Option<&str>,
    ) -> SearchPathSegment {
        let raw_name = item
            .name
            .as_deref()
            .map(ToOwned::to_owned)
            .or_else(|| fallback.map(ToOwned::to_owned))
            .unwrap_or_else(|| "?".to_string());
        let display_name = if item.name.is_some() {
            render_name(item)
        } else {
            raw_name.clone()
        };
        SearchPathSegment {
            name: raw_name,
            display_name,
            kind,
            is_public: matches!(item.visibility, Visibility::Public | Visibility::Default),
        }
    }

    fn record_item(
        &mut self,
        item: &Item,
        kind: SearchItemKind,
        segment: &SearchPathSegment,
        always_include: bool,
        extra_ancestors: &[Id],
    ) -> bool {
        if !always_include && !self.should_include(item) {
            return false;
        }

        let mut path: Vec<SearchPathSegment> = self
            .stack
            .iter()
            .map(|entry| entry.segment.clone())
            .collect();
        path.push(segment.clone());

        let mut ancestors: Vec<Id> = self.stack.iter().filter_map(|entry| entry.id).collect();
        ancestors.extend(extra_ancestors.iter().copied());

        let path_string = join_path(&path);
        let signature = self.signature_for(item, kind);
        let result = SearchResult {
            item_id: item.id,
            kind,
            path,
            path_string,
            raw_name: segment.name.clone(),
            display_name: segment.display_name.clone(),
            docs: item.docs.clone(),
            signature,
            ancestors,
            matched: SearchDomain::empty(),
        };

        self.entries.push(result);
        true
    }
    fn signature_for(&self, item: &Item, kind: SearchItemKind) -> Option<String> {
        match (&item.inner, kind) {
            (ItemEnum::Function(function), SearchItemKind::Function)
            | (ItemEnum::Function(function), SearchItemKind::Method)
            | (ItemEnum::Function(function), SearchItemKind::TraitMethod) => {
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
                Some(signature)
            }
            (ItemEnum::StructField(ty), SearchItemKind::Field) => {
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
                Some(signature)
            }
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
            (ItemEnum::Macro(_), SearchItemKind::Macro) => {
                Some(format!("macro {}", render_name(item)))
            }
            (ItemEnum::ProcMacro(proc_macro), SearchItemKind::ProcMacro) => {
                let prefix = match proc_macro.kind {
                    rustdoc_types::MacroKind::Derive => "#[proc_macro_derive]",
                    rustdoc_types::MacroKind::Attr => "#[proc_macro_attribute]",
                    rustdoc_types::MacroKind::Bang => "#[proc_macro]",
                };
                Some(format!("{} {}", prefix, render_name(item)))
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
                let mut signature = render_name(item);
                match &variant.kind {
                    rustdoc_types::VariantKind::Plain => {}
                    rustdoc_types::VariantKind::Tuple(fields) => {
                        let mut parts = Vec::new();
                        for field in fields {
                            if let Some(field_id) = field
                                && let Some(field_item) = self.crate_data.index.get(field_id)
                                && let ItemEnum::StructField(ty) = &field_item.inner
                            {
                                parts.push(render_type(ty));
                            }
                        }
                        signature.push('(');
                        signature.push_str(&parts.join(", "));
                        signature.push(')');
                    }
                    rustdoc_types::VariantKind::Struct { fields, .. } => {
                        let mut parts = Vec::new();
                        for field_id in fields {
                            if let Some(field_item) = self.crate_data.index.get(field_id)
                                && let ItemEnum::StructField(ty) = &field_item.inner
                            {
                                let name = field_item
                                    .name
                                    .as_deref()
                                    .map(ToOwned::to_owned)
                                    .unwrap_or_else(|| "_".to_string());
                                parts.push(format!("{}: {}", name, render_type(ty)));
                            }
                        }
                        signature.push_str(" { ");
                        signature.push_str(&parts.join(", "));
                        signature.push_str(" }");
                    }
                }
                Some(signature)
            }
            _ => None,
        }
    }

    fn should_include(&self, item: &Item) -> bool {
        if self.include_private {
            return true;
        }
        matches!(item.visibility, Visibility::Public | Visibility::Default)
    }
}

fn join_path(path: &[SearchPathSegment]) -> String {
    let mut out = String::new();
    for (idx, segment) in path.iter().enumerate() {
        if idx > 0 {
            out.push_str("::");
        }
        out.push_str(&segment.name);
    }
    out
}

fn contains(haystack: &str, needle: &str, case_sensitive: bool) -> bool {
    if needle.is_empty() {
        return false;
    }
    if case_sensitive {
        haystack.contains(needle)
    } else {
        haystack.to_lowercase().contains(needle)
    }
}

/// Build a renderer selection set covering matches, their ancestors, and optionally their children.
pub fn build_render_selection(
    index: &SearchIndex,
    results: &[SearchResult],
    expand_containers: bool,
) -> RenderSelection {
    let mut matches = HashSet::new();
    let mut context = HashSet::new();
    let mut expanded = HashSet::new();
    for result in results {
        matches.insert(result.item_id);
        context.insert(result.item_id);
        context.extend(result.ancestors.iter().copied());
    }
    if expand_containers {
        let containers: HashSet<Id> = results
            .iter()
            .filter(|result| {
                matches!(
                    result.kind,
                    SearchItemKind::Crate
                        | SearchItemKind::Module
                        | SearchItemKind::Struct
                        | SearchItemKind::Trait
                )
            })
            .map(|result| result.item_id)
            .collect();

        if !containers.is_empty() {
            expanded.extend(containers.iter().copied());
            let mut descendant_containers = HashSet::new();
            for entry in index.entries() {
                if let Some(pos) = entry
                    .ancestors
                    .iter()
                    .position(|ancestor| containers.contains(ancestor))
                {
                    context.insert(entry.item_id);
                    for descendant in entry.ancestors.iter().skip(pos + 1) {
                        context.insert(*descendant);
                        descendant_containers.insert(*descendant);
                    }
                }
            }
            expanded.extend(descendant_containers);
        }
    }

    RenderSelection::new(matches, context, expanded)
}

/// Format the set of matched domains into human-friendly labels.
pub fn describe_domains(domains: SearchDomain) -> Vec<&'static str> {
    let mut labels = Vec::new();
    if domains.contains(SearchDomain::NAMES) {
        labels.push("name");
    }
    if domains.contains(SearchDomain::DOCS) {
        labels.push("doc");
    }
    if domains.contains(SearchDomain::PATHS) {
        labels.push("path");
    }
    if domains.contains(SearchDomain::SIGNATURES) {
        labels.push("signature");
    }
    labels
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use rustdoc_types::{
        Abi, Crate, Function, FunctionHeader, FunctionSignature, Generics, Id, Impl, Item,
        ItemEnum, Module, Path, Struct, StructKind, Target, Trait, Type, Visibility,
    };

    use super::*;

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
        let widget_field = Id(2);
        let widget_impl = Id(3);
        let render_method = Id(4);
        let helper_fn = Id(5);
        let paintable_trait = Id(6);
        let paint_method = Id(7);

        let mut index = HashMap::new();

        index.insert(
            root,
            Item {
                id: root,
                crate_id: 0,
                name: Some("fixture".into()),
                span: None,
                visibility: Visibility::Public,
                docs: Some("Fixture root module".into()),
                links: HashMap::new(),
                attrs: Vec::new(),
                deprecation: None,
                inner: ItemEnum::Module(Module {
                    is_crate: true,
                    items: vec![widget, helper_fn, paintable_trait, widget_impl],
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
                docs: Some("Widget docs highlight the component".into()),
                links: HashMap::new(),
                attrs: Vec::new(),
                deprecation: None,
                inner: ItemEnum::Struct(Struct {
                    kind: StructKind::Plain {
                        fields: vec![widget_field],
                        has_stripped_fields: false,
                    },
                    generics: empty_generics(),
                    impls: vec![widget_impl],
                }),
            },
        );

        index.insert(
            widget_field,
            Item {
                id: widget_field,
                crate_id: 0,
                name: Some("id".into()),
                span: None,
                visibility: Visibility::Public,
                docs: Some("Identifier for Widget".into()),
                links: HashMap::new(),
                attrs: Vec::new(),
                deprecation: None,
                inner: ItemEnum::StructField(Type::Primitive("u32".into())),
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
                        output: Some(Type::Primitive("u32".into())),
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
                docs: Some("Helper docs mention Widget".into()),
                links: HashMap::new(),
                attrs: Vec::new(),
                deprecation: None,
                inner: ItemEnum::Function(Function {
                    sig: FunctionSignature {
                        inputs: vec![("count".into(), Type::Primitive("i32".into()))],
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
            paintable_trait,
            Item {
                id: paintable_trait,
                crate_id: 0,
                name: Some("Paintable".into()),
                span: None,
                visibility: Visibility::Public,
                docs: Some("Paintable trait handles colors".into()),
                links: HashMap::new(),
                attrs: Vec::new(),
                deprecation: None,
                inner: ItemEnum::Trait(Trait {
                    is_auto: false,
                    is_unsafe: false,
                    is_dyn_compatible: true,
                    items: vec![paint_method],
                    generics: empty_generics(),
                    bounds: Vec::new(),
                    implementations: Vec::new(),
                }),
            },
        );

        index.insert(
            paint_method,
            Item {
                id: paint_method,
                crate_id: 0,
                name: Some("paint".into()),
                span: None,
                visibility: Visibility::Public,
                docs: Some("Paint method docs".into()),
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
                        output: None,
                        is_c_variadic: false,
                    },
                    generics: empty_generics(),
                    header: default_header(),
                    has_body: false,
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

    fn build_index() -> SearchIndex {
        let crate_data = fixture_crate();
        SearchIndex::build(&crate_data, false)
    }

    #[test]
    fn name_domain_matches_impl_method() {
        let index = build_index();
        let mut options = SearchOptions::new("render");
        options.domains = SearchDomain::NAMES;
        let results = index.search(&options);
        assert!(results.iter().any(|r| r.raw_name == "render"));
        assert!(
            results
                .iter()
                .all(|r| r.matched.contains(SearchDomain::NAMES))
        );
    }

    #[test]
    fn multi_domain_hits_report_all_matches() {
        let index = build_index();
        let mut options = SearchOptions::new("Widget");
        options.domains = SearchDomain::NAMES | SearchDomain::DOCS;
        let results = index.search(&options);
        let widget = results
            .into_iter()
            .find(|r| r.raw_name == "Widget")
            .expect("Widget result");
        assert!(widget.matched.contains(SearchDomain::NAMES));
        assert!(widget.matched.contains(SearchDomain::DOCS));
    }

    #[test]
    fn default_domains_exclude_paths() {
        let defaults = SearchDomain::default();
        assert!(defaults.contains(SearchDomain::NAMES));
        assert!(defaults.contains(SearchDomain::DOCS));
        assert!(defaults.contains(SearchDomain::SIGNATURES));
        assert!(!defaults.contains(SearchDomain::PATHS));
    }

    #[test]
    fn path_domain_matches_impl_member() {
        let index = build_index();
        let mut options = SearchOptions::new("fixture::Widget::render");
        options.domains = SearchDomain::PATHS;
        let results = index.search(&options);
        assert!(results.iter().any(|r| r.raw_name == "render"));
    }

    #[test]
    fn signature_domain_matches_free_function() {
        let index = build_index();
        let mut options = SearchOptions::new("fn helper");
        options.domains = SearchDomain::SIGNATURES;
        let results = index.search(&options);
        assert!(results.iter().any(|r| r.raw_name == "helper"));
    }

    #[test]
    fn case_sensitive_toggle_affects_results() {
        let index = build_index();
        let mut options = SearchOptions::new("widget docs");
        options.domains = SearchDomain::DOCS;
        options.case_sensitive = true;
        assert!(index.search(&options).is_empty());
        options.case_sensitive = false;
        assert!(!index.search(&options).is_empty());
    }

    #[test]
    fn negative_query_returns_empty() {
        let index = build_index();
        let options = SearchOptions::new("missing");
        assert!(index.search(&options).is_empty());
    }

    #[test]
    fn describe_domains_lists_selected_flags() {
        assert_eq!(
            super::describe_domains(SearchDomain::empty()),
            Vec::<&str>::new()
        );
        assert_eq!(super::describe_domains(SearchDomain::NAMES), vec!["name"]);
        assert_eq!(
            super::describe_domains(SearchDomain::NAMES | SearchDomain::DOCS),
            vec!["name", "doc"]
        );
    }
}
