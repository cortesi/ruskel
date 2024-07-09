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
//! Users of this module must have the nightly Rust toolchain installed and available.
//! The main entry point is the `Ruskel` struct, which provides methods for configuring
//! and executing the skeletonization process.

use std::fs;
use std::path::{Path, PathBuf};

use cargo::{core::Workspace, ops, util::context::GlobalContext};
use syntect::{
    easy::HighlightLines,
    highlighting::ThemeSet,
    parsing::SyntaxSet,
    util::{as_24_bit_terminal_escaped, LinesWithEndings},
};

use rustdoc_types::Crate;

mod error;
mod render;

pub use crate::error::{Result, RuskelError};
use crate::render::Renderer;

/// Ruskel generates a skeletonized version of a Rust crate in a single page.
/// It produces syntactically valid Rust code with all implementations omitted.
///
/// The tool performs a 'cargo fetch' to ensure all referenced code is available locally,
/// then uses 'cargo doc' with the nightly toolchain to generate JSON output. This JSON
/// is parsed and used to render the skeletonized code. Users must have the nightly
/// Rust toolchain installed and available.
#[derive(Debug)]
pub struct Ruskel {
    /// Path to the Cargo.toml file for the target crate.
    manifest_path: PathBuf,

    /// Whether to build without default features.
    no_default_features: bool,

    /// Whether to build with all features.
    all_features: bool,

    /// List of specific features to enable.
    features: Vec<String>,

    /// Whether to apply syntax highlighting to the output.
    highlight: bool,
}

impl Ruskel {
    /// Creates a new Ruskel instance for the specified target.
    /// The target can be a path to a Rust file, directory, or a module name.
    pub fn new(target: &str) -> Result<Self> {
        let target_path = PathBuf::from(target);

        if target_path.exists() {
            let canonical_path = target_path.canonicalize()?;
            let manifest_path = Self::find_manifest(&canonical_path)?;

            Ok(Ruskel {
                manifest_path,
                no_default_features: false,
                all_features: false,
                features: Vec::new(),
                highlight: false,
            })
        } else {
            // Assume it's a module name if the path doesn't exist
            let workspace_root = Self::find_module(target)?;
            let manifest_path = workspace_root.join("Cargo.toml");

            Ok(Ruskel {
                manifest_path,
                no_default_features: false,
                all_features: false,
                features: Vec::new(),
                highlight: false,
            })
        }
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

    /// Generates and returns the parsed JSON representation of the crate's API.
    pub fn json(&self) -> Result<Crate> {
        let json_path = rustdoc_json::Builder::default()
            .toolchain("nightly")
            .manifest_path(&self.manifest_path)
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

    /// Generates a skeletonized version of the crate as a string of Rust code.
    pub fn render(&self, auto_impls: bool, private_items: bool) -> Result<String> {
        let renderer = Renderer::default()
            .with_auto_impls(auto_impls)
            .with_private_items(private_items);

        let crate_data = self.json()?;
        let rendered = renderer.render(&crate_data)?;

        if self.highlight {
            self.highlight_code(&rendered)
        } else {
            Ok(rendered)
        }
    }

    /// Returns a pretty-printed version of the crate's JSON representation.
    pub fn raw_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(&self.json()?)?)
    }

    fn find_module(module_name: &str) -> Result<PathBuf> {
        let config = GlobalContext::default().map_err(|e| RuskelError::Cargo(e.to_string()))?;
        let workspace = Workspace::new(&Path::new("Cargo.toml").canonicalize()?, &config)
            .map_err(|e| RuskelError::Cargo(e.to_string()))?;

        for package in workspace.members() {
            if package.name().as_str() == module_name {
                return Ok(package.manifest_path().parent().unwrap().to_path_buf());
            }
        }

        // Fetch all packages
        let options = ops::FetchOptions {
            gctx: &config,
            targets: vec![],
        };
        let (_, ps) =
            ops::fetch(&workspace, &options).map_err(|e| RuskelError::Cargo(e.to_string()))?;

        for i in ps.packages() {
            if i.name().as_str() == module_name {
                return Ok(i.manifest_path().parent().unwrap().to_path_buf());
            }
        }

        Err(RuskelError::ModuleNotFound(module_name.to_string()))
    }

    fn find_manifest(target_path: &Path) -> Result<PathBuf> {
        let mut path = if target_path.is_file() {
            target_path.parent().unwrap_or(Path::new("/")).to_path_buf()
        } else {
            target_path.to_path_buf()
        };

        loop {
            let manifest_path = path.join("Cargo.toml");
            if manifest_path.exists() {
                return Ok(manifest_path);
            }
            if !path.pop() {
                break;
            }
        }
        Err(RuskelError::ManifestNotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use tempfile::{tempdir, TempDir};

    macro_rules! assert_path_eq {
        ($left:expr, $right:expr) => {
            assert_eq!(
                $left.canonicalize().unwrap(),
                $right.canonicalize().unwrap()
            )
        };
    }

    fn create_cargo_ws(dir: &Path) -> std::io::Result<()> {
        let content = "[workspace]\nmembers = [\"member1\", \"member2\"]";
        fs::write(dir.join("Cargo.toml"), content)
    }

    fn create_cargo_child(dir: &Path, name: &str) -> std::io::Result<()> {
        let content = format!("[package]\nname = \"{}\"\nversion = \"0.1.0\"", name);
        fs::write(dir.join("Cargo.toml"), content)
    }

    fn setup_workspace() -> Result<TempDir> {
        let temp_dir = tempdir()?;
        create_cargo_ws(temp_dir.path())?;

        let member1_dir = temp_dir.path().join("member1");
        fs::create_dir_all(member1_dir.join("src"))?;
        create_cargo_child(&member1_dir, "test-package1")?;
        File::create(member1_dir.join("src").join("lib.rs"))?;

        let member2_dir = temp_dir.path().join("member2");
        fs::create_dir_all(member2_dir.join("src"))?;
        create_cargo_child(&member2_dir, "test-package2")?;
        File::create(member2_dir.join("src").join("main.rs"))?;

        Ok(temp_dir)
    }

    #[test]
    fn test_parse_rust_file_in_workspace() -> Result<()> {
        let temp_dir = setup_workspace()?;
        let lib_rs_path = temp_dir.path().join("member1").join("src").join("lib.rs");

        // Ensure the file exists
        assert!(lib_rs_path.exists(), "lib.rs file does not exist");

        let target = Ruskel::new(lib_rs_path.to_str().unwrap())?;
        assert_path_eq!(
            target.manifest_path,
            temp_dir.path().join("member1").join("Cargo.toml")
        );
        Ok(())
    }

    #[test]
    fn test_parse_nonexistent_path() {
        let result = Ruskel::new("/path/does/not/exist");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_standalone_crate() -> Result<()> {
        let temp_dir = tempdir()?;
        create_cargo_child(temp_dir.path(), "test1")?;
        let src_dir = temp_dir.path().join("src");
        fs::create_dir(&src_dir)?;
        File::create(src_dir.join("lib.rs"))?;

        let target = Ruskel::new(temp_dir.path().to_str().unwrap())?;
        assert_path_eq!(target.manifest_path, temp_dir.path().join("Cargo.toml"));

        Ok(())
    }

    #[test]
    fn test_parse_workspace_root() -> Result<()> {
        let temp_dir = setup_workspace()?;

        let target = Ruskel::new(temp_dir.path().to_str().unwrap())?;
        assert_path_eq!(target.manifest_path, temp_dir.path().join("Cargo.toml"));

        Ok(())
    }

    #[test]
    fn test_parse_workspace_member() -> Result<()> {
        let temp_dir = setup_workspace()?;
        let member1_dir = temp_dir.path().join("member1");

        let target = Ruskel::new(member1_dir.to_str().unwrap())?;
        assert_path_eq!(target.manifest_path, member1_dir.join("Cargo.toml"));

        Ok(())
    }
}
