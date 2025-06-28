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

Ruskel uses the `xtask` pattern for development tasks. All tasks are run through `cargo xtask`.

### Regenerating Standard Library Module Mappings

The standard library module mapping determines which modules come from `core`,
`alloc`, or `std`. This mapping needs to be regenerated when:

- The Rust standard library structure changes
- New modules are added to std/core/alloc
- Module locations change between crates

To regenerate the mapping:

```sh
# Preview the changes (outputs to stdout)
cargo xtask gen-std-mapping

# Write the changes to the source file
cargo xtask gen-std-mapping --write
```

This will:
1. Analyze the installed rust-docs-json to discover module locations
2. Generate the `STD_MODULE_MAPPING` static in `crates/libruskel/src/cargoutils.rs`
3. Update the source file with the new mapping

After regenerating, run the tests to ensure everything still works:
```sh
cargo test
```

## Architecture Notes

### Standard Library Support

Ruskel supports accessing Rust standard library documentation through the
`rust-docs-json` component. The key components are:

- **Module Mapping**: The `STD_MODULE_MAPPING` in `cargoutils.rs` maps module
  names to their actual crate locations (core/alloc/std)
- **Re-export Handling**: When users request `std::vec`, ruskel knows to load
  it from `alloc` while still displaying it as `std::vec`
- **Bare Module Protection**: Common module names like `vec` or `rc` are
  rejected with helpful error messages

