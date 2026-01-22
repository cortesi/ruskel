# v0.0.11

- [feat] Add search support with `--search`, `--search-spec`, and
  `--direct-match-only`.
- [feat] Add `--list` mode to emit an item catalog for navigation.
- [feat] Add configurable frontmatter output, including private API notes for
  bin-only crates.
- [feat] Support rendering bin-only targets via rustdoc JSON with
  private-by-default handling.
- [change] Merge impl blocks across alias/re-export paths to reduce duplicate
  output.
- [fix] Improve rustdoc/cargo diagnostics and target specification validation.
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
