# ruskel

[![Crates.io](https://img.shields.io/crates/v/libruskel.svg)](https://crates.io/crates/libruskel)
[![Documentation](https://docs.rs/libruskel/badge.svg)](https://docs.rs/libruskel)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

Ruskel generates skeletonized outlines of Rust crates. It renders a
single-page representation of a crate's public API with all implementation
omitted, while still producing syntactically correct Rust. 

Ruskel has two main uses:

- To provide quick access to Rust documentation from the command line.
- To export the full public API of a crate as a single file to pass to LLMs and
  other tools.


## Features

- Generate a skeletonized view of any Rust crate
- Support for local crates and remote crates from crates.io
- Syntax highlighting for terminal output 
- Option to output raw JSON data for further processing
- Configurable to include private items and auto-implemented traits
- Support for custom feature flags


## ruskel command line tool

`ruskel` is the command-line interface for easy use of the Ruskel functionality.

```sh
cargo install ruskel
```

Because Ruskel uses nightly-only features on `cargo doc`, you need to have the
nightly toolchain installed.


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


