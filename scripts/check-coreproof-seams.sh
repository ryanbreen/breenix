#!/bin/bash
#
# Core-proof seam placement ratchet — the SOURCE half of the production-cleanliness
# pair (`tests/coreproof_production_clean.rs` is the binary half).
#
# The harness's charter carries two permanent exclusions, and this script is the
# mechanism behind them rather than the comment that promises them:
#
#   1. No `proof_point!` inside any Tier-1 file or any interrupt/syscall handler,
#      on either architecture. CLAUDE.md's Tier-1 list is the x86 spelling of the
#      rule; the AArch64 timer and exception paths are its moral twins and are
#      prohibited on the same footing.
#   2. No `proof_point!` in the ERET epilogue. It is the hot path, and a redirect
#      there would strand the thread and destroy the fault evidence a later rung
#      depends on. The pilot places no seam anywhere in either architecture's
#      context-switch file, so the rule is stated at file granularity: no line
#      pins, nothing that drifts when the file is edited. A later rung that wants
#      a seam there argues for it in its own PR and narrows this list there.
#
# The rule is a PROHIBITION census, never an allow-list of blessed seam sites: a
# new seam in an ordinary untiered file needs no edit here, and a new seam in a
# prohibited file cannot be added without deleting a line from this script in the
# same diff, where review will see it.
#
# Exit code: 0 = clean, 1 = a prohibited file carries a seam.
#
# Usage:
#   scripts/check-coreproof-seams.sh              # scan this repository
#   scripts/check-coreproof-seams.sh --root DIR   # scan a copy (used by --prove)
#   scripts/check-coreproof-seams.sh --prove      # anti-vacuity: plant one seam
#                                                 # in a prohibited file inside a
#                                                 # throwaway copy and require the
#                                                 # scan to go red.

set -euo pipefail
set -E

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ROOT="$BREENIX_ROOT"
PROVE=0

while [ $# -gt 0 ]; do
    case "$1" in
        --root) ROOT="$2"; shift 2 ;;
        --prove) PROVE=1; shift ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

# Files that may never carry a perturbation seam.
#
# Tier-1 (CLAUDE.md, "Absolutely Forbidden"): the syscall hot path, the timer
# handler, and their assembly entries.
# Tier-1 equivalents on AArch64: the timer interrupt, the exception vectors and
# the syscall entry, which the x86 list names by their x86 spelling only.
# Context switch, both architectures: the ERET/IRETQ epilogue lives here.
PROHIBITED=(
    "kernel/src/syscall/handler.rs"
    "kernel/src/syscall/time.rs"
    "kernel/src/syscall/entry.asm"
    "kernel/src/interrupts/timer.rs"
    "kernel/src/interrupts/timer_entry.asm"
    "kernel/src/interrupts/breakpoint_entry.asm"
    "kernel/src/interrupts/context_switch.rs"
    "kernel/src/arch_impl/aarch64/timer_interrupt.rs"
    "kernel/src/arch_impl/aarch64/exception.rs"
    "kernel/src/arch_impl/aarch64/syscall_entry.rs"
    "kernel/src/arch_impl/aarch64/syscall_entry.S"
    "kernel/src/arch_impl/aarch64/boot.S"
    "kernel/src/arch_impl/aarch64/context_switch.rs"
    "kernel/src/arch_impl/aarch64/context.rs"
)

# Any spelling of a seam. The macro form is the seam itself; the direct calls are
# the way around it, so both are prohibited in the same places.
SEAM_PATTERNS=(
    'proof_point!'
    'crate::proof::'
    'kernel::proof::'
)

scan() {
    local root="$1"
    local violations=0
    local missing=0
    local scanned=0

    for relative in "${PROHIBITED[@]}"; do
        local file="$root/$relative"
        if [ ! -f "$file" ]; then
            echo "MISSING: $relative (prohibited-file list is stale)"
            missing=$((missing + 1))
            continue
        fi
        scanned=$((scanned + 1))
        for pattern in "${SEAM_PATTERNS[@]}"; do
            if grep -nF "$pattern" "$file" >/dev/null 2>&1; then
                echo "VIOLATION: $relative carries a seam ($pattern)"
                grep -nF "$pattern" "$file" | sed 's/^/    /'
                violations=$((violations + 1))
            fi
        done
    done

    # A prohibited-file list that has drifted off the tree stops proving anything,
    # so a stale entry fails exactly like a seam does.
    if [ "$missing" -gt 0 ]; then
        echo "CORE-PROOF SEAM RATCHET: FAILED ($missing prohibited path(s) no longer exist)"
        return 1
    fi
    if [ "$violations" -gt 0 ]; then
        echo "CORE-PROOF SEAM RATCHET: FAILED ($violations violation(s))"
        return 1
    fi
    echo "CORE-PROOF SEAM RATCHET: PASSED (${scanned} prohibited path(s) clean)"
    return 0
}

if [ "$PROVE" -eq 1 ]; then
    # Anti-vacuity. A scan that cannot go red proves nothing, so plant exactly one
    # seam in one prohibited file inside a throwaway copy and require a red.
    TMP_ROOT="$(mktemp -d)"
    trap 'rm -rf "$TMP_ROOT"' EXIT
    mkdir -p "$TMP_ROOT/kernel/src/syscall"
    for relative in "${PROHIBITED[@]}"; do
        mkdir -p "$TMP_ROOT/$(dirname "$relative")"
        cp "$BREENIX_ROOT/$relative" "$TMP_ROOT/$relative"
    done
    printf '\n// planted by --prove\nfn _coreproof_prove_seam() { proof_point!(BlockEntry); }\n' \
        >> "$TMP_ROOT/kernel/src/syscall/handler.rs"

    if scan "$TMP_ROOT" >/dev/null 2>&1; then
        echo "CORE-PROOF SEAM RATCHET ANTI-VACUITY: FAILED"
        echo "A seam planted in kernel/src/syscall/handler.rs did not redden the scan."
        exit 1
    fi
    echo "CORE-PROOF SEAM RATCHET ANTI-VACUITY: PASSED (planted seam reddens the scan)"
    exit 0
fi

scan "$ROOT"
