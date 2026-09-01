#!/bin/bash
#
# Core-proof production-cleanliness ratchet — the BINARY half of the pair
# (`scripts/check-coreproof-seams.sh` is the source half).
#
# The claim under test is not "the harness is feature-gated" — that is visible in
# the source and proves nothing about what ships. The claim is that
# `proof_point!` and `proof_cover!` compile to LITERALLY NOTHING in a production build, and this
# script measures it three ways against a real production-profile ELF:
#
#   LEG 1 (symbols): no symbol in the production kernel belongs to the harness.
#   LEG 2 (strings): the harness's marker literal appears nowhere in the image.
#   LEG 3 (bytes):   the production `.text` built with the seams present is
#                    byte-identical to the production `.text` built with every
#                    seam invocation neutralised. This is the only leg that can
#                    distinguish "expands to nothing" from "expands to something
#                    the optimiser happened to remove this time", and it is the
#                    reason the seam macro is defined in two polarities in lib.rs
#                    rather than as an empty function.
#
# LEG 3 has to control for two things that would otherwise make it a test of the
# harness rather than of the seams. BOTH were found by running it and watching it
# go red on a tree whose seams cost nothing:
#
#   * `.text` is compared, not the whole file, because `build.rs` stamps a fresh
#     `BREENIX_BUILD_ID` timestamp into `.rodata` on every build. No two
#     production ELFs are ever whole-file identical, including two builds of the
#     same unmodified tree. The id is fixed-width, so it moves no code.
#   * The seams are BLANKED IN PLACE by scripts/coreproof-blank-seams.py, which
#     preserves line and column count, and both builds run from the same
#     directory. Deleting the lines instead shifts every following line number,
#     which moves the `core::panic::Location` records those lines feed - and
#     `.text` addresses those records. Comparing the wrong two binaries would
#     have been a false red here and, with the polarity reversed, a false green.
#
# The module and the macro are deliberately left in place for LEG 3. They are
# already cfg-ed out of a production build, and the claim under test is about the
# seam INVOCATIONS at production call sites.
#
# Exit code: 0 = the production profile carries none of the harness; 1 = it does.
#
# Usage:
#   scripts/check-coreproof-production-clean.sh                # legs 1+2 on an existing ELF
#   scripts/check-coreproof-production-clean.sh --elf PATH     # legs 1+2 on PATH
#   scripts/check-coreproof-production-clean.sh --bytes        # legs 1+2+3 (two builds, slow)
#   scripts/check-coreproof-production-clean.sh --prove        # anti-vacuity: the same scan
#                                                              # run against a coreproof build
#                                                              # must go red on legs 1 and 2

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

MODE="scan"
ELF=""
while [ $# -gt 0 ]; do
    case "$1" in
        --elf) ELF="$2"; shift 2 ;;
        --bytes) MODE="bytes"; shift ;;
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
OBJCOPY="$(find_llvm_tool llvm-objcopy)" || { echo "ERROR: no llvm-objcopy available" >&2; exit 1; }

# The harness's own spellings. `coreproof` covers the feature name, the module's
# mangled symbols and the mutation features; `[COREPROOF:` is the marker prefix
# the gate scripts key on and is the string a surviving emitter would carry.
SYMBOL_NEEDLE='coreproof'
MARKER_NEEDLE='[COREPROOF:'

build_kernel() {
    local target_dir="$1"; shift
    ( cd "$REPO_ROOT" && CARGO_TARGET_DIR="$target_dir" cargo "${BUILD_ARGS[@]}" "$@" ) >/dev/null 2>&1
}

leg1_symbols() {
    local elf="$1" hits
    hits="$("$NM" --defined-only "$elf" 2>/dev/null | grep -ci "$SYMBOL_NEEDLE" || true)"
    echo "$hits"
}

leg2_strings() {
    local elf="$1" hits
    hits="$(grep -c -aF "$MARKER_NEEDLE" "$elf" 2>/dev/null || true)"
    echo "$hits"
}

text_hash() {
    local elf="$1" out
    out="$(mktemp)"
    "$OBJCOPY" -O binary --only-section=.text "$elf" "$out" 2>/dev/null
    shasum -a 256 "$out" | awk '{print $1}'
    rm -f "$out"
}

# ---------------------------------------------------------------------------
# Anti-vacuity: the same two scans, pointed at a build that DOES carry the
# harness, must go red. A cleanliness check that cannot detect the thing it
# forbids is worth nothing.
# ---------------------------------------------------------------------------
if [ "$MODE" = "prove" ]; then
    DIRTY_DIR="$REPO_ROOT/target/coreproof-prove-dirty"
    echo "Building a coreproof-profile kernel to prove the scan can go red..."
    if ! build_kernel "$DIRTY_DIR" --features boot_tests,coreproof,coreproof_component_a; then
        echo "CORE-PROOF PRODUCTION RATCHET ANTI-VACUITY: FAILED (coreproof build failed)"
        exit 1
    fi
    DIRTY_ELF="$DIRTY_DIR/aarch64-breenix-kernel/release/kernel-aarch64"
    S="$(leg1_symbols "$DIRTY_ELF")"
    M="$(leg2_strings "$DIRTY_ELF")"
    echo "  coreproof build: symbols=$S markers=$M"
    if [ "$S" -eq 0 ] || [ "$M" -eq 0 ]; then
        echo "CORE-PROOF PRODUCTION RATCHET ANTI-VACUITY: FAILED"
        echo "A build that DOES carry the harness was not detected (symbols=$S markers=$M)."
        exit 1
    fi
    echo "CORE-PROOF PRODUCTION RATCHET ANTI-VACUITY: PASSED (a coreproof build reddens both legs)"
    exit 0
fi

# ---------------------------------------------------------------------------
# Legs 1 and 2 against the production ELF.
# ---------------------------------------------------------------------------
CLEAN_DIR="$REPO_ROOT/target/coreproof-production-clean"
if [ -z "$ELF" ]; then
    echo "Building the production-profile kernel (no features)..."
    if ! build_kernel "$CLEAN_DIR"; then
        echo "CORE-PROOF PRODUCTION RATCHET: FAILED (production build failed)"
        exit 1
    fi
    ELF="$CLEAN_DIR/aarch64-breenix-kernel/release/kernel-aarch64"
fi

if [ ! -f "$ELF" ]; then
    echo "CORE-PROOF PRODUCTION RATCHET: FAILED (no ELF at $ELF)"
    exit 1
fi

SYMBOLS="$(leg1_symbols "$ELF")"
MARKERS="$(leg2_strings "$ELF")"
echo "LEG 1 (symbols matching '$SYMBOL_NEEDLE'): $SYMBOLS"
echo "LEG 2 (occurrences of '$MARKER_NEEDLE'):   $MARKERS"
if [ "$SYMBOLS" -ne 0 ] || [ "$MARKERS" -ne 0 ]; then
    echo "CORE-PROOF PRODUCTION RATCHET: FAILED"
    "$NM" --defined-only "$ELF" 2>/dev/null | grep -i "$SYMBOL_NEEDLE" | head -20
    exit 1
fi

if [ "$MODE" != "bytes" ]; then
    echo "CORE-PROOF PRODUCTION RATCHET: PASSED (legs 1+2; run --bytes for leg 3)"
    exit 0
fi

# ---------------------------------------------------------------------------
# LEG 3: the byte comparison.
# ---------------------------------------------------------------------------
WORK_TREE="$REPO_ROOT/target/coreproof-bytes-tree"
LEG3_TARGET="$REPO_ROOT/target/coreproof-bytes-target"
WORK_ELF="$LEG3_TARGET/aarch64-breenix-kernel/release/kernel-aarch64"
rm -rf "$WORK_TREE"
mkdir -p "$WORK_TREE"
echo "LEG 3: staging a build tree..."
( cd "$REPO_ROOT" && tar --exclude='./target' --exclude='./.git' --exclude='./node_modules' -cf - . ) \
    | ( cd "$WORK_TREE" && tar -xf - )

build_work_tree() {
    ( cd "$WORK_TREE" && CARGO_TARGET_DIR="$LEG3_TARGET" cargo "${BUILD_ARGS[@]}" ) >/dev/null 2>&1
}

echo "LEG 3: building with the seams present..."
if ! build_work_tree; then
    echo "CORE-PROOF PRODUCTION RATCHET: FAILED (staged build with seams failed)"
    exit 1
fi
WITH_SEAMS="$(text_hash "$WORK_ELF")"

echo "LEG 3: neutralising the seams in place..."
BLANKED="$("$SCRIPT_DIR/coreproof-blank-seams.py" "$WORK_TREE")"
echo "LEG 3: blanked ${BLANKED:-0} seam invocation(s)"
if [ "${BLANKED:-0}" -lt 1 ]; then
    echo "CORE-PROOF PRODUCTION RATCHET: FAILED"
    echo "No seam invocation was found to blank, so the byte comparison would be vacuous."
    exit 1
fi

echo "LEG 3: rebuilding with the seams neutralised..."
if ! build_work_tree; then
    echo "CORE-PROOF PRODUCTION RATCHET: FAILED (staged build without seams failed)"
    ( cd "$WORK_TREE" && CARGO_TARGET_DIR="$LEG3_TARGET" cargo "${BUILD_ARGS[@]}" ) 2>&1 | tail -30
    exit 1
fi
WITHOUT_SEAMS="$(text_hash "$WORK_ELF")"

echo "LEG 3 .text sha256 with seams:    $WITH_SEAMS"
echo "LEG 3 .text sha256 seams blanked: $WITHOUT_SEAMS"
if [ "$WITH_SEAMS" != "$WITHOUT_SEAMS" ]; then
    echo "CORE-PROOF PRODUCTION RATCHET: FAILED"
    echo "The seams changed the production .text. a core-proof macro is not expanding to nothing."
    exit 1
fi

echo "CORE-PROOF PRODUCTION RATCHET: PASSED (legs 1+2+3; production .text byte-identical over $BLANKED seam(s))"
exit 0
