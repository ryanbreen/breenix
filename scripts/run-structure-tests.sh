#!/usr/bin/env bash
#
# Run a host-side structural ratchet file without going through `cargo test`.
#
# WHY THIS EXISTS (review round 1 of #789 slice 2, finding M3)
#
# `cargo test -p breenix --test teardown_structure` cannot reach these tests on
# a machine that lacks the forked Rust library: the root crate's build script
# runs userspace/programs/build.sh, which stops with
# "ERROR: forked Rust library not found at <repo>/rust-fork/library". That is a
# build DEPENDENCY failure, so it happens before the integration test is
# compiled, and it is unrelated to what the test reads. The structure test files
# are std-only and depend on no crate in this workspace -- they read the tree
# from disk -- so `rustc --test` compiles and runs them directly.
#
# WHAT THIS DOES AND DOES NOT DO
#
# As of R191/PR-1 (docs/planning/green-program/gates/
# GATE-TOOLING-STRUCTURE-PREFLIGHT-PR1-2026-09-06.md), this script DOES run in
# a gate: docker/qemu/lib/gate-structure-preflight.sh's
# gate_structure_preflight function calls it once per discovered
# tests/*_structure.rs file, and all four boot gates (4/4, pinned by
# tests/gate_structure_preflight_wiring_structure.rs) --
# run-aarch64-boot-test-strict.sh, run-aarch64-prod-profile-boot-test.sh,
# run-x86-boot-tests.sh and run-x86-prod-profile-boot-test.sh -- source that
# lib and call the function before building or booting anything, so a ratchet
# in one of these files is now enforced on every one of those four gate runs,
# not only when a person or an agent runs this script by hand. This
# repository still has no GitHub Actions CI, so nothing runs a gate on every
# commit automatically; the ratchet's enforcement is tied to a gate
# invocation, whoever or whatever triggers it.
#
# Usage:
#   scripts/run-structure-tests.sh                       # discovered suites, shared preflight
#   scripts/run-structure-tests.sh teardown_structure    # one file, whole file
#   scripts/run-structure-tests.sh teardown_structure scheduler_lock
#                                                        # one file, filtered
#
# Exit status is the test binary's own: 0 on success, non-0 on failure.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ $# -eq 0 ]]; then
    source "$REPO_ROOT/docker/qemu/lib/gate-structure-preflight.sh"
    gate_structure_preflight "$REPO_ROOT" "${BREENIX_GATE_TMP:-${TMPDIR:-/tmp}}"
    exit $?
fi
STEM="$1"
FILTER="${2:-}"

SOURCE="${REPO_ROOT}/tests/${STEM}.rs"
if [[ ! -f "${SOURCE}" ]]; then
    echo "no such structure test file: ${SOURCE}" >&2
    exit 2
fi

OUT_DIR="${TMPDIR:-/tmp}/breenix-structure-tests"
mkdir -p "${OUT_DIR}"
BINARY="${OUT_DIR}/${STEM}"

echo "== compiling ${STEM} =="
# A restricted system PATH can omit rustup even when the toolchain is installed.
RUSTC_BIN="$(command -v rustc || true)"
if [[ -z "$RUSTC_BIN" ]]; then
    RUSTC_BIN="${CARGO_HOME:-$HOME/.cargo}/bin/rustc"
fi
CARGO_MANIFEST_DIR="${REPO_ROOT}" "$RUSTC_BIN" --edition=2021 --test "${SOURCE}" -o "${BINARY}"

echo "== running ${STEM} ${FILTER} =="
if [[ -n "${FILTER}" ]]; then
    "${BINARY}" "${FILTER}" --nocapture
else
    "${BINARY}"
fi
