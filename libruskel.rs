pub mod libruskel {
    //! Ruskel generates skeletonized versions of Rust crates.
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

    pub type Result<T> = std::result::Result<T, RuskelError>;

    pub enum RuskelError {
        /// Indicates that a specified module could not be found.
        ModuleNotFound(String),
        /// Indicates a failure in reading a file, wrapping the underlying IO error.
        FileRead(std::io::Error),
        /// Indicates a failure in the code generation process.
        Generate(String),
        /// Indicates an error occurred while executing a Cargo command.
        Cargo(String),
        /// Indicates an error occurred during code formatting.
        Format(String),
        /// Indicates an error occurred during syntax highlighting.
        Highlight(String),
        /// The specified filter did not match any items.
        FilterNotMatched(String),
        /// Failed to parse a Cargo.toml manifest
        ManifestParse(String),
        /// Indicates that the Cargo.toml manifest file could not be found in the current directory or any parent directories.
        ManifestNotFound,
        /// Indicates an invalid version string was provided.
        InvalidVersion(String),
        /// Indicates an invalid target specification was provided.
        InvalidTarget(String),
        /// Indicates a dependency was not found in the registry.
        DependencyNotFound,
        /// A catch-all for other Cargo-related errors.
        CargoError(String),
    }

    pub struct Renderer {}

    impl Renderer {
        pub fn new() -> Self {}

        pub fn with_filter(self, filter: &str) -> Self {}

        pub fn with_blanket_impls(self, render_blanket_impls: bool) -> Self {}

        pub fn with_auto_impls(self, render_auto_impls: bool) -> Self {}

        pub fn with_private_items(self, render_private_items: bool) -> Self {}

        pub fn render(&self, crate_data: &Crate) -> Result<String> {}
    }

    impl Default for Renderer {
        fn default() -> Self {}
    }

    /// Ruskel generates a skeletonized version of a Rust crate in a single page.
    /// It produces syntactically valid Rust code with all implementations omitted.
    ///
    /// The tool performs a 'cargo fetch' to ensure all referenced code is available locally,
    /// then uses 'cargo doc' with the nightly toolchain to generate JSON output. This JSON
    /// is parsed and used to render the skeletonized code. Users must have the nightly
    /// Rust toolchain installed and available.
    pub struct Ruskel {}

    impl Ruskel {
        /// Creates a new Ruskel instance for the specified target. A target specification is an
        /// entrypoint, followed by an optional path, whith components separated by '::'.
        ///
        ///   entrypoint[::path]
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
        pub fn new(target: &str) -> Self {}

        /// Enables or disables offline mode, which prevents Ruskel from fetching dependencies from the
        /// network.
        pub fn with_offline(self, offline: bool) -> Self {}

        /// Enables or disables syntax highlighting in the output.
        pub fn with_highlighting(self, highlight: bool) -> Self {}

        /// Disables default features when building the target crate.
        pub fn with_no_default_features(self, value: bool) -> Self {}

        /// Enables all features when building the target crate.
        pub fn with_all_features(self, value: bool) -> Self {}

        /// Enables a specific feature when building the target crate.
        pub fn with_feature(self, feature: String) -> Self {}

        /// Enables multiple specific features when building the target crate.
        pub fn with_features(self, features: Vec<String>) -> Self {}

        /// Returns the parsed representation of the crate's API.
        pub fn inspect(&self) -> Result<Crate> {}

        /// Generates a skeletonized version of the crate as a string of Rust code.
        pub fn render(&self, auto_impls: bool, private_items: bool) -> Result<String> {}

        /// Returns a pretty-printed version of the crate's JSON representation.
        pub fn raw_json(&self) -> Result<String> {}
    }

    impl Debug for Ruskel {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {}
    }
}

