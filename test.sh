#!/usr/bin/env bash
#
# The gate: run this, not bare `cargo test`.

set -euo pipefail

cd "$(dirname "$0")"

cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
