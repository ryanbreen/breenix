#!/usr/bin/env bash
# Build the real launcher executables used by the Python host tests.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
mkdir -p target/host-test-tools
scripts/build-background.sh rustc --edition=2021 -O scripts/metal/sha512-crypt.rs -o target/host-test-tools/sha512-crypt
scripts/build-background.sh swift build --package-path tools/breenix-runs --product breenix-runs --jobs "$CARGO_BUILD_JOBS"
scripts/build-background.sh cargo build -p xtask
scripts/build-background.sh cargo build --release --features testing,external_test_bins --bin qemu-uefi --bin qemu-bios
