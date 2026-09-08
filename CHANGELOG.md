# Unreleased

- [change] Publish `ruskel` and `ruskel-snapshot` as separate packages so each
  `cargo install` command installs only the named tool.
- [fix] Reject symlinked cache directories and cache/snapshot lock files before
  using them for owned storage operations.
- [fix] Preserve impl constants, generic associated types and their where
  clauses, function ABIs, function-pointer qualifiers, variadics, and singleton
  tuples in API output.
- [fix] Retain snapshot attributes whose names share prefixes with omitted
  built-in attributes. Regenerating existing snapshots can expose previously
  omitted API information.
- [fix] Resolve Rust source files that are Cargo library or binary roots,
  including custom paths, and handle nested `mod.rs` module targets.
- [fix] Expand enum and union contents in searches. Treat an empty MCP search
  as an empty query instead of rendering the whole crate.
- [fix] Preserve output bytes when the configured pager is unavailable.

# v0.0.11

- [feat] Isolate rustdoc JSON builds in a dedicated Ruskel cache.
- [feat] Add `--cache-dir`, `--cache-status`, and `--clean-cache` controls.
- [change] Show readable sizes, relative last-use times, workspace paths, and
  package versions in cache reports.
- [change] Add safe retention, soft-budget eviction, and storage recovery for
  cache-owned build data.
- [feat] Add search support with `--search`, `--search-spec`, and
  `--direct-match-only`.
- [feat] Add `--list` mode to emit an item catalog for navigation.
- [feat] Add configurable frontmatter output, including private API notes for
  bin-only crates.
- [feat] Support rendering bin-only targets via rustdoc JSON with
  private-by-default handling.
- [change] Merge impl blocks across alias/re-export paths to reduce duplicate
  output.
- [change] Replace positional `libruskel` feature and privacy arguments with
  the borrowed `CrateRequest` options type.
- [fix] Improve rustdoc/cargo diagnostics and target specification validation.
- [fix] Keep paths after an entrypoint inside the selected crate instead of
  retargeting them to a dependency with the same name.
- [fix] Resolve named dependencies by direct `Cargo.toml` keys instead of
  matching transitive package names.
- [fix] Render bodyless negative impls with their `!` polarity.
- [bug] Include struct field docs in output
- [feat] MCP server
- Many improvements to parsing, language support and output

# v0.0.10

- Update dependencies for Rust 1.85.0

# v0.0.9

- Simplify handling of auto traits - they are now all included or not based on
  the `--auto-impls` flag.
- Render some trait implementations as derives, rather than impl blocks.

# v0.0.8

- Adapt to rustdoc JSON format changes

# v0.0.7

- Add --quiet flag, and corresponding arguments to libruskel
- Adapt to rustdoc JSON format changes

# v0.0.6

- More robust output paging
- Filters now work for trait impl fns
- Silence cargo output during rendering
- Correct error when running ruskel with no argument outside a crate
- Many bugfixes in target specification and filtering
