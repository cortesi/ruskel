# ruskel

![Discord](https://img.shields.io/discord/1381424110831145070?style=flat-square&logo=rust&link=https%3A%2F%2Fdiscord.gg%2FfHmRmuBDxF)
[![Crates.io](https://img.shields.io/crates/v/libruskel.svg)](https://crates.io/crates/libruskel)
[![Documentation](https://docs.rs/libruskel/badge.svg)](https://docs.rs/libruskel)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

Ruskel produces a syntactically correct skeleton of a crate's public API: docs
included, implementation stripped. Crates not found locally are fetched from
[crates.io](https://crates.io).

Ruskel is great for:

- Quick access to Rust documentation from the command line.
- Exporting a crate's public API as a single file for LLMs and other tools.
- Standard library documentation (`std`, `core`, `alloc`), e.g. `ruskel std::vec::Vec`.

For example, here is the skeleton of the very tiny `termsize` crate:

<!-- snips: !cargo run --bin ruskel -- termsize -->
```rust
// Ruskel skeleton - syntactically valid Rust with implementation omitted.
// settings: target=termsize, visibility=public, auto_impls=false, blanket_impls=false

pub mod termsize {
    //! Termsize is a tiny crate that provides a simple
    //! interface for retrieving the current
    //! [terminal interface](http://www.manpagez.com/man/4/tty/) size
    //!
    //! ```rust
    //! extern crate termsize;
    //!
    //! termsize::get().map(|size| println!("rows {} cols {}", size.rows, size.cols));
    //! ```

    /// Container for number of rows and columns
    #[derive(Debug)]
    pub struct Size {
        /// number of rows
        pub rows: u16,
        /// number of columns
        pub cols: u16,
    }

    /// Gets the current terminal size
    pub fn get() -> Option<self::super::Size> {}
}
```

---

## Features

- Filter output to specific items with `--search`
- Tabular item listings with `--list`
- Syntax highlighting for terminal output
- Include private items and auto-implemented traits
- Custom feature flags and version specification


---

## Installation

Ruskel requires the Rust nightly toolchain to run. Install the nightly
toolchain and the `rust-docs-json` component:

```sh
rustup toolchain install nightly
rustup component add --toolchain nightly rust-docs-json
```

Install Ruskel:

```sh
cargo install ruskel
```

Ruskel requires nightly to run but can be installed with any toolchain.

---

## Usage


Basic usage:

```sh
ruskel [TARGET]
```

See the help output for all options:

```sh
ruskel --help
```

```sh
# Current project
ruskel

# A crate in the workspace
ruskel mypackage

# A dependency of the current project, or fetched from crates.io
ruskel serde

# A sub-path within a crate
ruskel serde::de::Deserialize

# Path to a crate
ruskel /my/path

# A module within that crate
ruskel /my/path::foo

# Specific version from crates.io
ruskel serde@1.0.0

# Search for "status" across names, signatures and doc comments
ruskel reqwest --search status

# Search for "status" in only names and signatures
ruskel reqwest --search status --search-spec name,signature

# Search for "status" in docs only
ruskel reqwest --search status --search-spec doc

# Access via std re-exports (recommended)
ruskel std::vec::Vec        # Vec type from std
ruskel std::rc::Rc          # Rc type from std
ruskel std::mem::size_of    # size_of function from std

# Direct access to core and alloc
ruskel core::mem            # Memory utilities from core
ruskel alloc::vec           # Vec module from alloc

# Entire crate
ruskel std                  # All of std
ruskel core                 # Core library (no_std compatible)
ruskel alloc                # Allocation library
```


---

## Search

Use `--search` to focus on specific items instead of rendering an entire crate.
The query runs across multiple domains and returns a skeleton containing only
the matches and their ancestors.

```sh
# Show methods and fields matching "status" within the reqwest crate
ruskel reqwest --search status --search-spec name,signature
```

By default the query matches name, doc, and signature domains, case-insensitively.
Use `--search-spec` to select domains (e.g., `--search-spec name,path` or
`--search-spec doc`). Add `--search-case-sensitive` for exact case matching, or
`--direct-match-only` to keep container matches collapsed.

Search respects `--private`, feature flags, and syntax highlighting.

## Listing

Use `--list` to print a concise catalog of crate items instead of rendering
Rust code. Each line reports the item kind and its fully qualified path:

```sh
# Survey the high-level structure of tokio without emitting code
ruskel tokio --list

crate      crate
module     crate::sync
struct     crate::sync::Mutex
trait      crate::io::AsyncRead
```

Combine `--list` with `--search` to filter the catalog using the same domain
controls. The listing honours `--private`, feature flags, and paging choices,
but conflicts with `--raw`.

---

## MCP Server

Ruskel can run as a Model Context Protocol (MCP) server for coding agents.

### Running as MCP Server

To start Ruskel in MCP server mode:

```bash
ruskel --mcp
```

This starts the server on stdout, exposing a single `ruskel` tool.

### MCP Configuration

For Codex CLI, Claude Code, or other coding agents:

```json
{
  "mcpServers": {
    "ruskel": {
      "command": "ruskel",
      "args": ["--mcp"]
    }
  }
}
```

### Tool Parameters

The `ruskel` tool accepts the following JSON parameters:

#### Required

- `target` (string): The crate/module to generate a skeleton for.

#### Optional

- `bin` (string | null, default: null): Select a specific binary target when rendering a package.
- `private` (boolean, default: false): Include private items.
- `frontmatter` (boolean, default: true): Include comment frontmatter.
- `search` (string | null, default: null): Restrict output to matches for this query.
- `search_spec` (array of strings | null, default: null): Search domains (name, doc, signature,
  path). Defaults to name, doc, signature.
- `search_case_sensitive` (boolean, default: false): Require exact-case matches when searching.
- `direct_match_only` (boolean, default: false): Only render direct matches, not expanded containers.
- `no_default_features` (boolean, default: false): Disable default features.
- `all_features` (boolean, default: false): Enable all features.
- `features` (array of strings, default: []): Features to enable.


---

## libruskel library

The underlying library can be used directly:

```rust
use libruskel::Ruskel;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rs = Ruskel::new("/path/to/target")?;
    let rendered = rs.render(false, false)?;
    println!("{}", rendered);
    Ok(())
}
```

---

## Community

Want to contribute? Have ideas or feature requests? Come tell us about it on
[Discord](https://discord.gg/fHmRmuBDxF).
