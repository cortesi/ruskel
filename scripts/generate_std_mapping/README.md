# Generate Std Module Mapping

This tool analyzes the Rust standard library structure to generate a mapping of which modules come from which crate (core, alloc, or std).

## Requirements

- Rust nightly toolchain with rust-docs-json component:
  ```sh
  rustup component add --toolchain nightly rust-docs-json
  ```

## Usage

```sh
cd scripts/generate_std_mapping
cargo run
```

The script will:
1. Analyze the std, core, and alloc crates from the installed rust-docs-json
2. Determine which modules are re-exported from where
3. Generate Rust code for the `STD_MODULE_MAPPING` static

Copy the generated code to replace the mapping in `crates/libruskel/src/cargoutils.rs`.

## Why this exists

The Rust standard library has a complex structure where `std` re-exports many modules from `core` and `alloc`. To correctly handle paths like `std::vec` (which actually comes from `alloc`), we need to know these mappings. This tool automatically discovers them from the official documentation.