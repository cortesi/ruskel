



# Rust Development Guidelines

## General Guidelines

- Use Rust edition 2024.
- Avoid including code examples in documentation comments.
- Always introduce items from the standard library with a `use` declaration at
  the top of the file; do not reference `std` paths directly in the code body.
- Functions should not be nested inside other functions except in extremely
  rare cases where necessary.

## Linting

Before committing, ensure all code passes lint checks and all warnings are
addressed. Run:

```bash
cargo clippy -q --fix --all-targets --all-features --allow-dirty --tests --examples 2>&1
```

Clippy outputs warnings to stderr, which we merge into stdout so all messages
appear together. Resolve any lingering warnings manually. After running lint
checks or making code edits, validate the result in 1-2 lines and proceed or
self-correct if validation fails.

## Formatting

Format all code according to Rust conventions with:

```bash
cargo fmt --all
```

## Testing
After completing changes, execute all tests using:

```bash
cargo test --all
```

## Dependencies

Add dependencies by using the following command rather than editing
`Cargo.toml` directly:

```bash
cargo add <crate_name>
```

## Ruskel Tool Usage

**ruskel** generates Rust skeletons displaying the API structure of crates,
modules, structs, traits, functions, or any Rust path—omitting implementation
bodies. This tool is useful for reviewing names, type signatures, derives, and
documentation comments during code writing or review.

Before any significant tool call (such as invoking ruskel), state in one line the purpose and minimal required inputs.

### When to Use ruskel
- To look up signatures or definitions of functions, traits, or structs.
- To obtain overviews of public or private APIs.
- When specific examples or crate documentation are needed.

### ruskel Usage Tips
- Request deep module paths (e.g., `tokio::sync::mpsc`) to stay within your token budget.
- Use the `--private` flag to view non-public items, which can be useful for inspecting your current codebase.

#### Examples
```bash
# Inspect the current project
ruskel

# Query a standard library trait
ruskel std::io::Read

# In a workspace with a crate 'mycrate'
ruskel mycrate

# View a method on a struct in the current crate
ruskel mycrate::Struct::method

# Explore a dependency or fetch from crates.io if not present
ruskel serde

# Look within a crate's module
ruskel serde::de::Deserialize

# Via filesystem path
ruskel /my/path

# Sub-module within a path
ruskel /my/path::foo

# Specific dependency version from crates.io
ruskel serde@1.0.0
```

</rust>

## Reliable Git Commits

- Prepare: run `cargo fmt --all`, `cargo clippy -q --fix --all-targets --all-features --allow-dirty --tests --examples 2>&1`, and `cargo test --all` to ensure clean state.
- Stage: prefer explicit staging. Use `git add -A` (or specific paths) and verify with `git status --porcelain`.
- Message: write the commit message to a file to avoid shell interpolation issues with symbols like `<`, `>`, `$`, and backticks. Example: write to `/tmp/commit_msg.txt` and run `git commit -F /tmp/commit_msg.txt`.
- Style: use concise Conventional Commits subjects (e.g., `fix(render): ...`) and a brief body listing changes and validation.
- Validate: after committing, run `git show --stat -1` to verify contents and `git log -1 --pretty=format:%s` to confirm the subject.

