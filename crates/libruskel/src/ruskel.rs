use rustdoc_types::Crate;

use super::{
    cargoutils::*,
    error::*,
    frontmatter::{FrontmatterBinaryTarget, FrontmatterConfig, FrontmatterHit, FrontmatterSearch},
    render::*,
    search::{
        ListItem, SearchIndex, SearchItemKind, SearchOptions, SearchResponse,
        build_render_selection,
    },
};

/// Ruskel generates a skeletonized version of a Rust crate in a single page.
/// It produces syntactically valid Rust code with all implementations omitted.
///
/// The tool performs a 'cargo fetch' to ensure all referenced code is available locally,
/// then uses 'cargo doc' with the nightly toolchain to generate JSON output. This JSON
/// is parsed and used to render the skeletonized code. Users must have the nightly
/// Rust toolchain installed and available.
#[derive(Debug, Clone)]
pub struct Ruskel {
    /// In offline mode Ruskel will not attempt to fetch dependencies from the network.
    offline: bool,

    /// Whether to render auto-implemented traits.
    auto_impls: bool,

    /// Whether to suppress output during processing.
    silent: bool,

    /// Whether to emit frontmatter comments with rendered output.
    frontmatter: bool,

    /// Optional binary target override for bin-only crates or bin rendering.
    bin_target: Option<String>,
}

/// Drop `use` matches when more specific items are present.
fn prune_redundant_use_items(results: &mut Vec<ListItem>) {
    let has_non_use = results
        .iter()
        .any(|item| !matches!(item.kind, SearchItemKind::Use | SearchItemKind::Crate));

    if has_non_use {
        results.retain(|item| item.kind != SearchItemKind::Use);
    }
}

/// Crate data loaded for a resolved target together with render metadata.
struct LoadedTarget {
    /// Resolved package location and intra-crate filter path.
    resolved_target: ResolvedTarget,
    /// Parsed rustdoc JSON for the selected target.
    crate_data: Crate,
    /// Binary target metadata for bin rendering and frontmatter output.
    bin_target: Option<BinaryTarget>,
    /// Effective private-item visibility after accounting for bin-only targets.
    include_private: bool,
}

impl Default for Ruskel {
    fn default() -> Self {
        Self::new()
    }
}

impl Ruskel {
    /// Creates a new Ruskel instance with default configuration.
    ///
    /// # Target Format
    ///
    /// A target specification is an entrypoint, followed by an optional path, with components
    /// separated by '::'.
    ///
    ///   entrypoint::path
    ///
    /// An entrypoint can be:
    ///
    /// - A path to a Rust file
    /// - A directory containing a Cargo.toml file
    /// - A module name
    /// - A package name. In this case the name can also include a version number, separated by an
    ///   '@' symbol.
    ///
    /// The path is a fully qualified path within the entrypoint.
    ///
    /// # Examples of valid targets:
    ///
    /// - src/lib.rs
    /// - my_module
    /// - serde
    /// - rustdoc-types
    /// - serde::Deserialize
    /// - serde@1.0
    /// - rustdoc-types::Crate
    /// - rustdoc_types::Crate
    pub fn new() -> Self {
        Self {
            offline: false,
            auto_impls: false,
            silent: false,
            frontmatter: true,
            bin_target: None,
        }
    }

    /// Enables or disables offline mode, which prevents Ruskel from fetching dependencies from the
    /// network.
    pub fn with_offline(mut self, offline: bool) -> Self {
        self.offline = offline;
        self
    }

    /// Enables or disables rendering of auto-implemented traits.
    pub fn with_auto_impls(mut self, auto_impls: bool) -> Self {
        self.auto_impls = auto_impls;
        self
    }

    /// Enables or disables silent mode, which suppresses output during processing.
    pub fn with_silent(mut self, silent: bool) -> Self {
        self.silent = silent;
        self
    }

    /// Enables or disables frontmatter emission on rendered output.
    pub fn with_frontmatter(mut self, frontmatter: bool) -> Self {
        self.frontmatter = frontmatter;
        self
    }

    /// Overrides the binary target used when rendering a crate.
    pub fn with_bin_target(mut self, bin_target: Option<String>) -> Self {
        self.bin_target = bin_target;
        self
    }

    /// Returns the parsed representation of the crate's API.
    ///
    /// # Arguments
    /// * `target` - The target specification (see new() documentation for format)
    /// * `no_default_features` - Whether to build without default features
    /// * `all_features` - Whether to build with all features
    /// * `features` - List of specific features to enable
    /// * `private_items` - Whether to include private items in the output
    pub fn inspect(
        &self,
        target: &str,
        no_default_features: bool,
        all_features: bool,
        features: Vec<String>,
        private_items: bool,
    ) -> Result<Crate> {
        Ok(self
            .load_target(
                target,
                no_default_features,
                all_features,
                features,
                private_items,
                private_items,
            )?
            .crate_data)
    }

    /// Execute a search against the crate and return the matched items along with a rendered skeleton.
    ///
    /// The search respects the same target resolution logic as [`Self::render`], but only the
    /// matched items and their ancestors are emitted in the final skeleton.
    pub fn search(
        &self,
        target: &str,
        no_default_features: bool,
        all_features: bool,
        features: Vec<String>,
        options: &SearchOptions,
    ) -> Result<SearchResponse> {
        let loaded = self.load_target(
            target,
            no_default_features,
            all_features,
            features,
            options.include_private,
            options.include_private,
        )?;
        let index = SearchIndex::build(&loaded.crate_data, loaded.include_private);
        let results = index.search(options);

        if results.is_empty() {
            return Ok(SearchResponse {
                results,
                rendered: String::new(),
            });
        }

        let selection = build_render_selection(&index, &results, options.expand_containers);
        let mut renderer = self.base_renderer(&loaded).with_selection(selection);
        if self.frontmatter {
            let hits = results
                .iter()
                .map(|result| FrontmatterHit {
                    path: result.path_string.clone(),
                    domains: result.matched,
                })
                .collect();
            let search_meta = FrontmatterSearch {
                query: options.query.clone(),
                domains: options.domains,
                case_sensitive: options.case_sensitive,
                expand_containers: options.expand_containers,
                hits,
            };
            renderer = self.attach_frontmatter(renderer, &loaded, target, Some(search_meta));
        }
        let rendered = renderer.render(&loaded.crate_data)?;

        Ok(SearchResponse { results, rendered })
    }

    /// Produce a lightweight listing of crate items, optionally filtered by a search query.
    pub fn list(
        &self,
        target: &str,
        no_default_features: bool,
        all_features: bool,
        features: Vec<String>,
        include_private: bool,
        search: Option<&SearchOptions>,
    ) -> Result<Vec<ListItem>> {
        let include_private =
            include_private || search.is_some_and(|options| options.include_private);
        let loaded = self.load_target(
            target,
            no_default_features,
            all_features,
            features,
            include_private,
            include_private,
        )?;
        let index = SearchIndex::build(&loaded.crate_data, loaded.include_private);

        let mut results: Vec<ListItem> = if let Some(options) = search {
            index
                .search(options)
                .into_iter()
                .map(|result| ListItem {
                    kind: result.kind,
                    path: result.path_string,
                })
                .collect()
        } else {
            index
                .entries()
                .iter()
                .cloned()
                .map(|entry| ListItem {
                    kind: entry.kind,
                    path: entry.path_string,
                })
                .collect()
        };

        prune_redundant_use_items(&mut results);

        Ok(results)
    }

    /// Render the crate target into a Rust skeleton without filtering.
    pub fn render(
        &self,
        target: &str,
        no_default_features: bool,
        all_features: bool,
        features: Vec<String>,
        private_items: bool,
    ) -> Result<String> {
        let loaded = self.load_target(
            target,
            no_default_features,
            all_features,
            features,
            true,
            private_items,
        )?;
        let mut renderer = self.base_renderer(&loaded);
        if self.frontmatter {
            renderer = self.attach_frontmatter(renderer, &loaded, target, None);
        }

        let rendered = renderer.render(&loaded.crate_data)?;

        Ok(rendered)
    }

    /// Returns a pretty-printed version of the crate's JSON representation.
    ///
    /// # Arguments
    /// * `target` - The target specification (see new() documentation for format)
    /// * `no_default_features` - Whether to build without default features
    /// * `all_features` - Whether to build with all features
    /// * `features` - List of specific features to enable
    /// * `private_items` - Whether to include private items in the JSON output
    pub fn raw_json(
        &self,
        target: &str,
        no_default_features: bool,
        all_features: bool,
        features: Vec<String>,
        private_items: bool,
    ) -> Result<String> {
        Ok(serde_json::to_string_pretty(&self.inspect(
            target,
            no_default_features,
            all_features,
            features,
            private_items,
        )?)?)
    }

    /// Load crate data and normalize the privacy policy derived from the selected target.
    fn load_target(
        &self,
        target: &str,
        no_default_features: bool,
        all_features: bool,
        features: Vec<String>,
        document_private_items: bool,
        include_private: bool,
    ) -> Result<LoadedTarget> {
        let resolved_target = resolve_target(target, self.offline)?;
        let read_options = CrateReadOptions {
            no_default_features,
            all_features,
            features,
            private_items: document_private_items,
            silent: self.silent,
            offline: self.offline,
            bin_override: self.bin_target.clone(),
        };
        let CrateRead {
            crate_data,
            bin_target,
        } = resolved_target.read_crate(&read_options)?;
        let include_private =
            include_private || bin_target.as_ref().is_some_and(|target| target.is_bin_only);

        Ok(LoadedTarget {
            resolved_target,
            crate_data,
            bin_target,
            include_private,
        })
    }

    /// Create the renderer preconfigured with target filtering and visibility policy.
    fn base_renderer(&self, loaded: &LoadedTarget) -> Renderer {
        Renderer::default()
            .with_filter(&loaded.resolved_target.filter)
            .with_auto_impls(self.auto_impls)
            .with_private_items(loaded.include_private)
    }

    /// Attach frontmatter metadata to a renderer when enabled.
    fn attach_frontmatter(
        &self,
        renderer: Renderer,
        loaded: &LoadedTarget,
        target: &str,
        search: Option<FrontmatterSearch>,
    ) -> Renderer {
        let filter = (!loaded.resolved_target.filter.is_empty())
            .then(|| loaded.resolved_target.filter.clone());
        let mut frontmatter = FrontmatterConfig::for_target(target.to_string()).with_filter(filter);
        if let Some(search) = search {
            frontmatter = frontmatter.with_search(search);
        }
        if let Some(bin_target) = &loaded.bin_target {
            frontmatter = frontmatter.with_binary_target(FrontmatterBinaryTarget::new(
                bin_target.name.clone(),
                bin_target.is_bin_only,
            ));
        }
        renderer.with_frontmatter(frontmatter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list_item(kind: SearchItemKind, path: &str) -> ListItem {
        ListItem {
            kind,
            path: path.to_string(),
        }
    }

    #[test]
    fn keeps_use_entries_when_they_are_the_only_members() {
        let mut items = vec![
            list_item(SearchItemKind::Crate, "only_use"),
            list_item(SearchItemKind::Use, "only_use::Serialize"),
        ];

        prune_redundant_use_items(&mut items);

        assert_eq!(
            items,
            vec![
                list_item(SearchItemKind::Crate, "only_use"),
                list_item(SearchItemKind::Use, "only_use::Serialize"),
            ]
        );
    }

    #[test]
    fn removes_use_entries_when_other_items_are_present() {
        let mut items = vec![
            list_item(SearchItemKind::Crate, "widget"),
            list_item(SearchItemKind::Use, "widget::prelude"),
            list_item(SearchItemKind::Function, "widget::draw"),
        ];

        prune_redundant_use_items(&mut items);

        assert_eq!(
            items,
            vec![
                list_item(SearchItemKind::Crate, "widget"),
                list_item(SearchItemKind::Function, "widget::draw"),
            ]
        );
    }

    #[test]
    fn preserves_use_entries_when_no_crate_item_is_present() {
        let mut items = vec![list_item(SearchItemKind::Use, "widget::prelude")];

        prune_redundant_use_items(&mut items);

        assert_eq!(
            items,
            vec![list_item(SearchItemKind::Use, "widget::prelude")]
        );
    }
}
