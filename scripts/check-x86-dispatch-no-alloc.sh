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
# source-level DENYLIST: it can only see spellings it lists. This script is the
# authority. It reads the SHIPPED kernel and fails if the function's own
# symbols -- its body and its {{closure}} symbols -- call anything that
# allocates.
#
# WHY IT RESOLVES INDIRECT CALLS. This kernel is a static PIE, and rustc emits
# most cross-crate calls as `movq <slot>(%rip), %rax; callq *%rax` through a GOT
# slot fixed up by an R_X86_64_RELATIVE relocation. The callee's name appears
# in no form at the call site. A guard that only reads instruction text therefore
# sees an allocating call as an anonymous indirect jump and passes. The callee is
# named in the relocation instead. Measured:
# a `thread.name.clone()` reintroduced into this function reaches
# `<alloc::string::String as Clone>::clone` through exactly such a slot. So the
# scan resolves each slot through its relocation to a .text symbol before
# judging it.
#
# DEPTH. This is a depth-1 check: the in-scope symbols' own call targets, direct
# and GOT-resolved. It is NOT a transitive walk. A transitive walk from this
# function reaches log::error!'s formatting machinery on the else arm and would
# redden on a clean tree, so depth-1 with an allocating-callee target set is the
# rule that is green on the fixed tree and red on the allocating one. What it
# cannot see: an allocation two frames down inside a callee that is itself not
# an alloc-crate symbol.
#
# Anti-vacuity: the guard FAILS if it cannot find the function symbol at all.
# A renamed, inlined-away or missing symbol must not read as "no allocation".
#
# Exit code:
#   0 - clean (symbols found, no allocating call target)
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

# Call targets that allocate. Two families:
#   * the allocator entry points themselves, as substrings so both the legacy
#     `_ZN` and the v0 `_RNv` manglings match (v0 spells __rust_alloc as
#     `_RNvCs..._7___rustc12___rust_alloc`, which contains `__rust_alloc`);
#   * the alloc-crate types whose methods allocate, which is what a call site in
#     this function actually reaches -- String::clone, Vec growth, Box::new and
#     the drop glue for them.
ALLOC_RE='__rust_alloc|__rust_realloc|__rg_alloc|__rdl_alloc|alloc\.\.alloc|alloc\.\.string|alloc\.\.vec|alloc\.\.raw_vec|alloc\.\.boxed|alloc\.\.collections|alloc\.\.ffi|5alloc5alloc|5alloc6string|5alloc3vec|5alloc5boxed'

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

# A readelf is REQUIRED, not optional. Without the relocation table the indirect
# call targets cannot be resolved, and an unresolved indirect call is exactly
# the case this guard exists to catch -- so a missing readelf is an error, not
# a quiet downgrade to a weaker check.
# claim-lint:ok: the resolution is measured, 2 of 2 legs: green on the fixed
# kernel and red on the shipped b257e69e kernel, recorded in the RCA round-2
# section of docs/planning/green-program/sockets/787-REGRESSION-RCA-2026-09-04.md.
find_readelf() {
    if command -v llvm-readelf >/dev/null 2>&1; then echo "llvm-readelf"; return 0; fi
    if command -v readelf >/dev/null 2>&1; then echo "readelf"; return 0; fi
    return 1
}

READELF="$(find_readelf)" || {
    echo "ERROR: no readelf found (need llvm-readelf or readelf on PATH)." >&2
    echo "This guard resolves GOT-indirect call targets through the relocation" >&2
    echo "table; without it the check would be weaker than it claims to be." >&2
    exit 1
}

if command -v sha256sum >/dev/null 2>&1; then
    ELF_SHA="$(sha256sum "$KERNEL_ELF" | cut -d" " -f1)"
else
    ELF_SHA="$(shasum -a 256 "$KERNEL_ELF" | cut -d" " -f1)"
fi

echo "Guard: x86 kernel-thread dispatch allocation check (#791)"
echo "  ELF:      $KERNEL_ELF"
echo "  sha256:   $ELF_SHA"
echo "  objdump:  $OBJDUMP"
echo "  readelf:  $READELF"
echo "  function: $FN_NAME (its own body and its closures)"

# --- three extractions, each keyed on hex with leading zeros stripped ---------
# (1) .text function symbols:            <addr> <mangled name>
# (2) R_X86_64_RELATIVE relocations:     <got slot> <target addr>
# (3) the disassembly itself.
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

"$OBJDUMP" -t "$KERNEL_ELF" 2>/dev/null \
  | awk '/ F +\.text/ { a=$1; sub(/^0+/,"",a); print a, $NF }' > "$WORK/syms"

"$READELF" -r "$KERNEL_ELF" 2>/dev/null \
  | awk '$3 ~ /R_X86_64_RELATIVE/ { s=$1; sub(/^0+/,"",s); t=$4; sub(/^0+/,"",t); print s, t }' > "$WORK/relocs"

"$OBJDUMP" -d "$KERNEL_ELF" 2>/dev/null > "$WORK/dis"

if [ ! -s "$WORK/dis" ]; then
    echo "ERROR: objdump produced no disassembly for $KERNEL_ELF" >&2
    exit 1
fi
if [ ! -s "$WORK/syms" ]; then
    echo "ERROR: no .text function symbols in $KERNEL_ELF" >&2
    exit 1
fi

# One awk pass over the disassembly. Inside a symbol whose name contains the
# function name (which also selects its {{closure}} symbols) collect the call
# targets two ways:
#   direct   -- `callq 0x... <SYMBOL>` names the callee in the instruction text
#   indirect -- `movq 0x...(%rip), %reg` whose `# 0x<slot>` comment names a GOT
#               slot; the relocation gives the target address and the symbol
#               table gives the name. Slots that do not resolve to a .text
#               function are data, not calls, and are skipped.
REPORT="$(awk -v fn="$FN_NAME" -v allocre="$ALLOC_RE" \
              -v symsf="$WORK/syms" -v relocf="$WORK/relocs" '
BEGIN {
    while ((getline l < symsf) > 0)  { split(l, a, " "); sym[a[1]] = a[2]; }
    while ((getline l < relocf) > 0) { split(l, a, " "); rel[a[1]] = a[2]; }
    ins = 0; nsyms = 0; ntargets = 0; violations = 0;
}
/^[0-9a-fA-F]+ <.*>:/ {
    s = $0; sub(/^[0-9a-fA-F]+ </, "", s); sub(/>:.*$/, "", s);
    cur = s; ins = (index(cur, fn) > 0);
    if (ins) nsyms++;
    next;
}
{
    if (!ins) next;
    target = "";
    if ($0 ~ /(callq|call|jmp|jmpq)[ \t]/ && match($0, /<[^>+]+(\+0x[0-9a-f]+)?>/)) {
        t = substr($0, RSTART + 1, RLENGTH - 2);
        sub(/\+0x[0-9a-f]+$/, "", t);
        if (index(t, fn) == 0) target = t;
    } else if ($0 ~ /movq[ \t]+0x[0-9a-f]+\(%rip\)/ && match($0, /#[ \t]*0x[0-9a-f]+[ \t]*$/)) {
        slot = substr($0, RSTART, RLENGTH);
        sub(/^#[ \t]*0x/, "", slot); sub(/[ \t]*$/, "", slot); sub(/^0+/, "", slot);
        if (slot in rel) { tgt = rel[slot]; if (tgt in sym) target = sym[tgt]; }
    }
    if (target == "") next;
    ntargets++;
    if (target ~ allocre) { printf("  %-20s calls %s\n", cur, target); violations++; }
}
END { printf("__SUMMARY__ symbols=%d targets=%d violations=%d\n", nsyms, ntargets, violations); }
' "$WORK/dis")"

SUMMARY="$(printf '%s\n' "$REPORT" | grep '^__SUMMARY__')"
SYMBOLS="$(echo "$SUMMARY" | sed -n 's/.*symbols=\([0-9]*\).*/\1/p')"
TARGETS="$(echo "$SUMMARY" | sed -n 's/.*targets=\([0-9]*\).*/\1/p')"
VIOLATIONS="$(echo "$SUMMARY" | sed -n 's/.*violations=\([0-9]*\).*/\1/p')"
SYMBOLS="${SYMBOLS:-0}"; TARGETS="${TARGETS:-0}"; VIOLATIONS="${VIOLATIONS:-0}"
OFFENDERS="$(printf '%s\n' "$REPORT" | grep -v '^__SUMMARY__' | grep -v '^[[:space:]]*$')"

echo "  symbols in scope: $SYMBOLS"
echo "  call targets resolved: $TARGETS"

# Anti-vacuity, two arms. A guard that finds no symbol has checked no code, and
# a guard that resolves no call target has checked no edge; silently passing in
# either case is how this class of ratchet goes vacuous.
if [ "$SYMBOLS" -eq 0 ]; then
    echo -e "${RED}FAIL:${NC} no symbol whose name contains '$FN_NAME' in $KERNEL_ELF." >&2
    echo -e "${YELLOW}The guard checked no code. Either the function was renamed," >&2
    echo -e "inlined away, or the wrong ELF was handed to this script. Fix the" >&2
    echo -e "guard rather than deleting it.${NC}" >&2
    exit 1
fi
if [ "$TARGETS" -eq 0 ]; then
    echo -e "${RED}FAIL:${NC} resolved 0 call targets inside $FN_NAME." >&2
    echo -e "${YELLOW}The symbol was found but no call edge came out of it, which" >&2
    echo -e "means the disassembly or relocation parsing has stopped matching this" >&2
    echo -e "toolchain's output. The guard is not checking what it claims.${NC}" >&2
    exit 1
fi

if [ "$VIOLATIONS" -eq 0 ]; then
    echo -e "${GREEN}PASS:${NC} 0 allocating call targets in $SYMBOLS in-scope symbol(s), $TARGETS edge(s) checked."
    exit 0
fi

echo -e "${RED}FAIL:${NC} found $VIOLATIONS allocating call target(s) in $FN_NAME." >&2
echo -e "${YELLOW}This function runs with IF=0 on each kernel-thread dispatch." >&2
echo -e "An allocation here takes the heap allocator's lock from interrupt" >&2
echo -e "context, which ordinary thread context holds with interrupts ENABLED --" >&2
echo -e "the shape that wedged the x86 boot-tests gate as issue #791.${NC}" >&2
printf '%s\n' "$OFFENDERS" >&2
exit 1
