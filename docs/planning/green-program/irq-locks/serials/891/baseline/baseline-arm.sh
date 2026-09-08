#!/bin/bash
set -eu
cd /private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad/s891/wt
export TMPDIR="$PWD/.tmp" BREENIX_GATE_TMP="$PWD/.gate-tmp"
source docker/qemu/lib/qemu-host-lock.sh
qemu_host_lock_acquire
out=docs/planning/green-program/irq-locks/serials/891/baseline
git rev-parse HEAD > "$out/arm-revision.txt"
qemu-system-aarch64 -M virt,gic-version=3 -cpu max -m 512 -smp 4 -kernel target/aarch64-breenix-kernel/release/kernel-aarch64 -display none -no-reboot -device virtio-gpu-device -device virtio-keyboard-device -device virtio-tablet-device -device virtio-blk-device,drive=ext2 -drive if=none,id=ext2,format=raw,file=target/ext2-aarch64.img,snapshot=on -device virtio-net-device,netdev=net0 -netdev user,id=net0 -serial file:"$out/arm-serial.txt" -gdb tcp::18910 -S > "$out/arm-qemu.txt" 2>&1 &
qpid=$!
qemu_host_lock_track_pid "$qpid"
timeout 60 gdb -q -batch -x .tmp/baseline.gdb > "$out/arm-gdb.txt" 2>&1 || true
kill "$qpid" 2>/dev/null || true
wait "$qpid" || true
