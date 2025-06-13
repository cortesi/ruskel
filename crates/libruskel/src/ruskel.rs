use syntect::{
    easy::HighlightLines,
    highlighting::ThemeSet,
    parsing::SyntaxSet,
    util::{as_24_bit_terminal_escaped, LinesWithEndings},
};

use rustdoc_types::Crate;

use super::{cargoutils::*, error::*, render::*};

/// Ruskel generates a skeletonized version of a Rust crate in a single page.
/// It produces syntactically valid Rust code with all implementations omitted.
///
/// The tool performs a 'cargo fetch' to ensure all referenced code is available locally,
/// then uses 'cargo doc' with the nightly toolchain to generate JSON output. This JSON
/// is parsed and used to render the skeletonized code. Users must have the nightly
/// Rust toolchain installed and available.
#[derive(Debug, Default)]
pub struct Ruskel {
    /// Whether to apply syntax highlighting to the output.
    highlight: bool,

    /// In offline mode Ruskell will not attempt to fetch dependencies from the network.
    offline: bool,

    /// Whether to render auto-implemented traits.
    auto_impls: bool,

    /// Whether to render private items.
    private_items: bool,

    /// Whether to suppress output during processing.
    silent: bool,
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
        Ruskel {
            highlight: false,
            offline: false,
            auto_impls: false,
            private_items: false,
            silent: false,
        }
    }

    /// Enables or disables offline mode, which prevents Ruskel from fetching dependencies from the
    /// network.
    pub fn with_offline(mut self, offline: bool) -> Self {
        self.offline = offline;
        self
    }

    /// Enables or disables syntax highlighting in the output.
    pub fn with_highlighting(mut self, highlight: bool) -> Self {
        self.highlight = highlight;
        self
    }

    /// Enables or disables rendering of auto-implemented traits.
    pub fn with_auto_impls(mut self, auto_impls: bool) -> Self {
        self.auto_impls = auto_impls;
        self
    }

    /// Enables or disables rendering of private items.
    pub fn with_private_items(mut self, private_items: bool) -> Self {
        self.private_items = private_items;
        self
    }

    /// Enables or disables silent mode, which suppresses output during processing.
    pub fn with_silent(mut self, silent: bool) -> Self {
        self.silent = silent;
        self
    }

    fn highlight_code(&self, code: &str) -> Result<String> {
        let ss = SyntaxSet::load_defaults_newlines();
        let ts = ThemeSet::load_defaults();

        let syntax = ss.find_syntax_by_extension("rs").unwrap();
        let mut h = HighlightLines::new(syntax, &ts.themes["Solarized (dark)"]);

        let mut output = String::new();
        for line in LinesWithEndings::from(code) {
            let ranges: Vec<(syntect::highlighting::Style, &str)> = h.highlight_line(line, &ss)?;
            let escaped = as_24_bit_terminal_escaped(&ranges[..], false);
            output.push_str(&escaped);
        }

        Ok(output)
    }

    /// Returns the parsed representation of the crate's API.
    ///
    /// # Arguments
    /// * `target` - The target specification (see new() documentation for format)
    /// * `no_default_features` - Whether to build without default features
    /// * `all_features` - Whether to build with all features
    /// * `features` - List of specific features to enable
    pub fn inspect(
        &self,
        target: &str,
        no_default_features: bool,
        all_features: bool,
        features: Vec<String>,
    ) -> Result<Crate> {
        let rt = resolve_target(target, self.offline)?;
        rt.read_crate(no_default_features, all_features, features, self.silent)
    }

    /// Generates a skeletonized version of the crate as a string of Rust code.
    ///
    /// # Arguments
    /// * `target` - The target specification (see new() documentation for format)
    /// * `no_default_features` - Whether to build without default features
    /// * `all_features` - Whether to build with all features
    /// * `features` - List of specific features to enable
    pub fn render(
        &self,
        target: &str,
        no_default_features: bool,
        all_features: bool,
        features: Vec<String>,
    ) -> Result<String> {
        let rt = resolve_target(target, self.offline)?;
        let crate_data = rt.read_crate(no_default_features, all_features, features, self.silent)?;

        let renderer = Renderer::default()
            .with_filter(&rt.filter)
            .with_auto_impls(self.auto_impls)
            .with_private_items(self.private_items);

        let rendered = renderer.render(&crate_data)?;

        if self.highlight {
            self.highlight_code(&rendered)
        } else {
            Ok(rendered)
        }
    }

    /// Returns a pretty-printed version of the crate's JSON representation.
    ///
    /// # Arguments
    /// * `target` - The target specification (see new() documentation for format)
    /// * `no_default_features` - Whether to build without default features
    /// * `all_features` - Whether to build with all features
    /// * `features` - List of specific features to enable
    pub fn raw_json(
        &self,
        target: &str,
        no_default_features: bool,
        all_features: bool,
        features: Vec<String>,
    ) -> Result<String> {
        Ok(serde_json::to_string_pretty(&self.inspect(
            target,
            no_default_features,
            all_features,
            features,
        )?)?)
    }
}
