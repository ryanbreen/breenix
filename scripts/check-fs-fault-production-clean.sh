#!/bin/bash
#
# ext2/VFS fault-injection production-cleanliness ratchet.
#
# The leg's non-negotiable is that a production build carries zero bytes of it.
# That is a claim about an ELF, not about a `#[cfg]`, so this script measures a
# real production-profile ELF rather than reading the source and trusting it.
#
#   LEG 1 (symbols): no symbol in the production kernel belongs to the leg.
#   LEG 2 (strings): the leg's marker prefix appears nowhere in the image.
#
# There is deliberately no byte-identity leg here, and the reason is structural
# rather than an economy: the core-proof harness needed one because its seams are
# macro invocations sitting at PRODUCTION call sites, where "expands to nothing"
# and "the optimiser removed it" are different claims. This leg has no production
# call sites at all -- the module and both of its two call sites (one per
# architecture main) are inside `#[cfg(feature = "fs_fault_inject")]` blocks, so
# there is no production code path for a byte comparison to be about. The source
# half of that claim is `scripts/check-fs-fault-seams.sh`, which is what would
# catch a call site escaping its cfg.
#
# Exit code: 0 = the production profile carries none of the leg; 1 = it does.
#
# Usage:
#   scripts/check-fs-fault-production-clean.sh            # legs 1+2 on a fresh production build
#   scripts/check-fs-fault-production-clean.sh --elf PATH # legs 1+2 on PATH
#   scripts/check-fs-fault-production-clean.sh --prove    # anti-vacuity: the same
#                                                         # scan against an
#                                                         # fs_fault_inject build
#                                                         # must go red on both legs

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

MODE="scan"
ELF=""
while [ $# -gt 0 ]; do
    case "$1" in
        --elf) ELF="$2"; shift 2 ;;
        --prove) MODE="prove"; shift ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

BUILD_ARGS=(build --release --target aarch64-breenix-kernel.json
            -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem
            -p kernel --bin kernel-aarch64)

find_llvm_tool() {
    local name="$1" sysroot cand
    if command -v "$name" >/dev/null 2>&1; then echo "$name"; return 0; fi
    sysroot="$(rustc --print sysroot 2>/dev/null)"
    if [ -n "$sysroot" ]; then
        cand="$(ls "$sysroot"/lib/rustlib/*/bin/"$name" 2>/dev/null | head -1)"
        if [ -n "$cand" ]; then echo "$cand"; return 0; fi
    fi
    return 1
}

NM="$(find_llvm_tool llvm-nm)" || { echo "ERROR: no llvm-nm available" >&2; exit 1; }

# `fault_inject` covers the module's mangled symbols; `[FSFAULT:` is the marker
# prefix every emitter in the leg carries and the one the gate keys on.
SYMBOL_NEEDLE='fault_inject'
MARKER_NEEDLE='[FSFAULT:'

build_kernel() {
    local target_dir="$1"; shift
    ( cd "$REPO_ROOT" && CARGO_TARGET_DIR="$target_dir" cargo "${BUILD_ARGS[@]}" "$@" ) >/dev/null 2>&1
}

leg1_symbols() {
    "$NM" --defined-only "$1" 2>/dev/null | grep -ci "$SYMBOL_NEEDLE" || true
}

leg2_strings() {
    grep -c -aF "$MARKER_NEEDLE" "$1" 2>/dev/null || true
}

if [ "$MODE" = "prove" ]; then
    DIRTY_DIR="$REPO_ROOT/target/fs-fault-prove-dirty"
    echo "Building an fs_fault_inject kernel to prove the scan can go red..."
    if ! build_kernel "$DIRTY_DIR" --features boot_tests,fs_fault_inject; then
        echo "FS-FAULT PRODUCTION RATCHET ANTI-VACUITY: FAILED (fs_fault_inject build failed)"
        exit 1
    fi
    DIRTY_ELF="$DIRTY_DIR/aarch64-breenix-kernel/release/kernel-aarch64"
    S="$(leg1_symbols "$DIRTY_ELF")"
    M="$(leg2_strings "$DIRTY_ELF")"
    echo "  fs_fault_inject build: symbols=$S markers=$M"
    if [ "$S" -eq 0 ] || [ "$M" -eq 0 ]; then
        echo "FS-FAULT PRODUCTION RATCHET ANTI-VACUITY: FAILED"
        echo "A build that DOES carry the leg was not detected (symbols=$S markers=$M)."
        exit 1
    fi
    echo "FS-FAULT PRODUCTION RATCHET ANTI-VACUITY: PASSED (an fs_fault_inject build reddens both legs)"
    exit 0
fi

CLEAN_DIR="$REPO_ROOT/target/fs-fault-production-clean"
if [ -z "$ELF" ]; then
    echo "Building the production-profile kernel (no features)..."
    if ! build_kernel "$CLEAN_DIR"; then
        echo "FS-FAULT PRODUCTION RATCHET: FAILED (production build failed)"
        exit 1
    fi
    ELF="$CLEAN_DIR/aarch64-breenix-kernel/release/kernel-aarch64"
fi

if [ ! -f "$ELF" ]; then
    echo "FS-FAULT PRODUCTION RATCHET: FAILED (no ELF at $ELF)"
    exit 1
fi

SYMBOLS="$(leg1_symbols "$ELF")"
MARKERS="$(leg2_strings "$ELF")"
echo "LEG 1 (symbols matching '$SYMBOL_NEEDLE'): $SYMBOLS"
echo "LEG 2 (occurrences of '$MARKER_NEEDLE'):   $MARKERS"
if [ "$SYMBOLS" -ne 0 ] || [ "$MARKERS" -ne 0 ]; then
    echo "FS-FAULT PRODUCTION RATCHET: FAILED"
    "$NM" --defined-only "$ELF" 2>/dev/null | grep -i "$SYMBOL_NEEDLE" | head -20
    exit 1
fi

echo "FS-FAULT PRODUCTION RATCHET: PASSED (legs 1+2)"
exit 0
