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
# COMPILE CACHE (#890)
#
# Before this round, this script recompiled each `tests/*_structure.rs`
# file from scratch on each call: no mtime check, no reuse across the
# four gates' four separate invocations. Measured cost (docs/planning/
# green-program/gates/GATE-TOOLING-STRUCTURE-PREFLIGHT-PR1-2026-09-06.md,
# "Isolated preflight time cost"): ~106s wall-clock on an Apple Silicon Mac
# and ~6m20s on the beast x86 Incus container, per gate invocation, paid in
# full on each of the four gates' runs.
#
# The cache below keys each compiled test binary on three things: the
# compiled source file's own content hash, `rustc --version --verbose`
# (so a toolchain change invalidates a cached binary the next time it is
# checked, rather than reusing one built by a different compiler), and the
# exact rustc flags this script passes (RUSTC_FLAGS below) -- a change to
# any of the three is a cache miss. The cache lives under
# ${BREENIX_STRUCTURE_CACHE:-${TMPDIR:-/tmp}/breenix-structure-cache}, a
# directory this script owns outside this repository's `target/` tree
# altogether. That placement is deliberate, not incidental: the kernel-swap
# hazard documented in docker/qemu/lib/gate-structure-preflight.sh (a
# `cargo test` in the same shell session hardlinking a fresh, wrongly-
# featured kernel binary over one a gate's build step just produced,
# because both live under `target/`) depends on the artifact being under
# `target/` in the first place. No file this script reads or writes -- the
# cached test binaries, their key files -- lives under `target/`, so that
# hazard does not apply to this cache; it is a separate, ordinary
# directory under $TMPDIR that no build step touches.
#
# A cache hit skips compilation only -- the compiled binary is still run
# on each call, exactly as before; test *execution* itself is not cached
# or skipped.
#
# A hit requires the stored key to match AND the cached binary to pass a
# cheap `--list` sanity check (a rustc `--test` harness binary supports
# this flag and exits 0 on it as long as the binary is intact); a binary
# that fails that check -- corrupt, truncated, built for the wrong
# platform, deleted out from under a stale key file -- is treated as a
# miss and recompiled, same as a stale key.
#
# BREENIX_STRUCTURE_NO_CACHE=1: loud, caller-set bypass. Skips the cache
# in both directions -- reads and writes neither
# $BREENIX_STRUCTURE_CACHE -- compiling to a private, single-use temp path
# instead. For a caller that wants a fresh compile (e.g. testing this
# script itself) without disturbing a cache another caller may be relying
# on concurrently.
#
# WHAT THIS DOES NOT CACHE: `rustc`'s own incremental/dep-info state, the
# content of the fixture/data files a structure test reads from the repo at
# *run* time (those are read fresh on each run, cache hit or not -- only
# the compiled *test binary* is reused), or anything about the four gates'
# own `[GATE_PREFLIGHT:...]` summary line, which this script does not print
# and does not change the format of.
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

RUSTC_FLAGS="--edition=2021 --test"

# sha256 of stdin, tried in two spellings since the two hosts these gates
# actually run on (Apple Silicon Mac, beast's Linux Incus container) don't
# agree on which is installed.
hash_stdin() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 | awk '{print $1}'
    else
        echo "no sha256 tool found (need sha256sum or shasum) -- cannot compute the structure-test cache key" >&2
        exit 3
    fi
}

if [[ -n "${BREENIX_STRUCTURE_NO_CACHE:-}" ]]; then
    # True bypass: reads and writes no part of the cache directory, so this
    # run cannot invalidate or race a cache another concurrent caller relies
    # on. Compiles to a private, single-use path instead.
    echo "== structure-test cache: bypass (BREENIX_STRUCTURE_NO_CACHE set) =="
    NOCACHE_BINARY="$(mktemp "${TMPDIR:-/tmp}/breenix-structure-nocache-XXXXXX")"
    trap 'rm -f "${NOCACHE_BINARY}"' EXIT
    echo "== compiling ${STEM} =="
    CARGO_MANIFEST_DIR="${REPO_ROOT}" rustc ${RUSTC_FLAGS} "${SOURCE}" -o "${NOCACHE_BINARY}"
    echo "== running ${STEM} ${FILTER} =="
    if [[ -n "${FILTER}" ]]; then
        "${NOCACHE_BINARY}" "${FILTER}" --nocapture
    else
        "${NOCACHE_BINARY}"
    fi
    exit $?
fi

CACHE_DIR="${BREENIX_STRUCTURE_CACHE:-${TMPDIR:-/tmp}/breenix-structure-cache}"
mkdir -p "${CACHE_DIR}"
BINARY="${CACHE_DIR}/${STEM}"
KEY_FILE="${CACHE_DIR}/${STEM}.key"

SOURCE_HASH="$(hash_stdin < "${SOURCE}")"
RUSTC_VERSION="$(rustc --version --verbose)"
CACHE_KEY="$(printf '%s\n%s\n%s\n' "${SOURCE_HASH}" "${RUSTC_VERSION}" "${RUSTC_FLAGS}" | hash_stdin)"

CACHE_HIT=0
if [[ -f "${KEY_FILE}" && -x "${BINARY}" ]]; then
    STORED_KEY="$(cat "${KEY_FILE}")"
    if [[ "${STORED_KEY}" == "${CACHE_KEY}" ]]; then
        if "${BINARY}" --list >/dev/null 2>&1; then
            CACHE_HIT=1
        else
            echo "== structure-test cache: miss (cached binary for ${STEM} failed a --list sanity check -- treating as corrupt) =="
        fi
    else
        echo "== structure-test cache: miss (stale cache key for ${STEM} -- source, rustc version, or flags changed) =="
    fi
else
    echo "== structure-test cache: miss (no cached binary for ${STEM} yet) =="
fi

if [[ "${CACHE_HIT}" -eq 1 ]]; then
    echo "== structure-test cache: hit (reusing compiled ${STEM}) =="
else
    echo "== compiling ${STEM} =="
    CARGO_MANIFEST_DIR="${REPO_ROOT}" rustc ${RUSTC_FLAGS} "${SOURCE}" -o "${BINARY}"
    printf '%s' "${CACHE_KEY}" > "${KEY_FILE}"
fi

echo "== running ${STEM} ${FILTER} =="
if [[ -n "${FILTER}" ]]; then
    "${BINARY}" "${FILTER}" --nocapture
else
    "${BINARY}"
fi
