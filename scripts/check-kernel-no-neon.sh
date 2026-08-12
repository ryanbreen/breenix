#!/bin/bash
#
# check-kernel-no-neon.sh — durable guard against re-arming issue #528.
#
# The aarch64 kernel MUST be built with the soft-float target
# aarch64-breenix-kernel.json ("features": "...,-neon,-fp-armv8",
# "abi": "softfloat"). A correctly-built kernel therefore contains ZERO
# vector/FP load or store instructions in its .text sections.
#
# If the kernel is ever built with the userspace NEON hardfloat target
# (aarch64-breenix.json) by mistake — as throwaway gate scripts did, silently
# re-arming #528 (see the #470 PR-1c RCA) — compiler-builtins' memcpy/memset
# and ordinary register spills begin using q/d/s/v registers on the kernel
# stack before the FPU trap is configured, producing the #528 fault class.
#
# This guard objdumps the kernel ELF, walks every .text* section, and FAILS
# if any FP/SIMD (q/d/s/v) load or store instruction appears in kernel code
# outside the documented allowlist (scripts/kernel-neon-allowlist.txt, which
# is intentionally empty — kernel code must be zero).
#
# Exit code:
#   0 - clean (no non-allowlisted FP/SIMD load/store in kernel .text)
#   1 - guard tripped (offending instructions found)  OR  usage/tooling error
#
# Usage:
#   ./scripts/check-kernel-no-neon.sh [path/to/kernel-elf]
# Default ELF: target/aarch64-breenix-kernel/release/kernel-aarch64

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

KERNEL_ELF="${1:-$REPO_ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64}"
ALLOWLIST="$REPO_ROOT/scripts/kernel-neon-allowlist.txt"

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'

if [ ! -f "$KERNEL_ELF" ]; then
    echo -e "${RED}ERROR:${NC} kernel ELF not found: $KERNEL_ELF" >&2
    echo "Build it first with the SOFT-FLOAT kernel target:" >&2
    echo "  cargo build --release --target aarch64-breenix-kernel.json \\" >&2
    echo "    -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \\" >&2
    echo "    -p kernel --bin kernel-aarch64" >&2
    exit 1
fi

# --- locate an aarch64-capable objdump -------------------------------------
# Prefer llvm-objdump (ships with the rust toolchain, disassembles any target).
find_objdump() {
    if command -v llvm-objdump >/dev/null 2>&1; then
        echo "llvm-objdump"; return 0
    fi
    local sysroot cand
    sysroot="$(rustc --print sysroot 2>/dev/null)"
    if [ -n "$sysroot" ]; then
        cand="$(ls "$sysroot"/lib/rustlib/*/bin/llvm-objdump 2>/dev/null | head -1)"
        if [ -n "$cand" ]; then echo "$cand"; return 0; fi
    fi
    # GNU binutils fallback (Linux CI hosts). macOS /usr/bin/objdump is llvm-based.
    if command -v objdump >/dev/null 2>&1; then
        echo "objdump"; return 0
    fi
    return 1
}

OBJDUMP="$(find_objdump)" || {
    echo -e "${RED}ERROR:${NC} no objdump found (need llvm-objdump or objdump on PATH)." >&2
    exit 1
}

echo "Guard: kernel FP/SIMD instruction check"
echo "  ELF:       $KERNEL_ELF"
echo "  objdump:   $OBJDUMP"
echo "  allowlist: $ALLOWLIST"

# --- disassemble and scan ---------------------------------------------------
DISASM="$("$OBJDUMP" -d "$KERNEL_ELF" 2>/dev/null)" || {
    echo -e "${RED}ERROR:${NC} objdump failed on $KERNEL_ELF" >&2
    exit 1
}

# One awk pass over the full disassembly:
#   * track the current section (only .text* is kernel code)
#   * track the owning symbol of each instruction
#   * flag any load/store whose first operand is an FP/SIMD (q/d/s/v) register
#   * suppress offenders whose owning symbol matches an allowlist regex
REPORT="$(printf '%s\n' "$DISASM" | awk -v allowfile="$ALLOWLIST" '
BEGIN {
    na = 0;
    while ((getline line < allowfile) > 0) {
        sub(/#.*/, "", line);
        gsub(/^[ \t]+|[ \t]+$/, "", line);
        if (line != "") allow[na++] = line;
    }
    insection = 0; sym = "?";
    total = 0; suppressed = 0; violations = 0; nprinted = 0;
    # FP/SIMD load or store: mnemonic starts ld/st, first operand is a
    # q/d/s/v register. GP registers are x/w/sp/xzr/wzr and never match
    # [qdsv][0-9], so this uniquely selects vector/FP loads and stores.
    fpre = "[ \t](ld|st)[a-z0-9]*[ \t]+[{]?[ \t]*[qdsv][0-9]";
}
/^Disassembly of section / {
    insection = ($0 ~ /section \.text/) ? 1 : 0;
    next;
}
/^[0-9a-fA-F]+ <.*>:/ {
    s = $0;
    sub(/^[0-9a-fA-F]+ </, "", s);
    sub(/>:.*$/, "", s);
    sym = s;
    next;
}
{
    if (!insection) next;
    if ($0 !~ /^[ \t]*[0-9a-fA-F]+:/) next;   # instruction lines only
    if ($0 !~ fpre) next;                      # FP/SIMD load/store only
    total++;
    allowed = 0;
    for (i = 0; i < na; i++) {
        if (sym ~ allow[i]) { allowed = 1; break; }
    }
    if (allowed) { suppressed++; next; }
    v = $0; sub(/^[ \t]+/, "", v);
    if (nprinted < 40) { printf("  %-58s  %s\n", sym, v); nprinted++; }
    violations++;
}
END {
    printf("__SUMMARY__ total=%d suppressed=%d violations=%d\n",
           total, suppressed, violations);
}
')"

SUMMARY="$(printf '%s\n' "$REPORT" | grep '^__SUMMARY__')"
SUPPRESSED="$(echo "$SUMMARY" | sed -n 's/.*suppressed=\([0-9]*\).*/\1/p')"
VIOLATIONS="$(echo "$SUMMARY" | sed -n 's/.*violations=\([0-9]*\).*/\1/p')"
SUPPRESSED="${SUPPRESSED:-0}"; VIOLATIONS="${VIOLATIONS:-0}"

OFFENDERS="$(printf '%s\n' "$REPORT" | grep -v '^__SUMMARY__' | grep -v '^[[:space:]]*$')"

if [ "$VIOLATIONS" -eq 0 ]; then
    echo -e "${GREEN}PASS:${NC} 0 FP/SIMD load/store instructions in kernel .text" \
            "(allowlisted & suppressed: $SUPPRESSED)."
    exit 0
fi

echo -e "${RED}FAIL:${NC} found $VIOLATIONS FP/SIMD load/store instruction(s) in kernel .text." >&2
echo -e "${YELLOW}This means the kernel was built with the NEON hardfloat target" >&2
echo -e "(aarch64-breenix.json) instead of the soft-float kernel target" >&2
echo -e "(aarch64-breenix-kernel.json) — re-arming issue #528.${NC}" >&2
echo "Offending instructions (symbol  ->  instruction), first 40:" >&2
printf '%s\n' "$OFFENDERS" >&2
exit 1
