# Development Guide

This guide covers development tasks and maintenance procedures for ruskel.

## Prerequisites

- Rust stable toolchain
- Rust nightly toolchain with rust-docs-json component (for std library mapping generation)

Install the nightly toolchain and required component:
```sh
rustup toolchain install nightly
rustup component add --toolchain nightly rust-docs-json
```

## Development Tasks

Use Ncode for the standard development loop:

```sh
ncode check
ncode test
ncode tidy
ncode tidy --check
```

Use `cargo xtask` for project-specific maintenance tasks, such as standard
library mapping generation. The configured tidy hook runs the mapping check.

### Regenerating Standard Library Module Mappings

The standard library module mapping determines which modules come from `core`,
`alloc`, or `std`. This mapping needs to be regenerated when:

- The Rust standard library structure changes
- New modules are added to std/core/alloc
- Module locations change between crates

To regenerate the mapping:

```sh
# Print the generated mapping to stdout
cargo xtask gen-std-mapping

# Write the generated mapping to the source file
cargo xtask gen-std-mapping --write

# Check that the checked-in mapping matches generated output
cargo xtask gen-std-mapping --check
```

This will:

1. Analyze the installed rust-docs-json to discover module locations
2. Generate the `STD_MODULE_MAPPING` static in `crates/libruskel/src/stdlib_mapping.rs`
3. Update the source file with the new mapping when `--write` is selected

The generated mapping is consumed by `crates/libruskel/src/stdlib.rs`. Run the
standard checks after regenerating:

```sh
ncode check
ncode test
```

## Architecture Notes

### Standard Library Support

Ruskel supports accessing Rust standard library documentation through the
`rust-docs-json` component. The key components are:

- **Module Mapping**: The `STD_MODULE_MAPPING` in `stdlib_mapping.rs` maps module
  names to their actual crate locations (core/alloc/std)
- **Re-export Handling**: When users request `std::vec`, ruskel knows to load
  it from `alloc` while still displaying it as `std::vec`
- **Bare Module Protection**: Common module names like `vec` or `rc` are
  rejected with helpful error messages
