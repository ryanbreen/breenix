#!/bin/bash
# Host-side structure-suite + critical-path-census preflight, shared by the
# four boot gates R191/PR-1 (the gate-tooling round) wires it into:
# run-aarch64-boot-test-strict.sh, run-aarch64-prod-profile-boot-test.sh,
# run-x86-boot-tests.sh and run-x86-prod-profile-boot-test.sh. See
# docs/planning/green-program/gates/GATE-TOOLING-STRUCTURE-PREFLIGHT-PR1-2026-09-06.md.
#
# Before this round, no gate script under docker/qemu/ invoked
# scripts/run-structure-tests.sh or `cargo test --test <structure-suite>`, so
# the tests/*_structure.rs ratchets -- including tests/
# critical_path_logging_census_structure.rs's pin on `scripts/
# check-critical-path-violations.sh` -- were enforced only by a person or an
# agent running them by hand (docs/planning/green-program/gates/
# CRITICAL-PATH-DEBT-PR0-ROUND-2026-09-06.md, "What this round deliberately
# did not do"). This file is that separate, reviewed wiring.
#
# WHY `scripts/run-structure-tests.sh`, never `cargo test`, in a gate script:
# run-aarch64-boot-test-strict.sh's own `require_boot_tests_kernel` comment
# records that ANY `cargo test` in the same shell session hardlinks a fresh
# kernel binary -- built with none of that gate's required features -- over
# the one its own preceding build step produced, in well under a second and
# with no output announcing the swap; a measured acceptance battery that ran
# the structural suites via `cargo test` and then that gate scored 0/6 on a
# kernel that was never asked to emit the gate's boot_tests-only markers.
# `scripts/run-structure-tests.sh` sidesteps this by construction, not by
# convention: it compiles one `tests/<stem>.rs` file directly with
# `rustc --test` (that script's own header), which reads no `Cargo.toml`,
# runs no build script, and writes its output binary under `$TMPDIR` --
# nothing under this repository's `target/` is read or written by that path,
# so there is no kernel artifact for it to touch, let alone swap. This
# preflight is the caller of that script, so it inherits the same property;
# each of the four callers below places it before its own kernel build step
# regardless, so the two never interleave in the first place.
#
# COST, measured (not asserted): step (a) below recompiles all 47 (47/47
# discovered at the time of writing) `tests/*_structure.rs` suites from
# scratch on every call -- `scripts/run-structure-tests.sh` has no
# mtime/staleness check, so nothing is cached or reused across the four
# gates' four separate invocations of this function. Standalone (this
# function alone, outside any boot loop), freshly measured on two hosts:
# ~127s wall-clock on an Apple Silicon Mac (macOS, native rustc), and
# ~331s (5m31s) wall-clock in the beast x86 Incus container (Linux, the
# actual host run-x86-boot-tests.sh and run-x86-prod-profile-boot-test.sh
# execute in for merge-gating) -- see the "Isolated preflight time cost"
# section of docs/planning/green-program/gates/
# GATE-TOOLING-STRUCTURE-PREFLIGHT-PR1-2026-09-06.md for the full method
# and both hosts' numbers. This is paid in full on every one of the four
# gates' runs; on the x86 host it adds several minutes on top of that
# gate's own boot loop.
#
# BREENIX_GATE_SKIP_STRUCTURE=1: loud, operator-set opt-out. Skips both
# steps below (no suite runs, no census-count read) and prints a
# `[GATE_PREFLIGHT:skipped=1:reason=...]` line instead of the scored one --
# for a caller that already ran the suites itself earlier in the same
# session (e.g. a launcher driving several of these four gates back to
# back), or a host that cannot build them (see the automatic
# `rustc`-availability check below, which sets the same reason field on its
# own if the operator did not set this variable).
#
# gate_structure_preflight <repo_root> <gate_tmp> is the one function each
# caller uses. It does not call `exit` or `return` on its own -- the caller
# decides what status to act on -- because the four gates below carry three
# different verdict idioms between them (an installed ERR trap fired via
# `false`; a bare `set -e` abort with no trap this early), so this function
# only prints the `[GATE_PREFLIGHT:...]` line and returns 0 (no suite red,
# or the loud skip) or 1 (at least one suite red, or the
# discovery itself found no suite to run); each caller decides how its own
# gate reports that 1 loudly.
#
# critical_path_lines=<n> is `scripts/check-critical-path-violations.sh`'s
# own total stdout line count -- the same "N lines of grep output" framing
# tests/critical_path_logging_census_structure.rs's own header comment uses.
# It is printed for a human reading gate output and is NOT itself a gate: that
# script exits 1 on purpose today (135 real call sites the census suite pins,
# per the drain plan) and will keep doing so until the drain reaches 0, so
# treating its exit code as a second gate would make each of these four
# gates' runs permanently red. The actual enforcement -- a per-(file,item-path)
# census that may not exceed its pinned count, and per that suite's own
# doc comment may not fall below it either without a conscious table update
# -- is tests/critical_path_logging_census_structure.rs's own job, and it is
# one of the suites step (a) already runs and gates on.
#
# pinned=<n> is read from that same suite's source text -- the WIDER
# census's total (CRITICAL_PATH_LOG_ANCHORS' summed third field, plus
# ESCAPED_SITE's own count; 136 at the time this preflight was written) --
# parsed out of the file rather than hardcoded here, so this number moves
# with the suite instead of drifting from it if a future PR's drain edits
# the table. The WIDER total, not the narrower 135-only one, is the
# comparable figure: check-critical-path-violations.sh's PROHIBITED_PATTERNS
# already carries the three spellings the wider census adds (both were
# widened together by the PR-0 round that added this suite), so
# critical_path_lines is counting against that same widened pattern set.

gate_structure_preflight() {
    local repo_root="$1"
    local gate_tmp="${2:-/tmp}"

    if [ -n "${BREENIX_GATE_SKIP_STRUCTURE:-}" ]; then
        echo "[GATE_PREFLIGHT:skipped=1:reason=BREENIX_GATE_SKIP_STRUCTURE set]"
        return 0
    fi

    local runner="$repo_root/scripts/run-structure-tests.sh"
    if [ ! -f "$runner" ]; then
        echo "[GATE_PREFLIGHT:skipped=1:reason=scripts/run-structure-tests.sh not found at $runner]"
        return 0
    fi
    if ! command -v rustc >/dev/null 2>&1; then
        echo "[GATE_PREFLIGHT:skipped=1:reason=rustc not found on PATH -- cannot compile tests/*_structure.rs]"
        return 0
    fi

    local tests_dir="$repo_root/tests"
    local log_dir="$gate_tmp/breenix_gate_structure_preflight"
    rm -rf "$log_dir" 2>/dev/null || true
    mkdir -p "$log_dir" 2>/dev/null || true

    local total=0
    local green=0
    local red_stems=""
    local stem
    while IFS= read -r stem; do
        [ -n "$stem" ] || continue
        total=$((total + 1))
        if bash "$runner" "$stem" >"$log_dir/$stem.log" 2>&1; then
            green=$((green + 1))
        else
            red_stems="$red_stems $stem"
        fi
    done < <(cd "$tests_dir" 2>/dev/null && find . -maxdepth 1 -type f -name '*_structure.rs' -print 2>/dev/null | sed 's|^\./||; s/\.rs$//' | sort)

    local critical_path_lines=0
    local checker="$repo_root/scripts/check-critical-path-violations.sh"
    if [ -f "$checker" ]; then
        critical_path_lines="$(bash "$checker" >"$log_dir/check-critical-path-violations.log" 2>&1; wc -l <"$log_dir/check-critical-path-violations.log" | tr -d ' ')"
    fi

    local pinned=0
    local census_file="$tests_dir/critical_path_logging_census_structure.rs"
    if [ -f "$census_file" ]; then
        local narrow escaped
        narrow="$(awk '/^const CRITICAL_PATH_LOG_ANCHORS/,/^\];$/' "$census_file" | grep -oE '[0-9]+\),$' | grep -oE '^[0-9]+' | awk '{s+=$1} END{print s+0}')"
        escaped="$(awk '/^const ESCAPED_SITE/,/^\);$/' "$census_file" | grep -oE '^[[:space:]]*[0-9]+,$' | grep -oE '[0-9]+')"
        pinned=$((narrow + ${escaped:-0}))
    fi

    echo "[GATE_PREFLIGHT:structure_suites=$green/$total:critical_path_lines=$critical_path_lines:pinned=$pinned]"

    if [ "$total" -eq 0 ]; then
        echo "GATE_PREFLIGHT: FAIL (found 0 tests/*_structure.rs files under $tests_dir -- discovery itself is broken, not just red)" >&2
        return 1
    fi
    if [ "$green" -ne "$total" ]; then
        echo "GATE_PREFLIGHT: FAIL ($((total - green)) of $total structure suite(s) red:$red_stems -- per-suite logs under $log_dir)" >&2
        return 1
    fi
    return 0
}
