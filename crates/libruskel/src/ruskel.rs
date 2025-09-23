use rustdoc_types::Crate;

use super::{
    cargoutils::*,
    error::*,
    frontmatter::{FrontmatterConfig, FrontmatterHit, FrontmatterSearch},
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
        let rt = resolve_target(target, self.offline)?;
        rt.read_crate(
            no_default_features,
            all_features,
            features,
            private_items,
            self.silent,
        )
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
        let rt = resolve_target(target, self.offline)?;
        let crate_data = rt.read_crate(
            no_default_features,
            all_features,
            features,
            options.include_private,
            self.silent,
        )?;

        let index = SearchIndex::build(&crate_data, options.include_private);
        let results = index.search(options);

        if results.is_empty() {
            return Ok(SearchResponse {
                results,
                rendered: String::new(),
            });
        }

        let selection = build_render_selection(&index, &results, options.expand_containers);
        let mut renderer = Renderer::default()
            .with_filter(&rt.filter)
            .with_auto_impls(self.auto_impls)
            .with_private_items(options.include_private)
            .with_selection(selection);
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
            let filter = if rt.filter.is_empty() {
                None
            } else {
                Some(rt.filter)
            };
            let frontmatter = FrontmatterConfig::for_target(target.to_string())
                .with_filter(filter)
                .with_search(search_meta);
            renderer = renderer.with_frontmatter(frontmatter);
        }
        let rendered = renderer.render(&crate_data)?;

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
        let include_private = include_private
            || search
                .map(|options| options.include_private)
                .unwrap_or(false);

        let rt = resolve_target(target, self.offline)?;
        let crate_data = rt.read_crate(
            no_default_features,
            all_features,
            features,
            include_private,
            self.silent,
        )?;

        let index = SearchIndex::build(&crate_data, include_private);

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

        results.retain(|item| item.kind != SearchItemKind::Use);

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
        let rt = resolve_target(target, self.offline)?;
        let crate_data = rt.read_crate(
            no_default_features,
            all_features,
            features,
            true,
            self.silent,
        )?;

        let mut renderer = Renderer::default()
            .with_filter(&rt.filter)
            .with_auto_impls(self.auto_impls)
            .with_private_items(private_items);
        if self.frontmatter {
            let filter = if rt.filter.is_empty() {
                None
            } else {
                Some(rt.filter)
            };
            let frontmatter = FrontmatterConfig::for_target(target.to_string()).with_filter(filter);
            renderer = renderer.with_frontmatter(frontmatter);
        }

        let rendered = renderer.render(&crate_data)?;

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
}
