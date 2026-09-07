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
# Previously tests/*_structure.rs files were recompiled from scratch
# on gate calls, with no reuse. Current timing measurements belong in
# docs/planning/green-program/gates/
# GATE-TOOLING-PREFLIGHT-CACHE-890-V2-2026-09-06.md.
#
# Keys cover the sorted transitive source set (path modules and include!,
# include_str!, include_bytes!), rustc version, flags, and canonical checkout
# root. Missing references contribute path-specific sentinels. The default
# directory is ${TMPDIR:-/tmp}/breenix-structure-cache/<checkout-root hash>;
# an explicit BREENIX_STRUCTURE_CACHE is used verbatim. Cache artifacts stay
# outside target/ by default, avoiding the kernel artifact swap hazard.
#
# Hits require a matching key and a nonempty, executable binary whose --list
# succeeds and prints a ': test' line. A mkdir lock serializes validation,
# compilation, publication, and execution for each entry. After waiting we
# re-check the entry, compile only on a miss to private adjacent temporary
# files, then atomically rename the binary before publishing its key.
# Cache hits and misses both run the test binary.
#
# BREENIX_STRUCTURE_NO_CACHE=1: loud, caller-set bypass. Skips the cache
# in both directions -- reads and writes neither
# $BREENIX_STRUCTURE_CACHE -- compiling to a private, single-use temp path
# instead. For a caller that wants a fresh compile (e.g. testing this
# script itself) without disturbing a cache another caller may be relying
# on concurrently. Only the exact value 1 bypasses; other values (including 0) take the cached path.
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

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
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

if [[ "${BREENIX_STRUCTURE_NO_CACHE:-}" == "1" ]]; then
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

# Resolve existing paths without GNU-only flags. For missing descendants,
# canonicalize the closest existing ancestor and retain the missing suffix.
canonical_path() {
    local path="$1" parent base
    if command -v realpath >/dev/null 2>&1 && realpath "$path" 2>/dev/null; then
        return
    fi
    if [[ -d "$path" ]]; then
        (cd "$path" && pwd -P)
        return
    fi
    parent="$(dirname "$path")"
    base="$(basename "$path")"
    parent="$(canonical_path "$parent")"
    case "$base" in
        .) printf '%s\n' "$parent" ;;
        ..) dirname "$parent" ;;
        *) printf '%s/%s\n' "$parent" "$base" ;;
    esac
}

# Bash indexed arrays work on stock macOS bash as well as Linux bash.
# grep scans individual lines; escaped quotes in Rust string literals do
# not satisfy these literal-path patterns (including the closing delimiter).
source_set() {
    local path seen digest reference
    path="$(canonical_path "$1")"
    for seen in "${VISITED[@]:-}"; do
        [[ "$seen" != "$path" ]] || return 0
    done
    VISITED[${#VISITED[@]}]="$path"
    if [[ ! -f "$path" ]]; then
        digest="MISSING:$path"
        printf '%s:%s\n' "$path" "$digest"
        return
    fi
    digest="$(hash_stdin < "$path")"
    printf '%s:%s\n' "$path" "$digest"
    while IFS= read -r reference; do
        [[ -n "$reference" ]] || continue
        case "$reference" in
            /*) source_set "$reference" ;;
            *) source_set "$(dirname "$path")/$reference" ;;
        esac
    done < <(grep -oE '#\[path[[:space:]]*=[[:space:]]*"[^"\]*"\]|include(_str|_bytes)?![[:space:]]*\("[^"\]*"\)' "$path" |
        sed -E 's/^[^"]*"([^"]*)".*$/\1/')
}

REPO_ROOT="$(canonical_path "$REPO_ROOT")"
ROOT_HASH="$(printf '%s' "$REPO_ROOT" | hash_stdin)"
CACHE_DIR="${BREENIX_STRUCTURE_CACHE-${TMPDIR:-/tmp}/breenix-structure-cache/${ROOT_HASH}}"
mkdir -p "$CACHE_DIR"
BINARY="${CACHE_DIR}/${STEM}"
KEY_FILE="${BINARY}.key"
LOCK_DIR="${BINARY}.lock"
LOCK_HELD=0
cleanup() {
    rm -f "${BINARY}.tmp.$$" "${KEY_FILE}.tmp.$$"
    if [[ "$LOCK_HELD" -eq 1 ]]; then
        rmdir "$LOCK_DIR"
    fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

VISITED=()
SOURCE_HASH="$(source_set "$SOURCE" | LC_ALL=C sort | hash_stdin)"
RUSTC_VERSION="$(rustc --version --verbose)"
CACHE_KEY="$(printf '%s\n%s\n%s\n%s\n' "$SOURCE_HASH" "$RUSTC_VERSION" "$RUSTC_FLAGS" "$REPO_ROOT" | hash_stdin)"

# Lock hits too: a shared override can hold another checkout's binary,
# so retain ownership until our test run finishes, not just until rename.
LOCK_START=$SECONDS
until mkdir "$LOCK_DIR" 2>/dev/null; do
    if (( SECONDS - LOCK_START >= 300 )); then
        echo "structure-test cache: timed out waiting for $LOCK_DIR" >&2
        exit 4
    fi
    sleep 0.1
done
LOCK_HELD=1

# Read key and validate only AFTER acquiring the lock, including callers
# that waited while another process compiled this entry.
CACHE_HIT=0
if [[ -f "$KEY_FILE" ]]; then
    if [[ "$(cat "$KEY_FILE")" == "$CACHE_KEY" ]]; then
        # Consume --list output rather than grep -q's early close, avoiding
        # SIGPIPE under pipefail when a suite has many test names.
        if [[ -s "$BINARY" ]] && [[ -x "$BINARY" ]] &&
            "$BINARY" --list 2>/dev/null | grep ': test' >/dev/null; then
            CACHE_HIT=1
        else
            echo "== structure-test cache: miss (corrupt cached binary for ${STEM}) =="
        fi
    else
        echo "== structure-test cache: miss (stale cache key for ${STEM}) =="
    fi
else
    echo "== structure-test cache: miss (no cached binary for ${STEM} yet) =="
fi

if [[ "$CACHE_HIT" -eq 1 ]]; then
    echo "== structure-test cache: hit (reusing compiled ${STEM}) =="
else
    echo "== compiling ${STEM} =="
    CARGO_MANIFEST_DIR="$REPO_ROOT" rustc ${RUSTC_FLAGS} "$SOURCE" -o "${BINARY}.tmp.$$"
    mv -f "${BINARY}.tmp.$$" "$BINARY"
    printf '%s' "$CACHE_KEY" > "${KEY_FILE}.tmp.$$"
    mv -f "${KEY_FILE}.tmp.$$" "$KEY_FILE"
fi

echo "== running ${STEM} ${FILTER} =="
if [[ -n "$FILTER" ]]; then
    "$BINARY" "$FILTER" --nocapture
else
    "$BINARY"
fi
