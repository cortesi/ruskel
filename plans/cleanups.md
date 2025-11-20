# Cleanups TODO

1. High-Priority Bugs

1. [x] Respect the `--private` flag when emitting JSON: plumb the `private_items` toggle through `Ruskel::inspect` so `rs.raw_json` can strip private entries instead of always returning the full graph (crates/libruskel/src/ruskel.rs:95).
2. [x] Harden pager execution by parsing the `PAGER` value into a command + args before the availability check/spawn, otherwise settings like `less -R` crash due to spawning a non-existent binary name (crates/ruskel/src/main.rs:248).
3. [x] Prefer workspace crates before crates.io: when resolving `Entrypoint::Name`, inspect the current workspace members instead of immediately falling back to a dummy fetch so local packages are used offline and stay in sync (crates/libruskel/src/cargoutils.rs:574).

2. Reliability & Robustness

1. [x] Stop panicking when deriving manifest paths: replace the unconditional `absolute(...).unwrap()` with fallible handling so odd filesystems or permission issues surface as `RuskelError` instead of aborting (crates/libruskel/src/cargoutils.rs:294).
2. [x] Improve offline error reporting for dummy crates by detecting `--offline` with uncached dependencies and returning a tailored message instead of a generic Cargo fetch failure (crates/libruskel/src/cargoutils.rs:644).

3. Code Quality Polish

1. [x] Centralize the reserved-word list so `render.rs` and `crateutils.rs` share a single source and stay consistent when Rust adds keywords (crates/libruskel/src/render.rs:30, crates/libruskel/src/crateutils.rs:23).
2. [x] Cache syntect assets (syntax + theme + macro regex) behind a `Lazy` so repeated highlights avoid reloading large tables and recompiling regexes (crates/libruskel/src/highlight.rs:15, crates/libruskel/src/render.rs:259).
3. [x] Fix typos in the public docs (`skeltonized`, `Ruskell`) to keep generated help text professional (crates/libruskel/src/lib.rs:8, crates/libruskel/src/ruskel.rs:21).

4. Structural Simplification

1. [x] Replace the bespoke `ColorMode` enum with clap's built-in `ColorChoice` so the CLI stops carrying its own Display impl and matching logic (crates/ruskel/src/main.rs), trimming code and letting clap handle color parsing.
2. [x] Drop the custom `LogLevel` wrapper in favor of `tracing_subscriber::filter::LevelFilter` parsed directly by clap, removing the enum plus `as_filter` helper and their match arms (crates/ruskel/src/main.rs).
3. [x] Eliminate the duplicate search-spec enums in the CLI and MCP crates by making `SearchDomain` parse/serialize string tokens in `crates/libruskel/src/search.rs`, then reusing it for `--search-spec` and `search_spec` so the extra enums and conversion loops can be deleted (crates/ruskel/src/main.rs; crates/mcp/src/server.rs).
4. [x] Consolidate nightly/sysroot detection into a single helper reused by the CLI, libruskel, and xtask (current copies in crates/ruskel/src/main.rs, crates/libruskel/src/cargoutils.rs, xtask/src/main.rs) to avoid three nearly identical rustup probes and keep messaging consistent.
5. [x] Encode the `--addr`/`--log` dependency on `--mcp` with clap `requires/conflicts_with` attributes instead of post-parse `if` checks, removing the manual validation branches and letting clap format the errors (crates/ruskel/src/main.rs).
