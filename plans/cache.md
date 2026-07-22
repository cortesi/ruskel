# SPEC: Dedicated build cache for rustdoc JSON generation

## Description

Ruskel will generate rustdoc JSON in a dedicated cache that it owns and
maintains. The cache will isolate Ruskel from project builds and Cargo registry
sources. It will also give generated artifacts a bounded lifecycle.

## What Changes

`CargoPath::read_crate` in `crates/libruskel/src/cargoutils.rs` currently lets
`rustdoc_json::Builder` select Cargo's target directory. Local queries can then
block developer builds. Registry queries can write build artifacts into the
registry source cache.

For each non-standard-library query, Ruskel will pass two absolute paths to
`rustdoc_json::Builder`:

- `Builder::target_dir` will select the workspace entry in the Ruskel cache.
- `Builder::env("CARGO_BUILD_BUILD_DIR", ...)` will select the same workspace
  entry for intermediate artifacts.

`rustdoc-json` 0.9.10 passes the first path through Cargo's `--target-dir`
option. It passes the second path to the child process environment. These
settings override `CARGO_TARGET_DIR`, `build.target-dir`, and
`build.build-dir`. Ruskel will not use a project-controlled output directory if
the cache is unavailable.

`CargoPath::read_crate` already handles standard-library targets before it
constructs a builder. This path will continue to read prebuilt JSON from the
nightly sysroot. It will not resolve, initialize, or maintain the cache.

The cache root resolves in this order:

1. `--cache-dir <PATH>` or `Ruskel::with_cache_dir(Some(path))`.
2. A nonempty `RUSKEL_CACHE_DIR` value.
3. `dirs::cache_dir()` with a `ruskel` child.

Ruskel will convert an explicit or environment path to an absolute path before
use. `Ruskel::with_cache_dir(None)` will restore environment and platform
resolution. A missing platform cache directory is a cache error. It will not
cause a fallback to a project directory.

## Ownership And Identity

The cache has this version-1 layout:

```text
<cache-root>/
  ruskel.cache
  CACHEDIR.TAG
  maintenance.stamp
  locks/
    gc.lock
    toolchain/<toolchain-id>.lock
    workspace/<workspace-id>.lock
  trash/<recognized-trash-name>/
  build/<toolchain-id>/
    ruskel.last-use
    <workspace-id>/
      ruskel.last-use
      ... Cargo target and build artifacts ...
```

`ruskel.cache` is the immutable ownership marker and layout-version record. An
advisory lock on this file provides the root lease. The files below `locks/`
provide the garbage-collection, toolchain, and workspace leases. The
`maintenance.stamp` file records the last completed maintenance pass.

Ruskel will accept an empty root or a root with recognized incomplete bootstrap
state. It will refuse a nonempty unmarked root and an unsupported layout
version. Initialization will use `create_new` for the marker. The process that
first acquires its exclusive lock will complete the marker and layout. A later
process can complete recognized state after an interrupted initialization.

`ruskel.cache` and the files below `locks/` will keep stable inodes. Ruskel will
never replace them with a rename. An initializer will write and flush the marker
in place while it holds the exclusive root lock. It will not change a valid
marker.

Stamp and last-use updates will use a same-directory temporary file and an
atomic rename. Temporary names will include the process ID and a process-local
atomic counter. Root validation will use `symlink_metadata`. It will reject a
marker symlink and will not treat symlinked build entries as owned directories.

Destructive operations will accept only fixed top-level names, 64-character
lowercase hexadecimal identity names, known metadata names, and generated
trash names. They will skip other entries. They will rename an accepted entry
into `trash/` before removal. Recursive removal will remove symlinks as links
and will not follow them.

The toolchain identity is the full SHA-256 digest of the stdout bytes from
`rustup run nightly rustc -vV`. `toolchain.rs` will compute it for each
non-standard-library request. A persistent MCP server will therefore observe a
nightly update. Ruskel will recompute the identity after it reads rustdoc JSON.
If the identity changed, Ruskel will reject that result and retry once.

`select_package_target` already creates `cargo::core::Workspace` from the
selected package manifest. It will also return the canonical
`Workspace::root()`. The workspace identity is the full SHA-256 digest of that
path's platform-native bytes. Workspace members will share one identity.
Distinct canonical roots will have distinct identities. The full digest will
name both the workspace entry and its lock.

## Concurrency And Lifecycle

Every non-standard-library build attempt will acquire leases in this order:

1. A shared root lease.
2. A shared toolchain lease.
3. An exclusive workspace lease.

The attempt will hold these leases from its first last-use update through the
rustdoc JSON read and toolchain recheck. Requests for different workspaces can
run concurrently. Requests for one workspace will serialize because feature,
privacy, and binary-target variants can replace the same JSON output.

Old-toolchain collection will try an exclusive toolchain lease without waiting.
Workspace collection will try the workspace lease without waiting. It will skip
a busy entry. Garbage collection will never wait while it holds an entry lease.
These rules prevent a collector and builder from waiting on each other.

`Ruskel` will contain a cloneable cache handle. The handle will resolve its
`CacheOwner` only when a cache operation first needs it. A process-local weak
registry, keyed by the canonical cache root, will make separate `Ruskel`
instances and their clones share one owner and one maintenance worker.

The worker will use one bounded wake channel and an atomic pending-reason flag.
The flag will preserve an emergency reason when signals coalesce. Each build
attempt will submit one signal. A low-space signal can request an urgent pass.
The final owner will close the channel, drain pending work, and join the worker.
One-shot CLI use will therefore wait for submitted maintenance before exit.

Cross-process maintenance will use `locks/gc.lock`. A routine worker will try
this lock without waiting. Synchronous recovery will wait for an active pass,
then use that completed pass instead of queueing an equivalent pass.

## Retention And Recovery

Each build attempt will update its workspace and toolchain `ruskel.last-use`
files before the build. It will update them again after a successful JSON read.
Missing, invalid, or future metadata will make an entry ineligible for deletion
and visible as a skipped status entry.

Routine maintenance becomes due 24 hours after `maintenance.stamp`. A completed
pass performs these operations in order:

1. Remove recognized trash from an interrupted deletion.
2. Remove an inactive noncurrent toolchain tree one hour after its last use.
3. Remove a workspace entry 14 days after its last use.
4. Measure recognized cache data and apply the soft budget.

The soft high-water mark is 20,000,000,000 bytes. The collector will evict
workspace entries in last-use order until usage is below 15,000,000,000 bytes.
It will retain the newest valid workspace entry and skip locked entries. The
cache can remain above its soft budget when no safe candidate exists.
`CacheStatus` will report the remaining excess.

`fs4::available_space` will classify less than 1,000,000,000 available bytes as
low space. A build attempt will submit an urgent maintenance signal when this
condition exists. Background work remains coalesced. A retry that depends on
freed space can run one synchronous pass without the 24-hour limit.

The build path will retain typed internal failure categories until it decides
whether to retry:

- Compiler and rustdoc diagnostics are user errors. Ruskel will keep the entry
  and return the current diagnostic mapping.
- Cache-entry I/O errors, `rustdoc_json::BuildError::IoError`, missing JSON, and
  `serde_json` syntax or end-of-file errors indicate probable storage damage.
- `serde_json` data errors indicate toolchain compatibility. Ruskel will return
  the nightly update guidance without a cold retry.
- Manifest, metadata, toolchain invocation, and invalid-layout errors are not
  cache damage. Ruskel will return them without a cold retry.

Low space can reclassify a failed build as storage-related even when Cargo
reported the failure through diagnostics. For probable storage damage, Ruskel
will move the affected workspace entry to `trash/`, release its build leases,
run one synchronous maintenance pass, and retry once.

The storage-recovery and toolchain-change retry budgets are independent. A
request can perform at most three build attempts. If a retry fails, the final
error will include the original failure, the recovery action, and the retry
failure. Recovery remains best effort when disk pressure exists outside the
cache.

## User And Library Interfaces

The CLI adds these forms:

```text
--cache-dir <PATH>  Override the cache root. Also settable with RUSKEL_CACHE_DIR.
--clean-cache       Clean cache-owned build data and report the result.
--cache-status      Report cache usage, entries, and last-use times.
```

`--cache-dir` is valid with ordinary commands, cache commands, and `--mcp`.
`--clean-cache` and `--cache-status` conflict with each other, `--mcp`, and an
explicit target. The positional target in `crates/ruskel/src/main.rs` will
become `Option<String>`. Ordinary commands will continue to use `./` when the
option is absent.

Cache commands will run before the nightly prerequisite check. Status does not
need a nightly toolchain. A clean will try an exclusive root lease without
waiting. It will report a busy cache when a request or maintenance pass holds a
shared root lease. It will preserve the marker, lock namespace, and empty build
and trash directories. The CLI will return an unsuccessful status for a busy or
partially failed clean.

The public library surface adds these high-level controls:

```rust
impl Ruskel {
    pub fn with_cache_dir(self, dir: Option<PathBuf>) -> Self;
    pub fn cache_status(&self) -> Result<CacheStatus>;
    pub fn clean_cache(&self) -> Result<CleanReport>;
}
```

`CacheStatus` will expose the root, total recognized usage, toolchain and
workspace entries, trash usage, skipped entries, and soft-budget excess.
`ToolchainCacheStatus` and `WorkspaceCacheStatus` will expose identity, size,
last-use time, and observed lock state. `CleanReport` will expose removed entry
and byte counts, root-busy state, skipped entries, failures, and usage after the
clean. `CacheIssue` will provide a path and an actionable message.

These report types will keep their fields private and provide documented
read-only accessors. `crates/libruskel/src/lib.rs` will re-export only the
high-level reports and their supporting value types. Layout, lease, retry,
garbage-collection, and scheduler types will remain private.

Normal `inspect`, `search`, `list`, `render`, and `raw_json` calls will use the
same cache handle through `Ruskel::load_target`. `crates/mcp/src/server.rs`
already clones its configured `Ruskel`. Those clones will share the cache
owner.

## Compatibility And Risks

The change preserves target resolution, Cargo feature selection, privacy,
offline behavior, binary-target selection, and rendered output. It does not
remove artifacts that older Ruskel versions created outside the new cache.
`README.md` will explain how users can remove those artifacts manually.

Cargo's global package-cache lock remains shared. Dependency resolution or
downloads can still delay other Cargo processes. The first query for each
workspace after this change or a nightly update will perform a cold build.

Advisory locks coordinate Ruskel processes. They do not protect the cache from
an external process that ignores them. External removal of the complete cache
cannot lose unique data, but it can make active requests fail.

This change does not alter Cargo incremental compilation, MCP cancellation, or
child-process lifecycle. It does not alter artifact reuse across distinct
workspace roots or cleanup inside one Cargo target directory.

## Execution Plan

### Stage 1: Isolate Rustdoc Builds

This stage makes every non-standard-library build use a validated cache entry
with bounded retry behavior.

- [x] Add `dirs`, `fs4`, and `sha2` to `Cargo.toml`,
  `crates/libruskel/Cargo.toml`, and `Cargo.lock`.
- [x] Add `crates/libruskel/src/cache/mod.rs` with cache configuration,
  reports, owner lookup, identities, and build leases.
- [x] Add `crates/libruskel/src/cache/layout.rs` with layout validation,
  initialization, stamps, traversal, trash renames, and no-follow deletion.
- [x] Add actionable cache failures to `crates/libruskel/src/error.rs` without
  hiding their paths or causes.
- [x] Re-export only `CacheStatus`, `CleanReport`, and their documented value
  types from `crates/libruskel/src/lib.rs`.
- [x] Add `Ruskel::with_cache_dir`, `Ruskel::cache_status`, and
  `Ruskel::clean_cache` in `crates/libruskel/src/ruskel.rs`.
- [x] Make `Ruskel::load_target` pass its cache handle through
  `CrateReadOptions` to `ResolvedTarget::read_crate`.
- [x] Extend `select_package_target` in `cargoutils.rs` to return the canonical
  `Workspace::root()` with target metadata.
- [x] Add the per-request nightly SHA-256 identity and post-build recheck in
  `crates/libruskel/src/toolchain.rs`.
- [x] Set `Builder::target_dir` and `CARGO_BUILD_BUILD_DIR` to the cache entry
  in `CargoPath::read_crate`.
- [x] Preserve Cargo diagnostic formatting while retaining internal retry
  categories until the retry decision.
- [x] Move damaged workspace entries to trash. Retry once without maintenance
  in Stage 1.
- [x] Implement independent one-retry budgets for toolchain changes and
  probable storage damage.
- [x] Add cache layout, identity, report, public API, retry, and symlink tests
  beside the owning modules.
- [x] Add `crates/libruskel/tests/cache.rs` to prove isolation, workspace
  sharing, standard-library bypass, and configuration precedence.
- [x] Run the exact cache unit tests and
  `cargo nextest run -p libruskel --test cache`.
- [x] Run `cargo check -p libruskel --all-targets` after the focused tests pass.
- [x] Review the Stage 1 diff and its adjacent error, target, and toolchain
  paths.

### Stage 2: Bound Cache Lifecycle

This stage adds coalesced maintenance, safe eviction, and deterministic shutdown
to the isolated cache.

- [x] Add `crates/libruskel/src/cache/maintenance.rs` with the worker, pending
  reason, shutdown, and cross-process GC lease.
- [x] Submit one normal or urgent maintenance signal after each
  non-standard-library build attempt.
- [x] Implement the 24-hour schedule, trash cleanup, one-hour old-toolchain
  retention, and 14-day workspace retention.
- [x] Implement the 20,000,000,000-byte high-water mark and
  15,000,000,000-byte eviction target.
- [x] Retain the newest workspace and report locked, invalid, future-dated,
  unrecognized, or failed entries.
- [x] Insert synchronous maintenance between the Stage 1 lease release and
  storage-recovery retry.
- [x] Make synchronous recovery wait for active GC. Do not run a duplicate pass
  after that wait.
- [x] Drain pending maintenance and join the worker when the final `CacheOwner`
  drops.
- [x] Add barrier-based thread tests for same-workspace serialization and
  different-workspace concurrency.
- [x] Add subprocess helper tests for root, toolchain, workspace, and GC lock
  behavior.
- [x] Add deterministic clock, space, and maintenance hooks for retention and
  budget tests.
- [x] Test interrupted initialization, interrupted trash deletion, partial
  clean failure, and an over-budget locked cache.
- [x] Run the exact maintenance unit tests and the complete `libruskel` package
  test suite.
- [x] Review the Stage 2 diff for lock order, worker ownership, deletion scope,
  and shutdown behavior.

### Stage 3: Expose And Document Cache Operations

This stage connects the cache to the CLI, MCP startup, documentation, and
workspace-wide proof.

- [x] Change `Cli::target` to `Option<String>` in `crates/ruskel/src/main.rs`.
  Preserve the `./` default.
- [x] Add `cache_dir`, `clean_cache`, and `cache_status` arguments with the
  specified Clap conflicts and environment binding.
- [x] Pass the selected cache directory through `ruskel_from_cli`, including
  the `--mcp` path.
- [x] Dispatch status and clean before `check_nightly_toolchain` and ordinary
  target handling.
- [x] Print deterministic status and clean reports with busy, skipped, failed,
  and excess states.
- [x] Return a failing CLI status when clean is busy or contains removal
  failures.
- [x] Extend `crates/ruskel/tests/cli.rs` with conflict, precedence, output,
  exit-status, and no-nightly cache-command tests.
- [x] Add a CLI unit test that proves `--cache-dir` is server-scoped and
  accepted by `Cli::mcp_defaults`.
- [x] Verify that `crates/mcp/src/server.rs` clones retain one cache owner.
  Do not add request-level cache controls.
- [x] Document cache selection, status, cleaning, retention, and legacy
  artifact removal in `README.md`.
- [x] Record the dedicated cache and its controls in `CHANGELOG.md`.
- [x] Run
  `cargo +nightly fmt --all -- --check --config-path ./rustfmt-nightly.toml`.
- [x] Run
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- [x] Run `cargo nextest run --workspace --all-features` without doctests.
- [x] Smoke-test local, registry, standard-library, clean, status, and MCP
  startup paths with a temporary cache.
- [x] Confirm project targets and registry sources receive no build artifacts
  during smoke tests.
- [x] Run `git diff --check` and review the final diff against every spec
  requirement.
