//! Ruskel is a tool for generating skeletonized versions of Rust crates.
//!
//! It produces a single-page, syntactically valid Rust code representation of a crate,
//! with all implementations omitted. This provides a clear overview of the crate's structure
//! and public API.
//!
//! Ruskel works by first fetching all dependencies, then using the nightly Rust toolchain
//! to generate JSON documentation data. This data is then parsed and rendered into
//! the skeletonized format. The skeltonized code is then formatted with rustfmt, and optionally
//! has syntax highlighting applied.
//!
//!
//! You must have the nightly Rust toolchain installed to use (but not to install) RUskel.
use std::fs;
use syntect::{
    easy::HighlightLines,
    highlighting::ThemeSet,
    parsing::SyntaxSet,
    util::{as_24_bit_terminal_escaped, LinesWithEndings},
};

use rustdoc_types::Crate;

mod cargoutils;
mod crateutils;
mod error;
mod render;

pub use crate::error::{Result, RuskelError};
pub use crate::render::Renderer;
use cargoutils::*;

/// Ruskel generates a skeletonized version of a Rust crate in a single page.
/// It produces syntactically valid Rust code with all implementations omitted.
///
/// The tool performs a 'cargo fetch' to ensure all referenced code is available locally,
/// then uses 'cargo doc' with the nightly toolchain to generate JSON output. This JSON
/// is parsed and used to render the skeletonized code. Users must have the nightly
/// Rust toolchain installed and available.
#[derive(Debug)]
pub struct Ruskel {
    /// The user's full target specification
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
    /// Creates a new Ruskel instance for the specified target.
    ///
    /// The target can be:
    /// - A path to a Rust file
    /// - A directory containing a Cargo.toml file
    /// - A module name (with or without path)
    /// - A package name (with or without path)
    /// - A fully qualified path to an item within a module
    /// - Blank, in which case we use the current directory
    ///
    /// The method normalizes package names, converting hyphens to underscores for internal use.
    ///
    /// # Examples of valid targets:
    ///
    /// - src/lib.rs
    /// - my_module
    /// - serde
    /// - rustdoc-types
    /// - serde::Deserialize
    /// - rustdoc-types::Crate
    /// - rustdoc_types::Crate
    ///
    /// The method will attempt to locate the appropriate Cargo.toml file and set up
    /// the filter for rendering based on the provided target.
    ///
    /// If offline is true, Ruskel will not attempt to fetch dependencies from the network.
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

    fn crate_from_package(&self, package_path: CargoPath) -> Result<Crate> {
        let json_path = rustdoc_json::Builder::default()
            .toolchain("nightly")
            .manifest_path(package_path.manifest_path())
            .document_private_items(true)
            .no_default_features(self.no_default_features)
            .all_features(self.all_features)
            .features(&self.features)
            .build()
            .map_err(|e| RuskelError::Generate(e.to_string()))?;
        let json_content = fs::read_to_string(&json_path)?;
        let crate_data: Crate = serde_json::from_str(&json_content)?;
        Ok(crate_data)
    }

    /// Generates and returns the parsed JSON representation of the crate's API.
    pub fn make_crate(&self) -> Result<Crate> {
        let (package_path, _) = self.find_cargo()?;
        self.crate_from_package(package_path)
    }

    fn find_cargo(&self) -> Result<(CargoPath, String)> {
        let (package_path, filter) = CargoPath::from_target(&self.target)?;
        let (package_path, filter) = if !filter.is_empty() {
            if let Some(cp) = package_path.find_dependency(&filter[0], self.offline)? {
                (cp, filter[1..].to_vec())
            } else {
                (package_path, filter)
            }
        } else {
            (package_path, filter)
        };
        Ok((package_path, filter.join("::")))
    }

    /// Generates a skeletonized version of the crate as a string of Rust code.
    pub fn render(&self, auto_impls: bool, private_items: bool) -> Result<String> {
        let (package_path, filter) = self.find_cargo()?;
        let crate_data = self.crate_from_package(package_path)?;

        let renderer = Renderer::default()
            .with_filter(&filter)
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
        Ok(serde_json::to_string_pretty(&self.make_crate()?)?)
    }
}
