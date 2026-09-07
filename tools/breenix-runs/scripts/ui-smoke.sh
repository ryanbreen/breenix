#!/usr/bin/env bash
set -euo pipefail
PACKAGE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
make -C "$PACKAGE_DIR" app
cd "$PACKAGE_DIR"
swift build --product breenix-runs -c release
BIN_DIR="$(swift build --show-bin-path -c release)"
FIXTURE_ROOT="$(mktemp -d "$PACKAGE_DIR/.build/ui-smoke-store.XXXXXX")"
trap 'rm -rf "$FIXTURE_ROOT"' EXIT
mkdir -p "$FIXTURE_ROOT/input/breenix_aarch64_strict_1" "$FIXTURE_ROOT/input/breenix_aarch64_strict_2"
# A composite of real fixture excerpts, not a newly measured guest boot.
# Both runs contain identical host facts so either row ordering exercises Traces.
for iteration in 1 2; do
    cat Tests/Fixtures/05-runtime-anti-vacuity-strict-serial.txt         Tests/Fixtures/fatal-regs-labelled-excerpt.txt         > "$FIXTURE_ROOT/input/breenix_aarch64_strict_$iteration/serial.txt"
    cp Tests/Fixtures/gate-boot-facts-positive.txt         "$FIXTURE_ROOT/input/breenix_aarch64_strict_$iteration/gate_boot_facts.txt"
done
BREENIX_RUNS_STORE="$FIXTURE_ROOT/store" "$BIN_DIR/breenix-runs" import "$FIXTURE_ROOT/input"
/usr/bin/swiftc Tests/UISmoke/main.swift -o "$PACKAGE_DIR/.build/ui-smoke"
"$PACKAGE_DIR/.build/ui-smoke" "$PACKAGE_DIR/Breenix Run Inspector.app" "$FIXTURE_ROOT/store"
