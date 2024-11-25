# ruskel

[![Crates.io](https://img.shields.io/crates/v/libruskel.svg)](https://crates.io/crates/libruskel)
[![Documentation](https://docs.rs/libruskel/badge.svg)](https://docs.rs/libruskel)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

Ruskel renders a single-page representation of a crate's public API with all
implementation omitted, while still producing syntactically correct Rust. 

It has two main uses:

- To provide quick access to Rust documentation from the command line.
- To export the full public API of a crate as a single file to pass to LLMs and
  other tools.


## Features

- Generate a skeletonized view of any Rust crate
- Support for local crates and remote crates from crates.io
- Syntax highlighting for terminal output 
- Optionally include private items and auto-implemented traits
- Support for custom feature flags and version specification

<details>
<summary> Example output </summary>

## Command

Here is an example of of ruskel rendering the skeleton for its own library.

```bash
ruskel libruskel
```

## Output

```rust
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
    //! You must have the nightly Rust toolchain installed to use (but not to install) Ruskel.

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

        /// Apply a filter to output. The filter is a path BELOW the outermost module.
        pub fn with_filter(self, filter: &str) -> Self {}

        /// Render impl blocks for traits implemented for all types?
        pub fn with_blanket_impls(self, render_blanket_impls: bool) -> Self {}

        /// Render impl blocks for auto traits like Send and Sync?
        pub fn with_auto_impls(self, render_auto_impls: bool) -> Self {}

        /// Render private items?
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
        /// silent: if true, no output is printed
        pub fn inspect(&self, silent: bool) -> Result<Crate> {}

        /// Generates a skeletonized version of the crate as a string of Rust code.
        pub fn render(
            &self,
            auto_impls: bool,
            private_items: bool,
            silent: bool,
        ) -> Result<String> {
        }

        /// Returns a pretty-printed version of the crate's JSON representation.
        pub fn raw_json(&self) -> Result<String> {}
    }

    impl Debug for Ruskel {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {}
    }
}

```

</details>

## ruskel command line tool

`ruskel` is the command-line interface for easy use of the Ruskel functionality.

```sh
cargo install ruskel
```

Because Ruskel uses nightly-only features on `cargo doc`, you need to have the
nightly toolchain installed to run it, but not to install it.


### Usage

Basic usage:

```sh
ruskel [TARGET]
```

Where `TARGET` can be a directory, file path, or a module name. If omitted, it defaults to the current directory.

#### Sample Options

- `--all-features`: Enable all features
- `--auto-impls`: Render auto-implemented traits
- `--features <FEATURES>`: Specify features to enable (comma-separated)
- `--highlight`: Force enable syntax highlighting
- `--no-default-features`: Disable default features
- `--no-highlight`: Disable syntax highlighting
- `--no-page`: Disable paging
- `--offline`: Don't fetch from crates.io
- `--private`: Render private items
- `--quiet`: Suppress output while building docs

For full details, see:

```sh
ruskel --help
```

Ruskel has a flexible target specification that tries to do the right thing in a wide set of circumstances.

```sh
# Current project
ruskel

# If we're in a workspace and we have a crate mypacakage
ruskel mypackage

# A dependency of the current project, else we fetch from crates.io 
ruskel serde

# A sub-path within a crate
ruskel serde::de::Deserialize 

# Path to a crate
ruskel /my/path

# A module within that crate
ruskel /my/path::foo

# A crate from crates.io with a specific version
ruskel serde@1.0.0
```

## libruskel library

`libruskel` is a library that can be integrated into other Rust projects to provide Ruskel functionality.

Here's a basic example of using `libruskel` in your Rust code:

```rust
use libruskel::Ruskel;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rs = Ruskel::new("/path/to/target")?;
    let rendered = rs.render(false, false)?;
    println!("{}", rendered);
    Ok(())
}
```


