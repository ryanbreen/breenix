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
# WHAT THIS DOES NOT DO
#
# It does not run in any gate. No gate script under docker/qemu/ invokes it, and
# this repository has no GitHub Actions CI, so a ratchet in one of these files
# is enforced only when a person or an agent runs this script. Wiring it into a
# gate is a separate change with its own review.
#
# Usage:
#   scripts/run-structure-tests.sh                       # teardown_structure, whole file
#   scripts/run-structure-tests.sh teardown_structure    # one file, whole file
#   scripts/run-structure-tests.sh teardown_structure scheduler_lock
#                                                        # one file, filtered
#
# Exit status is the test binary's own: 0 on success, non-0 on failure.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STEM="${1:-teardown_structure}"
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
CARGO_MANIFEST_DIR="${REPO_ROOT}" rustc --edition=2021 --test "${SOURCE}" -o "${BINARY}"

echo "== running ${STEM} ${FILTER} =="
if [[ -n "${FILTER}" ]]; then
    "${BINARY}" "${FILTER}" --nocapture
else
    "${BINARY}"
fi
