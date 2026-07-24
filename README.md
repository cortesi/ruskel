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
- Dedicated, bounded rustdoc build cache


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

## Build cache

Ruskel writes non-standard-library rustdoc builds to a dedicated cache. This
keeps generated artifacts out of project target directories and Cargo registry
source directories. Standard-library queries continue to read the prebuilt JSON
from the nightly sysroot.

Ruskel selects the cache root in this order:

1. `--cache-dir PATH` or `Ruskel::with_cache_dir(Some(path))`
2. A nonempty `RUSKEL_CACHE_DIR` value
3. The platform cache directory with a `ruskel` child

Use these commands to inspect or clean the cache. They do not require the
nightly toolchain.

```sh
ruskel --cache-status
ruskel --clean-cache
ruskel --cache-dir /custom/cache --cache-status
```

Ruskel runs cache maintenance after build requests. It removes interrupted
trash first. It removes inactive old-toolchain data after one hour and
workspace data after 14 days. If recognized usage exceeds 20 GB, Ruskel evicts
the oldest safe workspace entries until usage is below 15 GB. Active entries,
the newest valid workspace, and entries with invalid metadata remain in place.
`--cache-status` reports recorded workspace paths, package names and versions,
and entries that maintenance cannot safely remove. Cache entries created by an
older Ruskel version show their identity hash until the next query refreshes
their metadata.

Versions before this cache feature can leave artifacts in a project target
directory or a Cargo registry source directory. Run `cargo clean` in each
affected project. For registry sources, inspect the selected package directory
under the Cargo registry source cache and remove only its generated `target`
directory. Ruskel does not remove these legacy artifacts automatically.

Cargo still coordinates dependency resolution and downloads through its global
package-cache lock. A Ruskel query can wait for another Cargo process that holds
this lock.


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
      "args": ["--mcp", "--cache-dir", "/custom/cache"]
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
- `private` (boolean, default: false): Include private items. **Caution:** Avoid using this on
  entire crates as output can be extremely large. Prefer targeting specific modules or items.
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
use libruskel::{CrateRequest, Ruskel};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ruskel = Ruskel::new().with_cache_dir(None);
    let request = CrateRequest::default();
    let rendered = ruskel.render("/path/to/target", &request)?;
    println!("{rendered}");
    Ok(())
}
```

---

## Community

Want to contribute? Have ideas or feature requests? Come tell us about it on
[Discord](https://discord.gg/fHmRmuBDxF).
