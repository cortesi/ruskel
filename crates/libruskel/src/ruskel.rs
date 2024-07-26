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
#[derive(Debug)]
pub struct Ruskel {
    /// The full target specification
    target: String,

    /// Whether to build without default features.
    no_default_features: bool,

    /// Whether to build with all features.
    all_features: bool,

    /// List of specific features to enable.
    features: Vec<String>,

    /// Whether to apply syntax highlighting to the output.
    highlight: bool,

    /// In offline mode Ruskell will not attempt to fetch dependencies from the network.
    offline: bool,
}

impl Ruskel {
    /// Creates a new Ruskel instance for the specified target. A target specification is an
    /// entrypoint, followed by an optional path, whith components separated by '::'.
    ///
    ///   entrypoint::path
    ///
    /// A entrypoint can be:
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
    pub fn new(target: &str) -> Self {
        Ruskel {
            target: target.to_string(),
            no_default_features: false,
            all_features: false,
            features: Vec::new(),
            highlight: false,
            offline: false,
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

    /// Disables default features when building the target crate.
    pub fn with_no_default_features(mut self, value: bool) -> Self {
        self.no_default_features = value;
        self
    }

    /// Enables all features when building the target crate.
    pub fn with_all_features(mut self, value: bool) -> Self {
        self.all_features = value;
        self
    }

    /// Enables a specific feature when building the target crate.
    pub fn with_feature(mut self, feature: String) -> Self {
        self.features.push(feature);
        self
    }

    /// Enables multiple specific features when building the target crate.
    pub fn with_features(mut self, features: Vec<String>) -> Self {
        self.features = features;
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
    pub fn inspect(&self) -> Result<Crate> {
        let rt = resolve_target(&self.target, self.offline)?;
        rt.read_crate(
            self.no_default_features,
            self.all_features,
            self.features.clone(),
        )
    }

    /// Generates a skeletonized version of the crate as a string of Rust code.
    pub fn render(&self, auto_impls: bool, private_items: bool) -> Result<String> {
        let rt = resolve_target(&self.target, self.offline)?;
        let crate_data = rt.read_crate(
            self.no_default_features,
            self.all_features,
            self.features.clone(),
        )?;

        let renderer = Renderer::default()
            .with_filter(&rt.filter)
            .with_auto_impls(auto_impls)
            .with_private_items(private_items);

        let rendered = renderer.render(&crate_data)?;

        if self.highlight {
            self.highlight_code(&rendered)
        } else {
            Ok(rendered)
        }
    }

    /// Returns a pretty-printed version of the crate's JSON representation.
    pub fn raw_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(&self.inspect()?)?)
    }
}
