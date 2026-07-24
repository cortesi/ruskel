# DESLOP SPEC: One Inspection and Rendering Data Flow

## Description

Ruskel has good behavior coverage, but several paths express the same policy more than once. This specification reduces build-heavy tests, removes a production test bypass, and consolidates selection, cache, request, standard-library, and build policy. The result keeps the current output and cache safety rules while making each operation easier to test and change.

## Findings

### D1: Reuse Isolated Rustdoc Fixtures

- Outcome: Renderer integration targets share a small set of real rustdoc documents within each default test-harness process, and all test harnesses avoid the user's default Ruskel cache.
- Evidence: `crates/libruskel/tests/utils.rs::create_test_crate` creates a new random workspace for each generated test. `inspect_crate` then uses `Ruskel::new()` without `with_cache_dir`, so each case creates a persistent default-cache workspace. The `gen_tests!` suites define 127 source cases. A live `ruskel --cache-status` inspection found 456 retained `dummy_crate 0.1.0` workspaces with random temporary roots.
- Change: Replace the per-case crate builder with checked-in fixture definitions grouped by integration-test concern. Compose each group into one temporary fixture crate. Under the default test harness, compile each fixture once per integration-test process and retain its `Crate` in a test fixture guarded by `OnceLock`. Give every build-heavy process an explicit temporary cache whose guard lives as long as the fixture. Use the same fixture definitions under `nextest`, but do not claim compiled-fixture reuse across its isolated test processes. Keep a small, separate end-to-end set for target resolution, proc macros, crate-absolute paths, and cache integration. Do not commit generated rustdoc JSON.
- Constraints: Preserve the current syntax, privacy, procedural-macro, filtering, and formatting cases. Keep rustdoc schema changes visible by generating JSON with the selected nightly toolchain. Support parallel `nextest` processes without global environment mutation or shared temporary paths. Treat default-harness reuse and cache isolation under all harnesses as separate guarantees. Do not add doctests.
- Proof: Add a fixture-construction counter that proves each fixture builds once in a default-harness integration-test process. Run each renderer integration target with the default Cargo test harness twice. Run `cargo nextest run -p libruskel` for behavior and isolation, not as proof of cross-process reuse. Compare default cache status before and after both harnesses and prove that no new test workspace appears. Run Clippy for all `libruskel` targets.

### D2: Remove the MCP Test-Mode Backdoor

- Outcome: Every MCP tool request runs the same handler in tests and production.
- Evidence: `crates/mcp/src/server.rs::ruskel` returns `run_test_mode` whenever `RUSKEL_MCP_TEST_MODE` exists. `crates/mcp/tests/integration_test.rs::create_test_client_with_defaults` uses unsafe global environment mutation to enable this branch. Most tool-call tests then prove only that stub text is nonempty or contains copied parameters.
- Change: Delete `RUSKEL_MCP_TEST_MODE` handling and `run_test_mode`. Make protocol tests call the real handler with the narrowest standard-library targets that use prebuilt rustdoc JSON. Preflight the nightly `rust-docs-json` component and fail with the repository's installation guidance when it is unavailable. Assert stable response structure, target names, and semantic anchors for skeleton, search, frontmatter, default, error, and multi-request behavior. Do not assert complete rendered text, item counts, or nightly-specific prose. Keep protocol-only initialization and invalid-tool tests independent of rustdoc loading.
- Constraints: Keep the published MCP schema and server defaults unchanged. Keep tests offline and deterministic for any supported installed nightly. Treat nightly plus `rust-docs-json` as an explicit prerequisite for real-handler integration tests, and do not silently skip them. Do not add a public test interface, a test-only production feature, or a one-implementation service trait.
- Proof: Add prerequisite failure coverage for a missing `rust-docs-json` component and structural assertions for the narrow standard-library targets. Run `cargo nextest run -p ruskel-mcp`. Run the MCP integration target with the test-mode variable unset and set, and prove that both runs use identical production behavior. Run Clippy for all `ruskel-mcp` targets.

### D3: Use One Item-Selection Engine

- Outcome: Every render-visible target path and search match resolves to one item-ID selection before rendering starts.
- Evidence: `crates/libruskel/src/render.rs` contains the string-based `FilterMatch` state machine, `Renderer::filter`, `RenderState::filter_components`, and `should_filter`. The same file also contains the item-ID-based `RenderSelection` state machine. `crates/libruskel/src/search.rs::build_render_selection` independently computes matches, ancestors, descendants, and expansion. `RenderState::render_item` must apply both systems on each item. The string filter can also reach inlined targets through named and glob re-exports, descend through nameless containers and private modules, and control module documentation with path forms that the current canonical index does not represent.
- Change: Move render-visible path records and selection flags to one internal selection module. Record direct definition paths and use-site paths for named and glob-inlined re-exports, and resolve each alias to the item ID that the renderer emits. Represent nameless-container traversal, private-module descent, and module-document decisions in the same selection result. Resolve `Renderer::with_filter` through this model before rendering, and produce `FilterNotMatched` when no compatible path exists. Make the renderer consume only item-ID context, match, expansion, visibility, and module-document decisions. Remove `FilterMatch`, filter component tracking, and path-string checks from `RenderState` only after the compatibility cases pass.
- Constraints: Preserve `Renderer::with_filter` as a public compatibility entry point. Preserve direct paths, named re-export aliases, glob-inlined paths, nameless-container traversal, private-module descent, exact-path behavior, module documentation rules, private-item behavior, container expansion, `--direct-match-only`, fields, variants, trait items, and impl members. Do not drop a render-visible filter path to obtain one canonical definition path. Do not make the renderer depend on transport-specific search options.
- Proof: Add characterization tables for direct definitions, named re-export aliases, glob-inlined items, nameless containers, and private-module descent. For each case, preserve rendered output and module-document behavior, then compare exact filtering with the equivalent path-domain selection by item ID. Run all filter, module, listing, and search tests. Run CLI tests for nested, aliased, private, and unmatched paths. Run `cargo nextest run -p libruskel` and Clippy for all `libruskel` targets.

### D4: Build One Typed Cache Inventory

- Outcome: Status, clean, retention, and budget enforcement use one recognized cache-tree model.
- Evidence: `crates/libruskel/src/cache/owner.rs::{status_unlocked,toolchain_status,trash_status,clean_build_entries,clean_trash_entries}` and `cache/maintenance.rs::{cleanup_trash,collect_old_toolchains,collect_old_workspaces,workspace_candidates,recognized_usage}` repeat directory recognition, size, timestamp, and issue policy. `maintenance.rs` imports `read_dir_sorted` and `is_owned_directory` from `owner.rs`, which reverses the ownership boundary. `recognized_usage` measures whole build and trash directories, while cache status excludes unrecognized entries.
- Change: Add an internal cache inventory module that performs sorted, no-follow traversal and returns typed toolchain, workspace, trash, metadata, size, and issue records. Make status format this inventory. Make clean, retention, and budget code select candidates from it before they acquire the required leases and revalidate destructive targets. Put the canonical recognized-usage calculation in the inventory. Include the cache root in `CleanReport` so the CLI can remove its pre-clean status scan.
- Constraints: Preserve stable marker and lock inodes, lease order, busy-entry skipping, symlink refusal, trash-before-delete behavior, metadata safety, and best-effort issue reporting. Revalidate a candidate after lock acquisition because the inventory is a snapshot. Never count or delete an unrecognized entry as owned data.
- Proof: Run all cache unit, integration, subprocess-lock, and CLI tests. Add fixtures with foreign files, symlinked entries, invalid timestamps, disappearing entries, and locked workspaces. Prove that status, clean, retention, and budget classify each fixture consistently. Run `cargo nextest run -p libruskel --test cache`, the focused cache unit tests, and Clippy for all workspace targets.

### D5: Replace Positional Query Arguments with Request Options

- Outcome: Each inspection call carries one borrowed crate request and one optional search request.
- Evidence: `crates/libruskel/src/ruskel.rs::{inspect,search,list,render,raw_json,load_target}` repeat `no_default_features`, `all_features`, `features`, and privacy arguments. CLI and MCP callers clone the feature vector into these positional calls. Privacy exists in `SearchOptions`, a separate `list` parameter, and render or inspect parameters. CLI and MCP also fold and validate `SearchDomain` values through different helper paths, while the exported `parse_domain_tokens` silently ignores invalid values.
- Change: Add one public crate-request options type for default features, all features, explicit features, and private items. Borrow it in `inspect`, `search`, `list`, `render`, and `raw_json`. Remove privacy from `SearchOptions` and from the separate `list` argument. Keep operation-specific visibility policy inside `Ruskel`. Give `SearchDomain` one strict parsing method and one label method, then remove the redundant exported parsing and description helpers. Update the CLI and MCP adapters to construct one crate request per invocation.
- Constraints: Keep process-level settings on `Ruskel`: offline mode, diagnostics, cache, frontmatter, auto impls, and binary selection. Preserve Cargo's current handling of default, all, and explicit feature combinations. Treat the public signature change as an intentional pre-1.0 API change, document it in the changelog, and do not add deprecated forwarding overloads. Do not edit archived consumers that remain pinned to `libruskel 0.0.10`.
- Proof: Add request-construction and feature-matrix tests. Update all workspace consumers and compile examples or documentation that show library use. Run the CLI and MCP search matrices. Inspect the settled `libruskel` public surface with Ruskel, then run workspace Clippy and `cargo nextest run --workspace`.

### D6: Make Standard-Library Mapping a Deterministic Artifact

- Outcome: Standard-library resolution reads one dedicated, reproducible mapping artifact.
- Evidence: `xtask/src/main.rs::find_std_reexports` mixes rustdoc discovery with manual core, alloc, and std override lists. `generate_rust_code` groups output in a `HashMap`, so group order is not stable. `generate_std_mapping` rewrites a substring inside the 2,022-line `cargoutils.rs` file by searching for textual markers. Runtime loading and mapping code occupy the start of that unrelated file.
- Change: Move standard-library loading and re-export resolution to a dedicated module. Generate a sorted static mapping file from discovered modules plus one explicit override table in `xtask`. Write the artifact atomically. Add `gen-std-mapping --check` to compare generated bytes with the checked-in file. Make ordinary generation and check mode use the same function.
- Constraints: Preserve `std`, `core`, `alloc`, `proc_macro`, and `test` behavior. Preserve display-name rewriting for `std` re-exports and rejection of bare standard-module names. Require the selected nightly `rust-docs-json` component for generation and check mode. Make ordinary runtime compilation read only the checked-in artifact. Do not make runtime behavior depend on hash iteration order.
- Proof: Run generation twice and prove byte-identical output. Run `cargo xtask gen-std-mapping --check`. Run target-resolution tests for every mapped top-level module and focused CLI checks for direct and re-exported standard-library paths. Run `git diff --check` after regeneration.

### D7: Separate Target Resolution from Rustdoc Execution

- Outcome: Target resolution returns passive build input, and one build coordinator owns cache, retry, toolchain, diagnostics, and JSON loading.
- Evidence: `crates/libruskel/src/cargoutils.rs` mixes Cargo source discovery, dependency fetching, dummy manifest creation, package-target selection, standard-library handling, rustdoc execution, cache recovery, retry budgets, and diagnostic parsing. `CargoPath::read_crate` alone selects packages, acquires cache owners and leases, checks toolchain identity, classifies failures, runs recovery, and controls retries. `ResolvedTarget::read_crate` then adds a forwarding layer over that method.
- Change: Delete `cargoutils.rs` after D6 moves standard-library policy. Put filesystem and registry resolution, dummy crate creation, and `ResolvedTarget` in a target-resolution module. Put package-target selection, build attempts, retry categories, diagnostics, and JSON loading in a rustdoc-build module. Make `ResolvedTarget` a passive input with source, filter, and optional temporary-source ownership. Call one build coordinator directly from `Ruskel::load_target`. Keep cache types inside the build path and remove the forwarding `read_crate` methods.
- Constraints: Preserve workspace-member preference, offline errors, version selection, file-path conversion, binary selection, Cargo diagnostics, cache recovery, toolchain-change retry, and the three-attempt limit. Keep the temporary registry source alive through the build. Use concrete internal types and functions, not new traits or generic source abstractions.
- Proof: Move existing tests beside their owning modules. Run target, offline, binary, diagnostic, retry-budget, and cache recovery tests. Run `cargo nextest run -p libruskel`, `cargo check --workspace --all-targets`, workspace Clippy with warnings denied, and `cargo nextest run --workspace`.

## Execution Plan

### Stage 1: D1 — Reuse Isolated Rustdoc Fixtures

Renderer integration targets reuse isolated rustdoc fixtures under the default harness, and all harnesses avoid the default cache.

- [x] Implement D1 as specified.
- [x] Complete D1's required supporting and downstream work.
- [x] Run D1's proof.
- [x] Review the settled stage.

### Stage 2: D2 — Remove the MCP Test-Mode Backdoor

MCP tests and production use the same handler with explicit nightly prerequisites and stable assertions.

- [x] Implement D2 as specified.
- [x] Complete D2's required supporting and downstream work.
- [x] Run D2's proof.
- [x] Review the settled stage.

### Stage 3: D3 — Use One Item-Selection Engine

All current filter path forms and search results use one item-ID selection model.

- [x] Implement D3 as specified.
- [x] Complete D3's required supporting and downstream work.
- [x] Run D3's proof.
- [x] Review the settled stage.

### Stage 4: D4 — Build One Typed Cache Inventory

All cache readers and collectors use one recognized-tree policy.

- [ ] Implement D4 as specified.
- [ ] Complete D4's required supporting and downstream work.
- [ ] Run D4's proof.
- [ ] Review the settled stage.

### Stage 5: D5 — Replace Positional Query Arguments with Request Options

Each public operation receives one coherent crate request.

- [ ] Implement D5 as specified.
- [ ] Complete D5's required supporting and downstream work.
- [ ] Run D5's proof.
- [ ] Review the settled stage.

### Stage 6: D6 — Make Standard-Library Mapping a Deterministic Artifact

Standard-library mapping generation is isolated, sorted, and checkable.

- [ ] Implement D6 as specified.
- [ ] Complete D6's required supporting and downstream work.
- [ ] Run D6's proof.
- [ ] Review the settled stage.

### Stage 7: D7 — Separate Target Resolution from Rustdoc Execution

Resolution produces passive input for one explicit rustdoc build coordinator.

- [ ] Implement D7 as specified.
- [ ] Complete D7's required supporting and downstream work.
- [ ] Run D7's proof.
- [ ] Review the settled stage.
