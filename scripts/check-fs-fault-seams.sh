#!/bin/bash
#
# ext2/VFS fault-injection seam ratchet — the SOURCE half of the pair
# (`scripts/check-fs-fault-production-clean.sh` is the binary half).
#
# The binary half proves a production ELF carries none of the leg. This half
# protects the property that makes that true and would be the first thing to
# rot: every reference to the leg outside its own module is cfg-guarded, and no
# reference to it exists in a hot path at all.
#
#   RULE A (cfg census): every mention of `fault_inject` in kernel/src outside
#   kernel/src/fs/fault_inject.rs is either the `#[cfg(feature =
#   "fs_fault_inject")]` attribute itself, a comment, or a line whose preceding
#   non-blank line is that attribute. An unguarded call is what would put the
#   leg into a production build, and it is exactly what this catches.
#
#   RULE B (prohibition census): the leg is never referenced from a Tier-1 or
#   interrupt/syscall file, on either architecture. It reads a block device and
#   allocates; it has no business anywhere near those paths, and the rule is
#   stated at FILE granularity so nothing drifts when those files are edited.
#
# Like the core-proof seam ratchet, RULE B is a PROHIBITION census, never an
# allow-list of blessed call sites: a new call site in an ordinary file needs no
# edit here, and a call site in a prohibited file cannot be added without
# deleting a line from this script in the same diff, where review will see it.
#
# Exit code: 0 = clean, 1 = an unguarded or prohibited reference exists.
#
# Usage:
#   scripts/check-fs-fault-seams.sh             # scan this repository
#   scripts/check-fs-fault-seams.sh --root DIR  # scan a copy (used by --prove)
#   scripts/check-fs-fault-seams.sh --prove     # anti-vacuity: plant an
#                                               # unguarded call in a throwaway
#                                               # copy and require a red scan

set -uo pipefail

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

# Files that may never reference the leg. Tier-1 per CLAUDE.md ("Absolutely
# Forbidden"), plus their AArch64 equivalents, plus both context-switch files.
PROHIBITED=(
    "kernel/src/syscall/handler.rs"
    "kernel/src/syscall/time.rs"
    "kernel/src/syscall/entry.asm"
    "kernel/src/interrupts/timer.rs"
    "kernel/src/interrupts/timer_entry.asm"
    "kernel/src/interrupts/context_switch.rs"
    "kernel/src/arch/aarch64/timer.rs"
    "kernel/src/arch/aarch64/exceptions.rs"
    "kernel/src/arch/aarch64/context_switch.rs"
)

MODULE_REL="kernel/src/fs/fault_inject.rs"
# Spellings that name THIS leg. Deliberately not the bare word `fault_inject`:
# the unrelated, pre-existing `ec0_fault_inject` feature contains it as a
# substring, and a needle that swept that in would make this ratchet a
# permanent red rather than a check.
NEEDLE='fs_fault_inject|fs::fault_inject|run_fs_fault_leg|mod fault_inject'
CFG_ATTR='#\[cfg\(feature = "fs_fault_inject"\)\]'

scan() {
    local root="$1"
    local violations=0

    # ---- RULE A ----------------------------------------------------------
    # One awk pass per file: remember whether the previous non-blank line was the
    # cfg attribute, so a guarded reference on the following line is accepted.
    local unguarded
    unguarded="$(find "$root/kernel/src" -name '*.rs' -type f ! -path "*/fs/fault_inject.rs" -print0 \
        | xargs -0 awk '
            FNR == 1 { guarded = 0 }
            {
                line = $0
                trimmed = line
                sub(/^[ \t]*/, "", trimmed)
                if (trimmed == "") next
                if (line ~ /#\[cfg\(feature = "fs_fault_inject"\)\]/) { guarded = 1; next }
                if (line ~ /fs_fault_inject|fs::fault_inject|run_fs_fault_leg|mod fault_inject/) {
                    is_comment = (trimmed ~ /^\/\// || trimmed ~ /^\/\*/ || trimmed ~ /^\*/)
                    if (!is_comment && !guarded) printf "  UNGUARDED: %s:%d: %s\n", FILENAME, FNR, trimmed
                }
                guarded = 0
            }
        ')"
    if [ -n "$unguarded" ]; then
        echo "$unguarded" | sed "s|$root/||"
        violations=$(( violations + $(echo "$unguarded" | grep -c .) ))
    fi

    # ---- RULE B ----------------------------------------------------------
    local prohibited
    for prohibited in "${PROHIBITED[@]}"; do
        local path="$root/$prohibited"
        [ -f "$path" ] || continue
        if grep -qnE "$NEEDLE" "$path"; then
            echo "  PROHIBITED FILE: $prohibited references the fault-injection leg"
            grep -nE "$NEEDLE" "$path" | head -5 | sed 's/^/    /'
            violations=$((violations + 1))
        fi
    done

    echo "$violations" > "$root/.fs-fault-seam-violations"
    return 0
}

if [ "$PROVE" -eq 1 ]; then
    WORK="$(mktemp -d)"
    trap 'rm -rf "$WORK"' EXIT
    echo "Staging a copy and planting an unguarded call in an ordinary file..."
    ( cd "$BREENIX_ROOT" && tar --exclude='./target' --exclude='./.git' -cf - kernel/src ) \
        | ( cd "$WORK" && tar -xf - )
    PLANT="$WORK/kernel/src/fs/vfs/mod.rs"
    if [ ! -f "$PLANT" ]; then
        echo "FS-FAULT SEAM RATCHET ANTI-VACUITY: FAILED (no file to plant into)"
        exit 1
    fi
    printf '\nfn planted_unguarded_seam() { crate::fs::fault_inject::run_fs_fault_leg(); }\n' >> "$PLANT"
    scan "$WORK" >/dev/null
    PLANTED_VIOLATIONS="$(cat "$WORK/.fs-fault-seam-violations")"
    if [ "$PLANTED_VIOLATIONS" -lt 1 ]; then
        echo "FS-FAULT SEAM RATCHET ANTI-VACUITY: FAILED"
        echo "An unguarded call to the leg was not detected, so a clean verdict means nothing."
        exit 1
    fi
    echo "FS-FAULT SEAM RATCHET ANTI-VACUITY: PASSED (planted unguarded call detected)"
    exit 0
fi

echo "Scanning $ROOT for unguarded or prohibited references to the fault-injection leg..."
scan "$ROOT"
VIOLATIONS="$(cat "$ROOT/.fs-fault-seam-violations")"
rm -f "$ROOT/.fs-fault-seam-violations"
if [ "$VIOLATIONS" -ne 0 ]; then
    echo "FS-FAULT SEAM RATCHET: FAILED ($VIOLATIONS violation(s))"
    exit 1
fi
echo "FS-FAULT SEAM RATCHET: PASSED (every reference outside the module is cfg-guarded; no hot path references it)"
exit 0
