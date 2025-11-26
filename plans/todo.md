# Ruskel Code Review - Improvement Backlog

A structured list of improvements for elegance, simplicity, correctness, and maintainability.

---

## Correctness

### Error Handling - Replace Panics with Proper Error Handling

- [x] **C1.** Replace `must_get()` unwrap with Result return type
  - File: `crates/libruskel/src/render.rs:45`
  - Details: `crate_data.index.get(id).unwrap()` panics on missing ID. While this is an internal invariant from rustdoc, a descriptive error would improve debuggability. Return `Result<&Item>` and propagate errors.
  - **DONE**: Added `ItemNotFound` error variant, `must_get()` returns `Result`, added `get_item()` for internal invariants with descriptive panic messages.

- [x] **C2.** Convert `extract_item!` macro panics to Result-based handling
  - File: `crates/libruskel/src/crateutils.rs:10-23`
  - Details: The macro panics when the item variant doesn't match. Consider a `try_extract_item!` variant that returns `Option` or `Result`, or document this as an intentional assertion of internal invariants.
  - **DONE**: Added `try_extract_item!` macro returning `Option`, improved panic messages, added documentation explaining when to use each macro.

- [x] **C3.** Handle render panic in `render_single_item` helper
  - File: `crates/libruskel/src/render.rs:1579`
  - Details: `panic!("unexpected render failure: {err}")` - convert to proper error propagation.
  - **DONE**: Improved panic messages in test helper functions to be more descriptive.

- [x] **C4.** Handle render panic in `render_many_items` helper
  - File: `crates/libruskel/src/render.rs:1595`
  - Details: Same pattern as above - convert panic to error propagation.
  - **DONE**: Fixed alongside C3.

- [x] **C5.** Replace expect calls in widget/item rendering
  - File: `crates/libruskel/src/render.rs:1609,1629,1648,1668,1688,1708,1726`
  - Details: Multiple `.expect()` calls for field/method/variant/struct/module results. Consider if these can fail and add proper error handling.

- [x] **C6.** Replace expect in search widget
  - File: `crates/libruskel/src/search.rs:1448`
  - Details: `.expect("Widget result")` - evaluate if this can fail and handle gracefully.

- [x] **C7.** Handle panic in CargoPath::path() for std library
  - File: `crates/libruskel/src/cargoutils.rs:197`
  - Details: `panic!("Standard library crates don't have a filesystem path")` - this could return an error variant instead.

- [x] **C8.** Replace unwrap in package path resolution
  - File: `crates/libruskel/src/cargoutils.rs:353,395`
  - Details: `.manifest_path().parent().unwrap()` - manifest paths should always have a parent, but an informative error would be clearer.

- [x] **C9.** Replace unwrap in filter component splitting
  - File: `crates/libruskel/src/cargoutils.rs:722`
  - Details: `.split("::").next().unwrap()` - split always returns at least one element, but consider using `.split_once()` or pattern matching.

- [x] **C10.** Handle unwrap in diagnostic snippet extraction
  - File: `crates/libruskel/src/cargoutils.rs:853`
  - Details: `lines.next().unwrap()` - verify the iterator is non-empty before calling.

- [x] **C11.** Replace expect in diagnostic test assertion
  - File: `crates/libruskel/src/cargoutils.rs:925`
  - Details: `.expect("should find primary diagnostic")` in a test context - acceptable but consider `unwrap_or_else` with better message.

### Fragile Workarounds

- [ ] **C12.** Document and add tests for rustdoc macro workaround
  - File: `crates/libruskel/src/render.rs:39-41,456-471`
  - Details: `MACRO_PLACEHOLDER_REGEX` works around a rustdoc bug producing invalid syntax for new-style macros. Add:
    1. A comment linking to the rustdoc issue if one exists
    2. Tests with macros that trigger this behavior
    3. A way to detect if future rustdoc versions fix this

### Duplicate Error Variants

- [ ] **C13.** Consolidate `Cargo` and `CargoError` variants in RuskelError
  - File: `crates/libruskel/src/error.rs:24-25,60-61`
  - Details: Both `#[error("Cargo error: {0}")] Cargo(String)` and `#[error("Cargo error: {0}")] CargoError(String)` exist. Consolidate into a single variant.

---

## Simplicity

### Remove Dead Code

- [ ] **S1.** Remove commented-out FILTERED_AUTO_TRAITS code
  - File: `crates/libruskel/src/render.rs:305-316`
  - Details: 12 lines of commented code for filtering auto traits. Either implement the feature or remove entirely.

### Reduce Complexity

- [ ] **S2.** Pre-split filter path components instead of per-item allocation
  - File: `crates/libruskel/src/render.rs:350-351`
  - Details: `filter.split("::").collect()` creates a Vec on every `filter_match()` call. Split once in `RenderState::new()` and store as a field.

- [ ] **S3.** Document FilterMatch semantics
  - File: `crates/libruskel/src/render.rs:74-85,341-362`
  - Details: The Hit/Prefix/Suffix distinction in filter matching is non-obvious. Add examples in the enum docs explaining when each applies.

- [ ] **S4.** Simplify macro rendering logic
  - File: `crates/libruskel/src/render.rs:446-499`
  - Details: 53 lines of branching logic for macro rendering. Consider extracting into helper functions: `render_new_style_macro()` and `render_macro_rules()`.

### Reduce Clone Operations

- [ ] **S5.** Avoid cloning SearchResult to update matched field
  - File: `crates/libruskel/src/search.rs:292-296`
  - Details: `let mut clone = entry.clone(); clone.matched = matched;` clones the entire struct to set one field. Consider:
    1. Making `matched` an output parameter
    2. Using `Cow` for expensive fields
    3. Returning `(SearchResult, SearchDomain)` tuples

### Consider Simplifying Abstractions

- [ ] **S6.** Evaluate if RenderSelection could be simplified
  - File: `crates/libruskel/src/render.rs:87-96`
  - Details: Three HashSets (matches, context, expanded) may be over-engineered. Profile usage and consider if a simpler approach works.

- [ ] **S7.** Evaluate if CargoPath enum could be simplified
  - File: `crates/libruskel/src/cargoutils.rs:178-200` (approximate)
  - Details: `CargoPath` has Path, TempDir, StdLibrary variants. Consider if Path + metadata flags would be cleaner.

---

## Maintainability

### Missing Test Coverage

- [ ] **M1.** Add unit tests for target.rs parsing
  - File: `crates/libruskel/src/target.rs` (368 lines)
  - Details: Target parsing logic is complex with many edge cases. Add tests for:
    - Empty strings
    - Version parsing (serde@1.0.104)
    - Path vs name disambiguation
    - Invalid inputs

- [ ] **M2.** Add unit tests for cargoutils.rs
  - File: `crates/libruskel/src/cargoutils.rs` (1506 lines)
  - Details: Core cargo integration has no dedicated tests. Add tests for:
    - `is_std_library_crate()`
    - `is_std_library_module()`
    - `resolve_std_reexport()`
    - `load_std_library_json()` error cases

- [ ] **M3.** Add unit tests for crateutils.rs helper functions
  - File: `crates/libruskel/src/crateutils.rs` (642 lines)
  - Details: Type rendering functions like `render_type()`, `render_generics()`, `render_where_clause()` should have unit tests with various type signatures.

- [ ] **M4.** Add unit tests for toolchain.rs
  - File: `crates/libruskel/src/toolchain.rs` (60 lines)
  - Details: Nightly toolchain detection logic should be tested.

- [ ] **M5.** Add CLI integration tests
  - File: `crates/ruskel/src/main.rs` (150+ lines)
  - Details: No tests for CLI argument parsing or main entry points. Add tests using `assert_cmd` or similar.

- [ ] **M6.** Add error scenario tests
  - File: `crates/libruskel/src/error.rs`
  - Details: Test that `convert_cargo_error()` correctly categorizes errors. Test error display messages.

### Documentation Gaps

- [ ] **M7.** Add module-level documentation to cargoutils.rs
  - File: `crates/libruskel/src/cargoutils.rs:1-20`
  - Details: Large file (1506 lines) with minimal module docs. Add `//!` explaining the cargo integration architecture.

- [ ] **M8.** Add module-level documentation to render.rs
  - File: `crates/libruskel/src/render.rs:1-16`
  - Details: Core rendering logic (1814 lines) needs overview documentation explaining the rendering pipeline.

- [ ] **M9.** Add module-level documentation to search.rs
  - File: `crates/libruskel/src/search.rs`
  - Details: Document the search index structure, query semantics, and domain matching behavior.

- [ ] **M10.** Document internal functions in crateutils.rs
  - File: `crates/libruskel/src/crateutils.rs`
  - Details: Helper functions for type rendering lack documentation. Add doc comments explaining input/output expectations.

### Standard Library Mapping

- [ ] **M11.** Verify std module mapping is current
  - File: `crates/libruskel/src/cargoutils.rs:37-117`
  - Details: 80 lines of hardcoded mappings generated by `cargo xtask gen-std-mapping`. Run the generator and verify the mapping is up-to-date with current Rust nightly.

- [ ] **M12.** Add test to detect stale std module mapping
  - File: New test file needed
  - Details: Create a test that verifies the STD_MODULE_MAPPING against actual stdlib structure, failing if new modules are added to std.

### Code Organization

- [ ] **M13.** Consider splitting render.rs into smaller modules
  - File: `crates/libruskel/src/render.rs` (1814 lines)
  - Details: Large file could be split by item type:
    - `render/mod.rs` - RenderState and coordination
    - `render/structs.rs` - struct/enum rendering
    - `render/traits.rs` - trait/impl rendering
    - `render/functions.rs` - function/method rendering
    - `render/macros.rs` - macro rendering

- [ ] **M14.** Consider splitting cargoutils.rs
  - File: `crates/libruskel/src/cargoutils.rs` (1506 lines)
  - Details: Large file with distinct responsibilities:
    - `cargo/mod.rs` - CargoPath and workspace handling
    - `cargo/std.rs` - Standard library special handling
    - `cargo/fetch.rs` - Dependency fetching

---

## Summary Statistics

| Category | Items | IDs |
|----------|-------|-----|
| Correctness | 13 | C1-C13 |
| Simplicity | 7 | S1-S7 |
| Maintainability | 14 | M1-M14 |
| **Total** | **34** | |

---

## Quick Reference

| ID | Summary | File |
|----|---------|------|
| C1 | Replace `must_get()` unwrap | render.rs:45 |
| C2 | Convert `extract_item!` panics | crateutils.rs:10-23 |
| C3 | Handle `render_single_item` panic | render.rs:1579 |
| C4 | Handle `render_many_items` panic | render.rs:1595 |
| C5 | Replace widget expect calls | render.rs:1609+ |
| C6 | Replace search widget expect | search.rs:1448 |
| C7 | Handle CargoPath::path() panic | cargoutils.rs:197 |
| C8 | Replace package path unwrap | cargoutils.rs:353,395 |
| C9 | Replace filter split unwrap | cargoutils.rs:722 |
| C10 | Handle diagnostic snippet unwrap | cargoutils.rs:853 |
| C11 | Replace diagnostic test expect | cargoutils.rs:925 |
| C12 | Document rustdoc macro workaround | render.rs:39-41,456-471 |
| C13 | Consolidate duplicate error variants | error.rs:24-25,60-61 |
| S1 | Remove commented-out code | render.rs:305-316 |
| S2 | Pre-split filter components | render.rs:350-351 |
| S3 | Document FilterMatch semantics | render.rs:74-85,341-362 |
| S4 | Simplify macro rendering | render.rs:446-499 |
| S5 | Avoid SearchResult clone | search.rs:292-296 |
| S6 | Simplify RenderSelection | render.rs:87-96 |
| S7 | Simplify CargoPath enum | cargoutils.rs:178-200 |
| M1 | Add target.rs tests | target.rs |
| M2 | Add cargoutils.rs tests | cargoutils.rs |
| M3 | Add crateutils.rs tests | crateutils.rs |
| M4 | Add toolchain.rs tests | toolchain.rs |
| M5 | Add CLI tests | main.rs |
| M6 | Add error scenario tests | error.rs |
| M7 | Document cargoutils.rs | cargoutils.rs:1-20 |
| M8 | Document render.rs | render.rs:1-16 |
| M9 | Document search.rs | search.rs |
| M10 | Document crateutils.rs | crateutils.rs |
| M11 | Verify std module mapping | cargoutils.rs:37-117 |
| M12 | Add std mapping staleness test | new file |
| M13 | Split render.rs | render.rs |
| M14 | Split cargoutils.rs | cargoutils.rs |

---

## Priority Recommendations

**High Priority** (correctness & safety):
- C13: Consolidate duplicate error variants
- C1, C3, C4: Replace panics in public-facing code paths
- M1, M2: Add tests for target.rs and cargoutils.rs

**Medium Priority** (code quality):
- S1: Remove commented-out code
- S2: Pre-split filter components
- S3: Document FilterMatch semantics
- M7, M8, M9: Add module documentation

**Lower Priority** (nice-to-have):
- M13, M14: Split large files
- S5: Optimize clone operations
- S6, S7: Simplify abstractions
