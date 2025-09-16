# Frontmatter Feature

A plan to add configurable frontmatter comments to ruskel output, documenting invocation context
while keeping the rendered skeleton valid Rust.

1. Specification

1. [x] Draft the frontmatter format covering target path, visibility mode, search details, and the
       validity sentence.
2. [x] Define how hits are summarized when search mode is active, including domain labels per hit.
3. [x] Decide on comment syntax and placement to guarantee compatibility with existing output.

2. Rendering Implementation

1. [x] Extend the renderer to emit frontmatter comments before any crate content when enabled.
2. [x] Include search hit listings in the frontmatter when results are present, mirroring CLI labels.
3. [x] Ensure private/public flags and target path are correctly populated for both render and search
       flows.
4. [x] Guard frontmatter emission behind a configuration switch that defaults to on.

3. CLI and API Surface

1. [x] Add a CLI flag (and Ruskel API option) to disable frontmatter, wiring through MCP and search.
2. [x] Extend MCP parameters and response summaries to respect the new toggle.
3. [x] Update configuration plumbing/tests for quiet or raw JSON modes to bypass frontmatter when
       required.

4. Documentation

1. [x] Document the frontmatter feature in README, `--help`, and MCP tool descriptions, including the
       disable flag.
2. [x] Add release notes or changelog entry explaining the new default output change.

5. Quality Assurance

1. [x] Add regression tests covering default frontmatter, disabled mode, and search hit listings.
2. [x] Run `cargo +nightly fmt --all`, `cargo clippy --all --all-targets --all-features`, and
       `cargo test --all`.
