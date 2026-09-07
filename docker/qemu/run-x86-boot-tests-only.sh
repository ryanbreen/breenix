#!/bin/bash
# Issue 927: exercise the retirement cohorts with the ordinary init loader.
# The full testing-loader gate remains run-x86-boot-tests.sh.
set -Eeuo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
source "$SCRIPT_DIR/lib/qemu-host-lock.sh"
source "$SCRIPT_DIR/lib/gate-structure-preflight.sh"
export BREENIX_GATE_TMP="${BREENIX_GATE_TMP:-/tmp}"
case "$BREENIX_GATE_TMP" in /*) ;; *) echo 'FAIL: absolute BREENIX_GATE_TMP required'; exit 1 ;; esac
mkdir -p "$BREENIX_GATE_TMP"
OUTPUT_DIR="$(mktemp -d "$BREENIX_GATE_TMP/breenix_x86_boot_tests_only.XXXXXX")"
report_failure() {
    local status=$?
    echo "x86 boot_tests-only gate: FAIL (exit $status); artifacts: $OUTPUT_DIR"
    tail -100 "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true
    exit "$status"
}
trap report_failure ERR
cd "$BREENIX_ROOT"
echo "REVISION=$(git rev-parse HEAD)"
git diff --stat
echo "Artifacts: $OUTPUT_DIR"
if ! gate_structure_preflight "$BREENIX_ROOT" "$BREENIX_GATE_TMP"; then
    echo 'FAIL: structure-suite preflight'; false
fi
cargo build --release --features boot_tests --bin qemu-uefi 2>&1 | tee "$OUTPUT_DIR/build.log"
if grep -Eq '^[[:space:]]*(warning|error)(\[|:)' "$OUTPUT_DIR/build.log"; then
    echo 'FAIL: compile diagnostics'; false
fi
BREENIX_PRINT_UEFI_IMAGE=1 cargo run --release --features boot_tests --bin qemu-uefi >"$OUTPUT_DIR/image-path.txt" 2>"$OUTPUT_DIR/image-build.log"
if grep -Eq '^[[:space:]]*(warning|error)(\[|:)' "$OUTPUT_DIR/image-build.log"; then
    echo 'FAIL: image compile diagnostics'; false
fi
UEFI_IMG="$(sed -n 's/^UEFI_IMAGE=//p' "$OUTPUT_DIR/image-path.txt")"
test -f "$UEFI_IMG"
cp target/ovmf/x64/code.fd "$OUTPUT_DIR/OVMF_CODE.fd"
cp target/ovmf/x64/vars.fd "$OUTPUT_DIR/OVMF_VARS.fd"
# These early kernel oracles precede disk init; no userspace result is claimed.
dd if=/dev/zero of="$OUTPUT_DIR/placeholder.img" bs=1048576 count=16 2>/dev/null
touch "$OUTPUT_DIR/serial_user.txt" "$OUTPUT_DIR/serial_kernel.txt"
qemu_host_lock_acquire qemu-system-x86_64
qemu-system-x86_64 \
    -pflash "$OUTPUT_DIR/OVMF_CODE.fd" -pflash "$OUTPUT_DIR/OVMF_VARS.fd" \
    -drive "if=none,id=hd,format=raw,readonly=on,file=$UEFI_IMG" \
    -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
    -drive "if=none,id=testdisk,format=raw,readonly=on,file=$OUTPUT_DIR/placeholder.img" \
    -device virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off \
    -machine pc,accel=tcg -cpu qemu64 -smp 1 -m 512 \
    -display none -no-reboot -no-shutdown \
    -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
    -serial "file:$OUTPUT_DIR/serial_user.txt" -serial "file:$OUTPUT_DIR/serial_kernel.txt" \
    >"$OUTPUT_DIR/qemu.log" 2>&1 &
QEMU_PID=$!
qemu_host_lock_track_pid "$QEMU_PID"
# Match the full x86 gate's bound: this profile also runs the 64-child cohort.
for ((elapsed=0; elapsed<900; elapsed++)); do
    if grep -aqE '\[TOMBSTONE_JOIN_ORACLE:x86:|KERNEL PANIC|DOUBLE FAULT|TRIPLE FAULT' "$OUTPUT_DIR"/serial_*.txt; then break; fi
    kill -0 "$QEMU_PID" 2>/dev/null || break
    sleep 1
done
kill "$QEMU_PID" 2>/dev/null || true
wait "$QEMU_PID" 2>/dev/null || true
qemu_host_lock_release
# Read the exact cohort literals from the full-profile gate so their numerical
# criteria cannot diverge between these two profiles. Missing pins fail closed.
python3 - "$OUTPUT_DIR" "$SCRIPT_DIR/run-x86-boot-tests.sh" <<'PY'
import pathlib, re, sys
out, gate = map(pathlib.Path, sys.argv[1:])
serial = '\n'.join(p.read_text(errors='replace') for p in out.glob('serial_*.txt'))
assert not re.search(r'KERNEL PANIC|DOUBLE FAULT|TRIPLE FAULT|\[TEST:[^\]\n]*:FAIL', serial), 'guest failure'
for name in ('PT_COHORT_LITERAL', 'PT_EXEC_COHORT_LITERAL', 'TOMBSTONE_JOIN_ORACLE_LITERAL'):
    pins = re.findall(r"^" + name + r"='([^'\n]+)'", gate.read_text(), re.M)
    assert len(pins) == 1, f'missing/ambiguous pin: {name}'
    assert serial.count(pins[0]) == 1, f'missing/duplicate/mismatched {name}'
    print(pins[0])
for name in ('x86_retire_cohort', 'x86_exec_cohort'):
    marker = f'[TEST:process:{name}:PASS]'
    assert serial.count(marker) == 1, f'missing/duplicate {marker}'
    print(marker)
PY
echo 'x86 boot_tests-only gate: PASS'
