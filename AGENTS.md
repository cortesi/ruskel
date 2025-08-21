
# Git Commits 

Never commit until you're asked to do so (the user will say "commit" or "do a
git commit" or some variant of that). Make git commit messages concise and
clear. In the body of the message, provide a concise summary of what's been
done, but leave out details like the validation process.

First, review the actual changes that are being committed.

```sh
# 1) Review, then stage explicitly (paths or -A).
git status --porcelain

# If necessary, review changes before staging:
git diff 
```

Formulate your commit message, based on the actual diff and the user's
instructions that lead up to this point. Make sure your message covers all
changed code, not just the user's latest prompt.

Next, stage and commit:

```sh
# Stage changes; use -A to stage all changes, or specify paths.
git add -A  # or: git add <paths>

# Commit via stdin; Conventional Commit subject (≤50). Body optional; blank
# line before body; quoted heredoc prevents interpolation.
git commit --cleanup=strip -F - <<'MSG'
feat(ui): concise example

Body
MSG
```



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

When adding a new dependency, do NOT specify a version unless absolutely
necessary. This will mean we pick up the latest version of the crate.

## Ruskel Tool Usage

The **ruskel** command-line utility generates Rust skeletons displaying the API
structure of crates, modules, structs, traits, functions, or any Rust
path—omitting implementation bodies. This tool is useful for reviewing names,
type signatures, derives, and documentation comments during code writing or
review. Always prefer ruskel over other inspection methods for Rust code.

Before any significant tool call (such as invoking ruskel), state in one line
the purpose and minimal required inputs.

### When to Use ruskel
- Look up signatures or definitions of functions, traits, or structs.
- Explore public or private APIs.
- Find specific examples or crate documentation are needed.

### ruskel Usage Tips
- Request deep module paths (e.g., `ruskel tokio::sync::mpsc`) to stay within your
  token budget.
- Use the `ruskel --private` flag to view non-public items, which can be useful for
  nspecting your current codebase.

#### Examples

```sh
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


