
# Ruskel

[![Crates.io](https://img.shields.io/crates/v/ruskel.svg)](https://crates.io/crates/ruskel)
[![Documentation](https://docs.rs/ruskel/badge.svg)](https://docs.rs/ruskel)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

Ruskel is a command-line tool for generating skeletonized versions of Rust
crates. It produces a single-page, syntactically valid Rust code representation
of a crate, with all implementations omitted. This provides a quick, clear
overview of the crate's structure and public API.

A secondary goal of Ruskel is to provide complete snapshots of the entire
public surface area of a crate or module, which can be passed to AI tools as a
reference, without including the entire source.


## Features

- Generate a skeletonized view of any Rust crate
- Support for local crates and remote crates from crates.io
- Syntax highlighting for terminal output
- Option to output raw JSON data for further processing
- Configurable to include private items and auto-implemented traits
- Support for custom feature flags


## Installation

Install Ruskel using Cargo:

```sh
cargo install ruskel
```

## Usage

Basic usage:

```sh
ruskel [TARGET]
```

Where `TARGET` can be a directory, file path, or a module name. If omitted, it
defaults to the current directory.

### Options

- `--raw`: Output raw JSON instead of rendered Rust code
- `--auto-impls`: Render auto-implemented traits
- `--private`: Render private items
- `--no-default-features`: Disable default features
- `--all-features`: Enable all features
- `--features <FEATURES>`: Specify features to enable (comma-separated)
- `--highlight`: Force enable syntax highlighting
- `--no-highlight`: Disable syntax highlighting

For more details, run:

```sh
ruskel --help
```

## Examples

Generate a skeleton for the current project:

```sh
ruskel
```

Generate a skeleton for a specific crate from crates.io:

```sh
ruskel serde
```

Output raw JSON data:

```sh
ruskel --raw tokio
```

Include private items and auto-implemented traits:

```sh
ruskel --private --auto-impls
```

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
