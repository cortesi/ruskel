# ruskel

![Discord](https://img.shields.io/discord/1381424110831145070?style=flat-square&logo=rust&link=https%3A%2F%2Fdiscord.gg%2FfHmRmuBDxF)
[![Crates.io](https://img.shields.io/crates/v/libruskel.svg)](https://crates.io/crates/libruskel)
[![Documentation](https://docs.rs/libruskel/badge.svg)](https://docs.rs/libruskel)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

Ruskel produces a syntactically correct, single-page skeleton of a crate's
public API. If the crate is not found in the local workspace, it is fetched
from [crates.io](https://crates.io).

Ruskel is great for:

- Quick access to Rust documentation from the command line.
- Exporting the full public API of a crate as a single file to pass to LLMs and
  other tools.
- Quick access to std library documentation, including `std`, `core`, and
  `alloc` prefixes - e.g. `ruskel std::vec::Vec`.

For example, here is the skeleton of the very tiny `termsize` crate. Note that
the entire public API is included, but all implementation is omitted.

---

## Installation

To install Ruskel, run:

```sh
cargo install ruskel
```

Note: While ruskel requires the nightly toolchain to run, you can install it using any toolchain.


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

Ruskel has a flexible target specification that tries to do the right thing in
a wide set of circumstances.

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

# Search for "status" across names, signatures and doc comments
ruskel reqwest --search status

# Search for "status" in only names and signatures
ruskel reqwest --search status --search-spec name,signature

# Search for "status" in docs only
ruskel reqwest --search status --search-spec doc

# Access via std re-exports (recommended - matches your import statements)
ruskel std::vec::Vec        # Vec type from std
ruskel std::rc::Rc          # Rc type from std
ruskel std::mem::size_of    # size_of function from std

# Direct access to core and alloc
ruskel core::mem            # Memory utilities from core
ruskel alloc::vec           # Vec module from alloc

# Get entire crate documentation
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

By default the query matches the name, doc, and signature domains with case-insensitive
comparisons. Include the optional `path` domain when you need canonical path
matches by passing `--search-spec name,path`, or use `--search-spec doc` to
inspect documentation only. Combine with `--search-case-sensitive` to require
exact letter case.
Add `--direct-match-only` when you want container matches (modules, structs, traits) to stay
collapsed and show only the exact hits.

The search output respects existing flags like `--private`, feature controls, and
syntax highlighting options.

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
controls as skeleton search. The listing honours `--private`, feature flags, and
paging choices, and it conflicts with `--raw` because the output is tabular
text rather than Rust code.

---

## MCP Server

Ruskel can run as a Model Context Protocol (MCP) server, allowing it to be used
as a tool by AI assistants and other MCP clients.

### Running as MCP Server

To start Ruskel in MCP server mode:

```bash
ruskel --mcp
```

This starts the MCP server on stdout, ready to accept requests. The server exposes a single tool
called `ruskel` that generates skeletonized outlines of Rust crates.

### MCP Configuration

To use Ruskel with Claude Desktop or other MCP clients, add this configuration:

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
- `frontmatter` (boolean, default: true): Include comment frontmatter describing the invocation.
- `search` (string | null, default: null): Restrict the response to matches for this query instead
  of rendering the entire target.
- `search_spec` (array of strings | null, default: null): Search domains (name, doc, signature,
  path). When omitted or empty, name, doc, and signature are searched.
- `search_case_sensitive` (boolean, default: false): Require exact-case matches when searching.
- `direct_match_only` (boolean, default: false): Suppress container expansion so only direct hits
  are rendered.
- `no_default_features` (boolean, default: false): Disable default features.
- `all_features` (boolean, default: false): Enable all features.
- `features` (array of strings, default: []): Features to enable.

---

## Community

Want to contribute? Have ideas or feature requests? Come tell us about it on
[Discord](https://discord.gg/fHmRmuBDxF).

---

## Features

- Generate a skeletonized view of any Rust crate
- Support for both local crates and remote crates from crates.io
- Filter output to matched items using `--search` with the `--search-spec` domain selector and
  `--direct-match-only` when you want to avoid container expansion
- Generate tabular item listings with `--list`, optionally filtered by `--search`
- Syntax highlighting for terminal output
- Optionally include private items and auto-implemented traits
- Support for custom feature flags and version specification
- Full support for Rust standard library documentation (std, core, alloc)

---

## Requirements

Ruskel requires the Rust nightly toolchain for its operation:

- **Nightly toolchain**: Required for unstable rustdoc features used to generate JSON documentation
- **rust-docs-json component** (optional): Required only for standard library documentation access

Install the nightly toolchain:
```sh
rustup toolchain install nightly
```

For standard library support, also install:
```sh
rustup component add --toolchain nightly rust-docs-json
```


---

## libruskel library

`libruskel` is a library that can be integrated into other Rust projects to
provide Ruskel functionality.

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
