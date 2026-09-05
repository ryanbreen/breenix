#!/bin/bash
#
# check-x86-dispatch-no-alloc.sh - binary-level guard for issue #791.
#
# interrupts::context_switch::setup_kernel_thread_return runs inside the timer
# interrupt with IF=0 on each kernel-thread dispatch. An allocation there takes
# the heap allocator's lock from interrupt context, and ordinary thread context
# holds that lock with interrupts ENABLED -- the exact shape that wedged the x86
# boot-tests gate inside x86_retire_cohort (#791; RCA at
# docs/planning/green-program/sockets/787-REGRESSION-RCA-2026-09-04.md).
#
# tests/dispatch_path_lock_free_structure.rs also checks this, but it is a
# source-level DENYLIST: it can only see spellings it lists, and it cannot see
# an allocation reached through a callee. This script is the authority. It
# disassembles the SHIPPED kernel and fails if the function's own code
# references any Rust allocator symbol.
#
# Anti-vacuity: the guard FAILS if it cannot find the function symbol at all.
# A renamed, inlined-away or missing symbol must not read as "no allocation".
#
# Exit code:
#   0 - clean (symbol found, no allocator reference in its body)
#   1 - guard tripped, or the symbol was not found, or a tooling/usage error
#
# Usage:
#   ./scripts/check-x86-dispatch-no-alloc.sh [path/to/kernel-elf]
# Default ELF: the newest
#   target/x86_64-unknown-none/release/deps/artifact/*/bin/kernel-*
# which is the binary the bootloader embeds into breenix-uefi.img.
# claim-lint:ok: the "none" above is the x86_64-unknown-none target triple, not a
# claim; the 1 of 1 ELF this guard reads is that artifact path.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

FN_NAME="setup_kernel_thread_return"

# Rust allocator entry points, matched as substrings so both the legacy `_ZN`
# and the v0 `_RNv` manglings are covered (v0 spells __rust_alloc as
# `_RNvCs..._7___rustc12___rust_alloc`, which still contains `__rust_alloc`).
ALLOC_RE='__rust_alloc|__rust_realloc|__rg_alloc|__rdl_alloc|__rust_no_alloc|5alloc5alloc'

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'

KERNEL_ELF="${1:-}"
if [ -z "$KERNEL_ELF" ]; then
    KERNEL_ELF="$(ls -t "$REPO_ROOT"/target/x86_64-unknown-none/release/deps/artifact/*/bin/kernel-* 2>/dev/null | grep -v '\.d$' | head -1)"
fi

if [ -z "$KERNEL_ELF" ] || [ ! -f "$KERNEL_ELF" ]; then
    echo "ERROR: x86 kernel ELF not found (looked for the newest" >&2
    echo "  target/x86_64-unknown-none/release/deps/artifact/*/bin/kernel-*)." >&2
    echo "Build it first with the boot_tests profile." >&2
    exit 1
fi

# --- locate an objdump ------------------------------------------------------
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


if command -v sha256sum >/dev/null 2>&1; then
    ELF_SHA="$(sha256sum "$KERNEL_ELF" | cut -d' ' -f1)"
else
    ELF_SHA="$(shasum -a 256 "$KERNEL_ELF" | cut -d' ' -f1)"
fi

echo "Guard: x86 kernel-thread dispatch allocation check (#791)"
echo "  ELF:      $KERNEL_ELF"
echo "  sha256:   $ELF_SHA"
echo "  objdump:  $OBJDUMP"
echo "  function: $FN_NAME (its own body and its closures)"

DISASM="$("$OBJDUMP" -d "$KERNEL_ELF" 2>/dev/null)"
if [ -z "$DISASM" ]; then
    echo "ERROR: objdump produced no disassembly for $KERNEL_ELF" >&2
    exit 1
fi

# One awk pass over the disassembly:
#   * track the current symbol from each `<addr> <SYMBOL>:` header
#   * a symbol is IN SCOPE when its (mangled) name contains the function name,
#     which also picks up its `{{closure}}` symbols
#   * inside scope, flag any instruction whose operand text names a Rust
#     allocator entry point (call, jmp tail-call, or an address load alike)
REPORT="$(printf '%s\n' "$DISASM" | awk -v fn="$FN_NAME" -v allocre="$ALLOC_RE" '
/^[0-9a-fA-F]+ <.*>:/ {
    s = $0;
    sub(/^[0-9a-fA-F]+ </, "", s);
    sub(/>:.*$/, "", s);
    sym = s;
    inscope = (index(sym, fn) > 0) ? 1 : 0;
    if (inscope) { syms[sym] = 1; nsyms++; }
    next;
}
{
    if (!inscope) next;
    if ($0 !~ /^[ \t]*[0-9a-fA-F]+:/) next;
    if ($0 !~ allocre) next;
    v = $0; sub(/^[ \t]+/, "", v);
    printf("  %-70s  %s\n", sym, v);
    violations++;
}
END {
    printf("__SUMMARY__ symbols=%d violations=%d\n", nsyms, violations);
}
')"

SUMMARY="$(printf '%s\n' "$REPORT" | grep '^__SUMMARY__')"
SYMBOLS="$(echo "$SUMMARY" | sed -n 's/.*symbols=\([0-9]*\).*/\1/p')"
VIOLATIONS="$(echo "$SUMMARY" | sed -n 's/.*violations=\([0-9]*\).*/\1/p')"
SYMBOLS="${SYMBOLS:-0}"; VIOLATIONS="${VIOLATIONS:-0}"
OFFENDERS="$(printf '%s\n' "$REPORT" | grep -v '^__SUMMARY__' | grep -v '^[[:space:]]*$')"

echo "  symbols in scope: $SYMBOLS"

# Anti-vacuity. A guard that finds no symbol has checked no code, and silently
# passing there is exactly how this class of ratchet goes vacuous.
if [ "$SYMBOLS" -eq 0 ]; then
    echo -e "${RED}FAIL:${NC} no symbol whose name contains '$FN_NAME' in $KERNEL_ELF." >&2
    echo -e "${YELLOW}The guard checked no code. Either the function was renamed," >&2
    echo -e "inlined away, or the wrong ELF was handed to this script. Fix the" >&2
    echo -e "guard rather than deleting it.${NC}" >&2
    exit 1
fi

if [ "$VIOLATIONS" -eq 0 ]; then
    echo -e "${GREEN}PASS:${NC} 0 Rust allocator references in $SYMBOLS in-scope symbol(s)."
    exit 0
fi

echo -e "${RED}FAIL:${NC} found $VIOLATIONS Rust allocator reference(s) in $FN_NAME." >&2
echo -e "${YELLOW}This function runs with IF=0 on every kernel-thread dispatch." >&2
echo -e "An allocation here takes the heap allocator's lock from interrupt" >&2
echo -e "context, which ordinary thread context holds with interrupts ENABLED --" >&2
echo -e "the shape that wedged the x86 boot-tests gate as issue #791.${NC}" >&2
printf '%s\n' "$OFFENDERS" >&2
exit 1
