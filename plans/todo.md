# Search Feature Enhancements

1. Stage One: Consolidate search domains flag
Clarify the CLI so users know every domain is searched unless they opt out explicitly.

1. [x] Replace the individual `--search_*` flags in `crates/ruskel/src/main.rs` with a single
       `--search-spec` option that accepts comma-separated domains and defaults to all domains.
2. [x] Update search domain parsing to reject unknown tokens and surface clear guidance in the
       CLI help output.
3. [x] Refresh user docs (`README.md`, CLI usage text) to document the default behaviour and the
       new `--search-spec` flag.

2. Stage Two: Expand container matches
Ensure direct container hits display their full contents while leaf hits stay focused, and
add a flag to restrict output to direct matches when desired.

1. [x] Adjust `crates/libruskel/src/search.rs` so container matches add their children to the
       render selection.
2. [x] Refine `crates/libruskel/src/render.rs` to honour the new selection data, expanding
       modules, structs, and traits on direct hits while keeping other members elided.
3. [x] Introduce a `--direct-match-only` flag in `crates/ruskel/src/main.rs` that suppresses
       container expansion when set, and thread the behaviour into the search path.
4. [x] Add tests covering module and struct matches to lock in the new display rules alongside
       the existing method filtering behaviour, including the direct-match-only mode.

3. Stage Three: Implement listing mode
Provide a structured listing of crate items with their types and fully qualified paths.

1. [x] Add a `--list` argument to the CLI and thread it through to the libruskel API alongside
       optional search filters.
2. [x] Implement a listing routine in `crates/libruskel` that emits item type labels and full
       paths, reusing the search index when a query is provided.
3. [x] Document the listing output format and extend tests or fixtures to cover the new mode.

4. Stage Four: Clarify rustdoc build failures
Provide users with actionable error messages when rustdoc JSON generation fails.

1. [x] Investigate rustdoc_json error handling paths to identify parse vs compile failures.
2. [x] Improve error reporting so ruskel surfaces the specific failure cause to the user.
3. [x] Update docs or tests if needed to cover the refined messaging.
