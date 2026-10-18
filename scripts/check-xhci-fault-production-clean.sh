#!/bin/bash
#
# xHCI recovery-exercise production-cleanliness ratchet (#1931).
#
# The exercise is built only into fault-test kernels (`xhci_fault_inject`), so a
# production metal kernel has no code that reads its request file and a file on
# a root disk cannot start it. That is a claim about what the machine is given,
# so this script builds the x86 kernel the way the metal build does
# (`cargo build --release --bin qemu-uefi`, no features) and measures it:
#
#   LEG 1 (symbols): no symbol in the kernel ELF belongs to the exercise's module.
#   LEG 2 (strings): the delivered UEFI image holds zero occurrences of
#                    `xhci-fault`, the prefix of its log lines and the name of
#                    its request file.
#   claim-lint:ok: #1931 these define the pass condition leg1_symbols/leg2_strings measure.
#
# Exit code: 0 = both legs found zero matches; 1 = a leg found one, or a build failed.
# claim-lint:ok: #1931 the exit contract of this script, not an observation.
#
# Usage:
#   scripts/check-xhci-fault-production-clean.sh          # legs 1+2 on a production build
#   scripts/check-xhci-fault-production-clean.sh --prove  # anti-vacuity: an
#                                                         # xhci_fault_inject build
#                                                         # must redden both legs
#
# The builds use the repository's own target directory: the bootloader's build
# script runs a nested cargo that waits forever on an outer CARGO_TARGET_DIR's lock.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

MODE="scan"
case "${1:-}" in
    "") ;;
    --prove) MODE="prove" ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
esac

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

# `4xhci5fault` is the module path in both Rust manglings; the two entry points
# are named too, in case one is emitted under a generic or inlined parent.
SYMBOL_NEEDLE='4xhci5fault|fault_exercise|recovery_exercise'
MARKER_NEEDLE='xhci-fault'

# Build, then print "<kernel ELF> <UEFI image>" from cargo's own artifact report.
build() {
    local messages
    messages="$(mktemp)"
    if ! ( cd "$REPO_ROOT" && cargo build --release --bin qemu-uefi --message-format=json-render-diagnostics "$@" ) > "$messages"; then
        rm -f "$messages"
        return 1
    fi
    python3 - "$messages" <<'EOF'
import json, sys
elf = image = None
for line in open(sys.argv[1]):
    try:
        message = json.loads(line)
    except ValueError:
        continue
    if message.get('reason') == 'compiler-artifact' and message.get('target', {}).get('name') == 'kernel' \
            and message.get('executable'):
        elf = message['executable']
    if message.get('reason') == 'build-script-executed' and '#breenix@' in message.get('package_id', '') \
            and message.get('out_dir'):
        image = message['out_dir'] + '/breenix-uefi.img'
if not elf or not image:
    sys.exit('cargo did not report the kernel ELF and the UEFI image')
print(elf, image)
EOF
    local status=$?
    rm -f "$messages"
    return $status
}

leg1_symbols() {
    "$NM" --defined-only "$1" 2>/dev/null | grep -cE "$SYMBOL_NEEDLE" || true
}

leg2_strings() {
    grep -c -aF "$MARKER_NEEDLE" "$1" 2>/dev/null || true
}

if [ "$MODE" = "prove" ]; then
    echo "Building an xhci_fault_inject kernel to prove the scan can go red..."
    if ! OUTPUT="$(build --features xhci_fault_inject)"; then
        echo "XHCI-FAULT PRODUCTION RATCHET ANTI-VACUITY: FAILED (xhci_fault_inject build failed)"
        exit 1
    fi
    read -r ELF IMAGE <<< "$OUTPUT"
    S="$(leg1_symbols "$ELF")"
    M="$(leg2_strings "$IMAGE")"
    echo "  xhci_fault_inject build: symbols=$S image-markers=$M"
    if [ "$S" -eq 0 ] || [ "$M" -eq 0 ]; then
        echo "XHCI-FAULT PRODUCTION RATCHET ANTI-VACUITY: FAILED"
        echo "A build that does carry the exercise was not detected (symbols=$S image-markers=$M)."
        exit 1
    fi
    echo "XHCI-FAULT PRODUCTION RATCHET ANTI-VACUITY: PASSED (an xhci_fault_inject build reddens both legs)"
    exit 0
fi

echo "Building the production metal kernel (no features)..."
if ! OUTPUT="$(build)"; then
    echo "XHCI-FAULT PRODUCTION RATCHET: FAILED (production build failed)"
    exit 1
fi
read -r ELF IMAGE <<< "$OUTPUT"
SYMBOLS="$(leg1_symbols "$ELF")"
MARKERS="$(leg2_strings "$IMAGE")"
echo "LEG 1 (kernel symbols matching '$SYMBOL_NEEDLE'): $SYMBOLS"
echo "LEG 2 (occurrences of '$MARKER_NEEDLE' in the UEFI image): $MARKERS"
if [ "$SYMBOLS" -ne 0 ] || [ "$MARKERS" -ne 0 ]; then
    echo "XHCI-FAULT PRODUCTION RATCHET: FAILED"
    "$NM" --defined-only "$ELF" 2>/dev/null | grep -E "$SYMBOL_NEEDLE" | head -20
    exit 1
fi
echo "XHCI-FAULT PRODUCTION RATCHET: PASSED (legs 1+2)"
exit 0
