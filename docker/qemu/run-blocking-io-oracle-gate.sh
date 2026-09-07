#!/bin/bash
# #813 opt-in boot_tests-only data oracle. Private image and reaping supervisor.
set -Eeuo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
source "$SCRIPT_DIR/lib/qemu-host-lock.sh"
source "$SCRIPT_DIR/lib/gate-structure-preflight.sh"
BREENIX_GATE_TMP="${BREENIX_GATE_TMP:-/tmp}"
ARCH=""
PROGRAM=""
BOOTS=1
QEMU_PID=""
CURRENT_SERIAL=""
report_gate_failure() {
    local status=$?
    trap - ERR
    echo "blocking I/O oracle gate: FAIL at ${BASH_SOURCE[0]}:${BASH_LINENO[0]}: ${BASH_COMMAND} (exit $status)"
    if [ -n "$QEMU_PID" ]; then
        kill "$QEMU_PID" 2>/dev/null || true
        wait "$QEMU_PID" 2>/dev/null || true
    fi
    if [ -f "$CURRENT_SERIAL" ]; then tail -200 "$CURRENT_SERIAL" || true; fi
    exit "$status"
}
trap report_gate_failure ERR
case "$BREENIX_GATE_TMP" in
    /*) ;;
    *) echo "FAIL: BREENIX_GATE_TMP must be absolute"; false ;;
esac
while [ "$#" -gt 0 ]; do
    case "$1" in
        --arch) ARCH="$2"; shift 2 ;;
        --program) PROGRAM="$2"; shift 2 ;;
        --boots) BOOTS="$2"; shift 2 ;;
        *) echo "FAIL: unknown option $1"; false ;;
    esac
done
case "$ARCH" in x86_64|aarch64) ;; *) echo "FAIL: --arch x86_64|aarch64 required"; false ;; esac
[ "$PROGRAM" = pipe_fifo_blocking_oracle ] || { echo "FAIL: unsupported --program"; false; }
case "$BOOTS" in ''|*[!0-9]*|0*) echo "FAIL: positive --boots required"; false ;; esac
[ "${#BOOTS}" -le 2 ] && [ "$BOOTS" -le 25 ] || { echo "FAIL: boot budget exceeds 25"; false; }
mkdir -p "$BREENIX_GATE_TMP"
OUTPUT_ROOT="$(mktemp -d "$BREENIX_GATE_TMP/breenix_813_${ARCH}.XXXXXX")"
echo "Artifacts: $OUTPUT_ROOT"

# Kept equal to the driver's derived arm set by pipe_fifo_blocking_structure.
EXPECTED_ARMS=(
    full_block
    atomic4096
    atomic4095
    large_block
    nonblock_full
    nonblock_atomic
    nonblock_large
    poll
    last_reader
    duplicate_reader
    signal_before
    signal_after
    ignored_signal
)

if ! gate_structure_preflight "$BREENIX_ROOT" "$BREENIX_GATE_TMP"; then
    echo "blocking I/O oracle gate preflight: FAIL (structure-suite preflight failed -- see GATE_PREFLIGHT line above)" >&2
    false
fi
cd "$BREENIX_ROOT"
./userspace/programs/build.sh --arch "$ARCH" 2>&1 | tee "$OUTPUT_ROOT/userspace-build.log"
if grep -Eq '^[[:space:]]*(warning|error)(\[|:)' "$OUTPUT_ROOT/userspace-build.log"; then
    echo "FAIL: userspace compile warnings/errors"; false
fi
if [ "$ARCH" = aarch64 ]; then
    cargo build --release --features boot_tests --target aarch64-breenix-kernel.json \
        -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
        -p kernel --bin kernel-aarch64 2>&1 | tee "$OUTPUT_ROOT/kernel-build.log"
    KERNEL="$BREENIX_ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
    "$BREENIX_ROOT/scripts/check-kernel-no-neon.sh" "$KERNEL"
    BASE_IMAGE="$BREENIX_ROOT/target/ext2-aarch64.img"
    BIN_DIR="$BREENIX_ROOT/userspace/programs/aarch64"
else
    # The ordinary init loader is required: testing/external_test_bins selects
    # the external test loader instead of /sbin/init. Use only boot_tests here.
    cargo build --release --features boot_tests --bin qemu-uefi 2>&1 | tee "$OUTPUT_ROOT/kernel-build.log"
    BREENIX_PRINT_UEFI_IMAGE=1 cargo run --release --features boot_tests --bin qemu-uefi >"$OUTPUT_ROOT/image-path.txt"
    UEFI_IMG="$(python3 - "$OUTPUT_ROOT/image-path.txt" <<'PY_IMAGE'
import pathlib, sys
lines = pathlib.Path(sys.argv[1]).read_text().splitlines()
paths = [line.split('=', 1)[1] for line in lines if line.startswith('UEFI_IMAGE=')]
assert len(paths) == 1 and pathlib.Path(paths[0]).is_file(), 'missing/ambiguous built UEFI image'
print(paths[0])
PY_IMAGE
)"
    BASE_IMAGE="$BREENIX_ROOT/target/ext2.img"
    BIN_DIR="$BREENIX_ROOT/userspace/programs"
    cp target/ovmf/x64/code.fd "$OUTPUT_ROOT/OVMF_CODE.fd"
    cp target/ovmf/x64/vars.fd "$OUTPUT_ROOT/OVMF_VARS.fd"
    dd if=/dev/zero of="$OUTPUT_ROOT/placeholder.img" bs=1048576 count=16 2>/dev/null
fi
# #559: accept only the disclosed pinned-nightly core NEON notice on ARM.
# Keep the notice in the build log and inspect its report, not just its summary.
python3 - "$ARCH" "$OUTPUT_ROOT" <<'PY_WARNINGS'
import pathlib, re, subprocess, sys
arch, directory = sys.argv[1:]
root = pathlib.Path(directory)
log = (root / 'kernel-build.log').read_text()
diagnostics = [line.strip() for line in log.splitlines()
               if re.match(r'\s*(warning|error)(\[|:)', line)]
if diagnostics:
    summary = (r'warning: the following packages contain code that will be rejected by a future '
               r'version of Rust: core v0\.0\.0 \([^\n]*nightly-2025-06-24-aarch64-apple-darwin/'
               r'lib/rustlib/src/rust/library/core\)')
    assert arch == 'aarch64' and len(diagnostics) == 1 and re.fullmatch(summary, diagnostics[0]), diagnostics
    report_id = re.search(r'cargo report future-incompatibilities --id (\d+)', log)
    assert report_id, 'missing future-incompatibility report ID'
    report = subprocess.check_output(['cargo', 'report', 'future-incompatibilities',
                                     '--id', report_id[1]], text=True)
    (root / 'future-incompatibilities.txt').write_text(report)
    warnings = set(re.findall(r'^> warning: (.*)$', report, re.M))
    assert warnings == {'enabling the `neon` target feature on the current target is unsound due to ABI issues'}, warnings
    assert 'https://github.com/rust-lang/rust/issues/134375' in report
    print('Disclosed #559 pinned-nightly core NEON notice; no other compile diagnostics')
PY_WARNINGS
if [ ! -f "$BASE_IMAGE" ]; then ./scripts/create_ext2_disk.sh --arch "$ARCH"; fi
cp "$BASE_IMAGE" "$OUTPUT_ROOT/oracle.img"
# Homebrew does not place e2fsprogs on PATH by default.
DEBUGFS="${BREENIX_DEBUGFS:-debugfs}"
if ! command -v "$DEBUGFS" >/dev/null 2>&1 && [ -x /opt/homebrew/opt/e2fsprogs/sbin/debugfs ]; then
    DEBUGFS=/opt/homebrew/opt/e2fsprogs/sbin/debugfs
fi
command -v "$DEBUGFS" >/dev/null
# Copy to short paths before debugfs parsing; the debugfs commands below use
# fixed relative filenames. Verify image bytes after installation.
cp "$BIN_DIR/pipe_fifo_blocking_oracle.elf" "$OUTPUT_ROOT/worker.elf"
cp "$BIN_DIR/pipe_fifo_blocking_supervisor.elf" "$OUTPUT_ROOT/supervisor.elf"
(
    cd "$OUTPUT_ROOT"
    cat >install.commands <<'COMMANDS'
rm /sbin/init
write supervisor.elf /sbin/init
set_inode_field /sbin/init mode 0100755
rm /bin/pipe_fifo_blocking_oracle
write worker.elf /bin/pipe_fifo_blocking_oracle
set_inode_field /bin/pipe_fifo_blocking_oracle mode 0100755
dump /sbin/init installed-supervisor.elf
dump /bin/pipe_fifo_blocking_oracle installed-worker.elf
COMMANDS
    "$DEBUGFS" -w -f install.commands oracle.img >install.log 2>&1
    cmp supervisor.elf installed-supervisor.elf
    cmp worker.elf installed-worker.elf
)
for ((boot=1; boot<=BOOTS; boot++)); do
    RUN_DIR="$OUTPUT_ROOT/boot_$boot"
    mkdir -p "$RUN_DIR"
    cp "$OUTPUT_ROOT/oracle.img" "$RUN_DIR/ext2.img"
    CURRENT_SERIAL="$RUN_DIR/serial.txt"
    : >"$CURRENT_SERIAL"
    if [ "$ARCH" = aarch64 ]; then
        qemu_host_lock_acquire qemu-system-aarch64
        qemu-system-aarch64 -M virt,gic-version=3 -cpu max -m 512 -smp 4 \
            -kernel "$KERNEL" -display none -no-reboot \
            -device virtio-gpu-device -device virtio-keyboard-device -device virtio-tablet-device \
            -device virtio-blk-device,drive=ext2 \
            -drive "if=none,id=ext2,format=raw,file=$RUN_DIR/ext2.img" \
            -device virtio-net-device,netdev=net0 -netdev user,id=net0 \
            -serial "file:$CURRENT_SERIAL" >"$RUN_DIR/qemu.log" 2>&1 &
    else
        qemu_host_lock_acquire qemu-system-x86_64
        cp "$OUTPUT_ROOT/OVMF_VARS.fd" "$RUN_DIR/OVMF_VARS.fd"
        qemu-system-x86_64 -pflash "$OUTPUT_ROOT/OVMF_CODE.fd" -pflash "$RUN_DIR/OVMF_VARS.fd" \
            -drive "if=none,id=hd,format=raw,readonly=on,file=$UEFI_IMG" \
            -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
            -drive "if=none,id=placeholder,format=raw,readonly=on,file=$OUTPUT_ROOT/placeholder.img" \
            -device virtio-blk-pci,drive=placeholder,disable-modern=on,disable-legacy=off \
            -drive "if=none,id=ext2disk,format=raw,file=$RUN_DIR/ext2.img" \
            -device virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off \
            -netdev user,id=net0 -device e1000,netdev=net0 \
            -machine pc,accel=tcg -cpu qemu64 -smp 1 -m 512 \
            -display none -no-reboot -no-shutdown \
            -serial "file:$CURRENT_SERIAL" -serial "file:$RUN_DIR/kernel.txt" \
            >"$RUN_DIR/qemu.log" 2>&1 &
    fi
    QEMU_PID=$!
    qemu_host_lock_track_pid "$QEMU_PID"
    elapsed=0
    while [ "$elapsed" -lt 120 ]; do
        if grep -aqF '[PIPE_WRITE_RESULT:' "$CURRENT_SERIAL"; then break; fi
        if grep -aqE 'KERNEL PANIC|DATA_ABORT|INSTRUCTION_ABORT|DOUBLE FAULT|TRIPLE FAULT|soft lockup detected' "$RUN_DIR"/*.txt; then break; fi
        kill -0 "$QEMU_PID" 2>/dev/null || break
        sleep 1
        elapsed=$((elapsed + 1))
    done
    kill "$QEMU_PID" 2>/dev/null || true
    wait "$QEMU_PID" 2>/dev/null || true
    QEMU_PID=""
    qemu_host_lock_release
    [ "$elapsed" -lt 120 ] || { echo "FAIL: host deadline"; false; }
    python3 - "$ARCH" "$RUN_DIR" "${EXPECTED_ARMS[@]}" <<'PY'
import pathlib, re, sys
arch, directory, *arms = sys.argv[1:]
text = '\n'.join(p.read_text(errors='replace') for p in pathlib.Path(directory).glob('*.txt'))
assert not re.search(r'KERNEL PANIC|panic!|DATA_ABORT|INSTRUCTION_ABORT|Unhandled sync exception|DOUBLE FAULT|TRIPLE FAULT|soft lockup detected', text, re.I), 'kernel crash'
results = re.findall(r'\[PIPE_WRITE_RESULT:([^\]]+)\]', text)
assert results == [f'{arch}:status=0'], f'missing/duplicate/nonzero reaped result: {results}'
assert ':verdict=FAIL' not in text, 'oracle arm failed'
records = re.findall(r'\[PIPE_WRITE_ORACLE:([^\]]+)\]', text)
for kind in ('pipe', 'fifo'):
    for arm in arms:
        prefix = f'{arch}:{kind}:{arm}:verdict=PASS:bytes='
        matches = [r for r in records if r.startswith(prefix)]
        assert matches, f'missing {kind}/{arm}'
        for record in matches:
            counts = re.fullmatch(re.escape(prefix) + r'(\d+):expected=(\d+)', record)
            assert counts and counts[1] == counts[2], f'byte tally: {record}'
summary = f'[PIPE_WRITE_SUMMARY:{arch}:passed={2 * len(arms)}:failed=0]'
assert summary in text, 'missing complete arm tally'
print(f'PASS: {arch}, {2 * len(arms)} arms, exact byte tallies and worker reaped')
PY
done
echo "PASS: blocking I/O oracle $ARCH boots=$BOOTS; serials=$OUTPUT_ROOT/boot_*/serial.txt"
