#!/bin/bash
# Host tests of the BreenGL libraries: the development loop of the breengel lanes
# (docs/gui/breengel.md). Each crate's own unit tests run on the host with warnings
# denied, one test at a time (libgfx::work and libfont::work count every thread); the
# guest suite (/sbin/suite-breengel) gives the verdict.
#
# Usage: scripts/host-test-graphics.sh [crate-dir ...]   (default: every crate below)
#
# Each lane adds its own crates to CRATES.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CRATES=(
    libs/libgfx
    libs/libfont
    libs/libimg
    libs/libbui
    libs/libicon
)
if [[ $# -gt 0 ]]; then
    CRATES=("$@")
fi

export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
export CARGO_TARGET_DIR="${BREENIX_HOST_GRAPHICS_TARGET:-$ROOT/target/host-graphics}"
export RUSTFLAGS="-D warnings"
LOW=()
if [[ "$(uname)" == "Darwin" ]]; then
    LOW=(taskpolicy -b)
fi

failed=()
for crate in "${CRATES[@]}"; do
    manifest="$ROOT/$crate/Cargo.toml"
    if [[ ! -f "$manifest" ]]; then
        echo "==> $crate: no Cargo.toml at $manifest" >&2
        failed+=("$crate")
        continue
    fi
    echo "==> $crate"
    if ! (cd "$ROOT" && "${LOW[@]}" cargo test --manifest-path "$manifest" --quiet -- --test-threads=1); then
        failed+=("$crate")
    fi
done

if [[ ${#failed[@]} -gt 0 ]]; then
    echo "host graphics tests FAILED: ${failed[*]}" >&2
    exit 1
fi
echo "host graphics tests passed: ${CRATES[*]}"
