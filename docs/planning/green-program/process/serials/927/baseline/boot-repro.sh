set -Eeuo pipefail
cd /root/breenix-927a
source /root/.cargo/env
export BREENIX_GATE_TMP=/root/breenix-927a-tmp TMPDIR=/root/breenix-927a-tmp
OUT="$TMPDIR/${1:-repro}"
mkdir -p "$OUT"
printf 'REVISION='; git rev-parse HEAD
source docker/qemu/lib/qemu-host-lock.sh
UEFI_IMG=$(sed -n 's/^UEFI_IMAGE=//p' "$TMPDIR/repro-image.txt")
cp target/ovmf/x64/code.fd "$OUT/OVMF_CODE.fd"
cp target/ovmf/x64/vars.fd "$OUT/OVMF_VARS.fd"
truncate -s 16M "$OUT/placeholder.img"
qemu_host_lock_acquire qemu-system-x86_64
ARGS=()
if [ "${1:-}" = gdb ]; then ARGS=(-gdb tcp:127.0.0.1:1927 -S); fi
qemu-system-x86_64 -pflash "$OUT/OVMF_CODE.fd" -pflash "$OUT/OVMF_VARS.fd" \
-drive "if=none,id=hd,format=raw,readonly=on,file=$UEFI_IMG" -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
-drive "if=none,id=testdisk,format=raw,readonly=on,file=$OUT/placeholder.img" -device virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off \
-machine pc,accel=tcg -cpu qemu64 -smp 1 -m 512 -display none -no-reboot -no-shutdown \
-device isa-debug-exit,iobase=0xf4,iosize=0x04 -serial "file:$OUT/serial_user.txt" -serial "file:$OUT/serial_kernel.txt" "${ARGS[@]}" >"$OUT/qemu.log" 2>&1 &
QEMU_PID=$!
echo "$QEMU_PID" >"$OUT/qemu.pid"
qemu_host_lock_track_pid "$QEMU_PID"
if [ "${1:-}" = gdb ]; then wait "$QEMU_PID"; else
for ((i=0;i<120;i++)); do
    if grep -aq 'x86 retire cohort gate failed' "$OUT"/serial_*.txt; then break; fi
    kill -0 "$QEMU_PID" || break
    sleep 1
done
kill "$QEMU_PID" 2>/dev/null || true
wait "$QEMU_PID" 2>/dev/null || true
qemu_host_lock_release
fi
