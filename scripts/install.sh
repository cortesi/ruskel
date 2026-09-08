#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

cargo install --locked --path "$repo_root/crates/ruskel" "$@"
cargo install --locked --path "$repo_root/crates/ruskel-snapshot" "$@"
