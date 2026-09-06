//! Item-ID selections shared by exact-path filtering and search rendering.

use std::collections::{HashMap, HashSet};

use rustdoc_types::{Crate, Id, Impl, Item, ItemEnum, StructKind, VariantKind, Visibility};

use crate::{
    error::{Result, RuskelError},
    search::{SearchIndex, SearchItemKind, SearchResult},
};

/// Traits rendered as `#[derive(...)]` instead of explicit impl blocks.
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
    "Serialize",
    "Deserialize",
];

/// One parent-to-child occurrence in the render traversal.
pub type RenderEdge = (Option<Id>, Id);

/// Selection of item identifiers used when rendering subsets of a crate.
#[derive(Debug, Clone, Default)]
pub struct RenderSelection {
    /// Selection metadata keyed by item identifier.
    entries: HashMap<Id, SelectionFlags>,
    /// Optional occurrence-level restriction for aliased and directly defined
    /// items.
    edges: Option<HashSet<RenderEdge>>,
    /// Optional occurrence-level module documentation decisions.
    module_docs: Option<HashSet<RenderEdge>>,
}

/// Flags describing how a specific item participates in a render selection.
#[derive(Debug, Clone, Copy, Default)]
struct SelectionFlags {
    /// The item is an explicit match.
    matched: bool,
    /// The item is retained to preserve hierarchy context.
    in_context: bool,
    /// The item should expand to include all of its children.
    expanded: bool,
}

impl RenderSelection {
    /// Create a selection from explicit search matches and context sets.
    pub(crate) fn new(
        matches: HashSet<Id>,
        mut context: HashSet<Id>,
        expanded: HashSet<Id>,
    ) -> Self {
        for id in &matches {
            context.insert(*id);
        }

        let mut entries: HashMap<Id, SelectionFlags> = HashMap::new();
        for id in context {
            entries.entry(id).or_default().in_context = true;
        }
        for id in matches {
            entries.entry(id).or_default().matched = true;
        }
        for id in expanded {
            entries.entry(id).or_default().expanded = true;
        }

        Self {
            entries,
            edges: None,
            module_docs: None,
        }
    }

    /// Build a selection for one exact render-visible path.
    pub(crate) fn for_filter(
        crate_data: &Crate,
        filter: &str,
        render_private_items: bool,
        render_auto_impls: bool,
        render_blanket_impls: bool,
    ) -> Result<Self> {
        FilterIndex::build(
            crate_data,
            render_private_items,
            render_auto_impls,
            render_blanket_impls,
        )
        .select(filter)
    }

    /// Restrict this selection to occurrences admitted by another selection.
    pub(crate) fn restrict_to(mut self, restriction: &Self) -> Self {
        self.entries.retain(|id, flags| {
            if !restriction.in_context(id) {
                return false;
            }
            flags.matched &= restriction.in_context(id);
            flags.expanded &= restriction.in_context(id);
            true
        });
        self.edges = restriction.edges.clone();
        self.module_docs = restriction.module_docs.clone();
        self
    }

    /// Whether this selection retains an explicit match from another selection.
    pub(crate) fn retains_match_from(&self, other: &Self) -> bool {
        other
            .entries
            .iter()
            .any(|(id, flags)| flags.matched && self.in_context(id))
    }

    /// Is the item an explicit match?
    pub(crate) fn is_match(&self, id: &Id) -> bool {
        self.entries.get(id).is_some_and(|flags| flags.matched)
    }

    /// Is the item retained to preserve hierarchy context?
    pub(crate) fn in_context(&self, id: &Id) -> bool {
        self.entries.get(id).is_some_and(|flags| flags.in_context)
    }

    /// Is this concrete parent-to-child occurrence retained?
    pub(crate) fn allows_item(&self, parent: Option<Id>, id: &Id) -> bool {
        self.in_context(id)
            && self
                .edges
                .as_ref()
                .is_none_or(|edges| edges.contains(&(parent, *id)))
    }

    /// Should the item's children be fully expanded?
    pub(crate) fn is_expanded(&self, id: &Id) -> bool {
        self.entries.get(id).is_some_and(|flags| flags.expanded)
    }

    /// Should this concrete module occurrence emit its documentation?
    pub(crate) fn renders_module_docs(&self, parent: Option<Id>, id: &Id) -> bool {
        self.module_docs
            .as_ref()
            .is_none_or(|modules| modules.contains(&(parent, *id)))
    }
}

/// Build a renderer selection set covering search matches and their context.
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
                        | SearchItemKind::Enum
                        | SearchItemKind::Union
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

/// Apply the renderer's impl visibility policy.
pub fn should_render_impl(
    impl_: &Impl,
    render_auto_impls: bool,
    render_blanket_impls: bool,
) -> bool {
    if impl_.is_synthetic && !render_auto_impls {
        return false;
    }
    if derive_trait_name(impl_).is_some() {
        return false;
    }
    render_blanket_impls || impl_.blanket_impl.is_none()
}

/// Return the short name of a trait rendered through `#[derive(...)]`.
pub fn derive_trait_name(impl_: &Impl) -> Option<&str> {
    let name = impl_.trait_.as_ref()?.path.split("::").last()?;
    DERIVE_TRAITS.contains(&name).then_some(name)
}

/// One render-visible path and the item-ID chain needed to reach it.
#[derive(Debug)]
pub struct RenderPathRecord {
    /// Path below the crate root.
    pub(crate) path: Vec<String>,
    /// Concrete item occurrences needed to render this record.
    pub(crate) chain: Vec<RenderEdge>,
    /// Item at the end of the record.
    pub(crate) id: Id,
    /// Whether the record is a module occurrence.
    is_module: bool,
    /// Whether exact filtering must force all structural children.
    expand_when_matched: bool,
}

/// Render-visible paths collected before exact filtering.
struct FilterIndex {
    /// Every direct and re-exported path the renderer can visit.
    records: Vec<RenderPathRecord>,
}

impl FilterIndex {
    /// Build the render path index for one crate.
    fn build(
        crate_data: &Crate,
        render_private_items: bool,
        render_auto_impls: bool,
        render_blanket_impls: bool,
    ) -> Self {
        let mut builder = FilterIndexBuilder {
            crate_data,
            render_private_items,
            render_auto_impls,
            render_blanket_impls,
            records: Vec::new(),
            chain: Vec::new(),
            active: Vec::new(),
        };
        builder.visit_item(crate_data.root, None, &[]);
        Self {
            records: builder.records,
        }
    }

    /// Select one exact path and all compatible ancestor and descendant
    /// occurrences.
    fn select(self, filter: &str) -> Result<RenderSelection> {
        let components: Vec<String> = filter.split("::").map(ToOwned::to_owned).collect();
        if components.is_empty() || !self.records.iter().any(|record| record.path == components) {
            return Err(RuskelError::FilterNotMatched(filter.to_string()));
        }

        let mut matches = HashSet::new();
        let mut context = HashSet::new();
        let mut expanded = HashSet::new();
        let mut edges = HashSet::new();
        let mut module_docs = HashSet::new();

        for record in &self.records {
            let is_match = record.path == components;
            let is_ancestor = components.starts_with(&record.path);
            let is_descendant = record.path.starts_with(&components);
            if !(is_match || is_ancestor || is_descendant) {
                continue;
            }

            if is_match {
                matches.insert(record.id);
                if record.expand_when_matched {
                    expanded.insert(record.id);
                }
            }
            for edge in &record.chain {
                context.insert(edge.1);
                edges.insert(*edge);
            }
            if record.is_module
                && is_descendant
                && !record.path.is_empty()
                && let Some(edge) = record.chain.last()
            {
                module_docs.insert(*edge);
            }
        }

        let mut selection = RenderSelection::new(matches, context, expanded);
        selection.edges = Some(edges);
        selection.module_docs = Some(module_docs);
        Ok(selection)
    }
}

/// Recursive collector that mirrors the renderer's item traversal.
struct FilterIndexBuilder<'a> {
    /// Rustdoc document being indexed.
    crate_data: &'a Crate,
    /// Whether glob expansion may include private children.
    render_private_items: bool,
    /// Whether synthetic impls are renderable.
    render_auto_impls: bool,
    /// Whether blanket impls are renderable.
    render_blanket_impls: bool,
    /// Collected render paths.
    records: Vec<RenderPathRecord>,
    /// Current concrete occurrence chain.
    chain: Vec<RenderEdge>,
    /// Item IDs on the active recursion path.
    active: Vec<Id>,
}

impl FilterIndexBuilder<'_> {
    /// Visit one ordinary render item.
    fn visit_item(&mut self, id: Id, parent: Option<Id>, prefix: &[String]) {
        self.visit_item_as(id, parent, prefix, None);
    }

    /// Visit one render item with an optional use-site name.
    fn visit_item_as(
        &mut self,
        id: Id,
        parent: Option<Id>,
        prefix: &[String],
        use_site_name: Option<&str>,
    ) {
        if self.active.contains(&id) {
            return;
        }
        let Some(item) = self.crate_data.index.get(&id) else {
            return;
        };

        let is_root = id == self.crate_data.root;
        let mut path = prefix.to_vec();
        if !is_root {
            let name = use_site_name
                .map(ToOwned::to_owned)
                .or_else(|| match &item.inner {
                    ItemEnum::Use(import) => Some(import.name.clone()),
                    _ => item.name.clone(),
                });
            if let Some(name) = name {
                path.push(name);
            }
        }

        self.active.push(id);
        self.chain.push((parent, id));
        self.records.push(RenderPathRecord {
            path: path.clone(),
            chain: self.chain.clone(),
            id,
            is_module: matches!(item.inner, ItemEnum::Module(_)),
            expand_when_matched: matches!(item.inner, ItemEnum::Enum(_)),
        });

        match &item.inner {
            ItemEnum::Module(module) => {
                for child in &module.items {
                    self.visit_item(*child, Some(id), &path);
                }
            }
            ItemEnum::Struct(struct_) => {
                match &struct_.kind {
                    StructKind::Unit => {}
                    StructKind::Tuple(fields) => {
                        for field in fields.iter().flatten() {
                            self.visit_item(*field, Some(id), &path);
                        }
                    }
                    StructKind::Plain { fields, .. } => {
                        for field in fields {
                            self.visit_item(*field, Some(id), &path);
                        }
                    }
                }
                for impl_id in &struct_.impls {
                    self.visit_impl(*impl_id, id, &path);
                }
            }
            ItemEnum::Enum(enum_) => {
                for variant in &enum_.variants {
                    self.visit_item(*variant, Some(id), &path);
                }
                for impl_id in &enum_.impls {
                    self.visit_impl(*impl_id, id, &path);
                }
            }
            ItemEnum::Variant(variant) => match &variant.kind {
                VariantKind::Plain => {}
                VariantKind::Tuple(fields) => {
                    for field in fields.iter().flatten() {
                        self.visit_item(*field, Some(id), &path);
                    }
                }
                VariantKind::Struct { fields, .. } => {
                    for field in fields {
                        self.visit_item(*field, Some(id), &path);
                    }
                }
            },
            ItemEnum::Trait(trait_) => {
                for child in &trait_.items {
                    self.visit_item(*child, Some(id), &path);
                }
            }
            ItemEnum::Use(import) => self.visit_use(item, import, prefix),
            _ => {}
        }

        self.chain.pop();
        self.active.pop();
    }

    /// Visit a resolved import using the same inlining rules as the renderer.
    fn visit_use(&mut self, item: &Item, import: &rustdoc_types::Use, prefix: &[String]) {
        let Some(imported_id) = import.id else {
            return;
        };
        if import.is_glob {
            let Some(source) = self.crate_data.index.get(&imported_id) else {
                return;
            };
            let ItemEnum::Module(module) = &source.inner else {
                return;
            };
            for child_id in &module.items {
                let Some(child) = self.crate_data.index.get(child_id) else {
                    continue;
                };
                if self.render_private_items
                    || matches!(child.visibility, Visibility::Public | Visibility::Default)
                {
                    self.visit_item(*child_id, Some(item.id), prefix);
                }
            }
        } else {
            self.visit_item_as(imported_id, Some(item.id), prefix, Some(&import.name));
        }
    }

    /// Visit a nameless impl and its path-addressable members.
    fn visit_impl(&mut self, id: Id, parent: Id, target_path: &[String]) {
        if self.active.contains(&id) {
            return;
        }
        let Some(item) = self.crate_data.index.get(&id) else {
            return;
        };
        let ItemEnum::Impl(impl_) = &item.inner else {
            return;
        };
        if !should_render_impl(impl_, self.render_auto_impls, self.render_blanket_impls) {
            return;
        }

        self.active.push(id);
        self.chain.push((Some(parent), id));
        for child in &impl_.items {
            self.visit_item(*child, Some(id), target_path);
        }
        self.chain.pop();
        self.active.pop();
    }
}

/// Collect direct and use-site paths using the renderer's default impl policy.
pub fn render_paths(crate_data: &Crate, render_private_items: bool) -> Vec<RenderPathRecord> {
    FilterIndex::build(crate_data, render_private_items, false, false).records
}
