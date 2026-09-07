# Canonical API snapshots

`ruskel-snapshot` records the public library API of selected Cargo packages.
The generated directory is stable input for Git history and review.

## Create a snapshot

Install the nightly toolchain and its `rustfmt` component. Ruskel selects the
toolchain's host target unless you pass `--target`.

```sh
rustup toolchain install nightly --component rustfmt
ruskel-snapshot \
  --output ./api \
  ./crates/*
```

Each input is a `Cargo.toml` file or a directory that contains one. A workspace
manifest selects all workspace members. A package manifest selects only that
package. The shell expands globs before Ruskel receives the paths.

The first capture stores `nightly`, its current host target, and the feature
policy in `api/.ruskel-snapshot.toml`. Later commands reuse omitted profile
values. Updating the nightly toolchain can change the generated snapshot:

```sh
ruskel-snapshot --output ./api ./crates/*
ruskel-snapshot --offline --output ./api ./crates/*
```

All selected workspaces need a usable `Cargo.lock`. Capture runs Cargo with
`--locked`. Offline capture also prevents Cargo from using the network.

## Change a profile

Pass an explicit value to migrate one stored profile field. The new marker
makes the migration visible in Git.

```sh
ruskel-snapshot \
  --toolchain nightly \
  --target aarch64-apple-darwin \
  --output ./api \
  ./crates/*
```

The `--toolchain nightly` migration moves an older dated profile back to the
rolling nightly channel. A dated nightly remains available as an explicit
override when a project chooses to pin one.

Use `--features package/feature` for a multi-package capture. Separate multiple
features with commas. An unqualified feature is valid only when the command
selects one package.

```sh
ruskel-snapshot --features libruskel/extra,ruskel-mcp/tls --output ./api ./crates/*
ruskel-snapshot --no-default-features --features libruskel/extra --output ./api ./crates/*
```

`--all-features` conflicts with `--no-default-features` and `--features`.
`--no-default-features` can be used with `--features`.

## Snapshot lock

Ruskel keeps a persistent advisory lock beside each destination, named
`.<destination-name>.ruskel-snapshot.lock`. For a root `api/` destination, the
lock beside the directory is `.api.ruskel-snapshot.lock`; add the following
path to `.gitignore` when the repository stores its snapshot at the repository
root:

```gitignore
/.api.ruskel-snapshot.lock
```

The lock coordinates update and check commands that use the same physical
destination. Check mode can create this coordination file even though it never
changes the generated destination. The file remains in place after each run so
later processes continue to use the same lock identity.

## Ownership and reports

The marker owns only the crate files that it names. Update mode refuses a
nonempty unmarked directory, unexpected files, destination symlinks, and owned
file symlinks. Do not put hand-written files in the generated directory.

The command prints the marker first. It then prints managed crate files,
removed or unexpected paths, interrupted backups, and skipped packages. The
status field has these values:

- `changed`: the file is new or has different bytes.
- `unchanged`: the stored and captured bytes are identical.
- `removed`: the old marker owned a file that the new capture does not need.
- `unexpected`: the marker does not own a path in the destination.
- `interrupted`: a validated backup shows an incomplete directory swap.
- `skipped`: a selected package has no library or procedural-macro target.

Binary-only packages do not fail a mixed capture. Capture fails if all selected
packages are binary-only. Public items with `#[doc(hidden)]` remain in the
snapshot because Rust visibility defines the captured surface.

Update mode writes a complete sibling tree and swaps it into place. A process
stop between the two renames can leave one validated backup. The next update
restores or removes that backup. Check mode never performs recovery. It reports
the backup as `interrupted` drift.

## Check and automate snapshots

Check mode performs capture and comparison without changing the destination:

```sh
ruskel-snapshot --check --output ./api ./crates/*
```

Status 0 means that the snapshot is current. Status 1 means that check mode
found drift. Status 2 means that arguments, discovery, capture, profile, or
storage failed.

This pre-commit hook rejects unstaged snapshot inputs. It updates `api/` but
never stages files. The commit stops if the generated change is not staged.

```sh
#!/bin/sh
set -eu

if ! git diff --quiet -- Cargo.lock ':(glob)**/Cargo.toml' ':(glob)crates/**/*.rs' ||
   test -n "$(git ls-files --others --exclude-standard -- Cargo.lock ':(glob)**/Cargo.toml' ':(glob)crates/**/*.rs')"
then
    echo "snapshot inputs have unstaged changes; stage or restore them first" >&2
    exit 1
fi

ruskel-snapshot --output ./api ./crates/*

if ! git diff --quiet -- api ||
   test -n "$(git ls-files --others --exclude-standard -- api)"
then
    echo "api snapshot changed; review and stage api/ before committing" >&2
    exit 1
fi
```

For partial staging, run capture against a temporary checkout of the index.
The second checkout preserves the staged snapshot for comparison.

```sh
#!/bin/sh
set -eu

work=$(mktemp -d "${TMPDIR:-/tmp}/ruskel-snapshot-index.XXXXXX")
trap 'rm -rf "$work"' EXIT HUP INT TERM
mkdir "$work/source" "$work/expected"
git checkout-index --all --prefix="$work/source/"
git checkout-index --all --prefix="$work/expected/"
(
    cd "$work/source"
    ruskel-snapshot --output ./api ./crates/*
)
diff -ru "$work/expected/api" "$work/source/api"
```

CI uses the same package selection in check mode:

```sh
ruskel-snapshot --check --output ./api ./crates/*
```

Ruskel does not install hooks, stage files, or modify Git configuration.
