#!/bin/bash
# #693 fix battery, x86, hermetic. Boots a PRIVATE snapshot of the branch's own
# images with the argv the qemu-uefi runner emits under BREENIX_NET_MODE=none,
# so a concurrent rebuild of any shared tree cannot swap the kernel under a
# running boot. Sequential: one QEMU at a time, and the only process this script
# kills is the PID it started.
set -u
COUNT="${1:-25}"
TAG="${2:-fix-tcg}"
ACCEL="${ACCEL:-tcg}"
CPU="${CPU:-qemu64}"
HARD_TIMEOUT="${HARD_TIMEOUT:-300}"
GRACE="${GRACE:-6}"
BIN=/root/bx693-bin
OUT="/root/bx693-out/$TAG"
mkdir -p "$OUT"

for i in $(seq 1 "$COUNT"); do
  D="$OUT/boot$i"
  rm -rf "$D"; mkdir -p "$D"
  cp "$BIN/vars.fd" "$D/OVMF_VARS.fd"
  cp "$BIN/ext2.img" "$D/ext2.img"
  qemu-system-x86_64 \
    -pflash "$BIN/code.fd" \
    -pflash "$D/OVMF_VARS.fd" \
    -drive "if=none,id=hd,format=raw,media=disk,file=$BIN/breenix-uefi.img" \
    -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
    -drive "if=none,id=testdisk,format=raw,file=$BIN/test_binaries.img" \
    -device virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off \
    -drive "if=none,id=ext2disk,format=raw,file=$D/ext2.img" \
    -device virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off \
    -machine "pc,accel=$ACCEL" -cpu "$CPU" -smp 1 -m 512 \
    -device virtio-vga,xres=1920,yres=1080 \
    -display none -boot strict=on -no-reboot -no-shutdown -monitor none \
    -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
    -fw_cfg name=opt/org.tianocore/StdoutToSerial,string=1 \
    -serial file:"$D/serial_user.log" \
    -serial file:"$D/serial_kernel.log" \
    > "$D/stdout.log" 2>&1 &
  QPID=$!
  SEEN=0
  for ((t=0; t<HARD_TIMEOUT; t+=2)); do
    sleep 2
    kill -0 "$QPID" 2>/dev/null || break
    if grep -qhaE '\[POLL_TCP_ORACLE:(PASS|FAIL)' "$D/serial_user.log" "$D/serial_kernel.log" 2>/dev/null; then
      SEEN=1; sleep "$GRACE"; break
    fi
  done
  kill -9 "$QPID" 2>/dev/null
  wait "$QPID" 2>/dev/null
  sleep 2

  VERDICT="$(grep -haoE '\[POLL_TCP_ORACLE:(PASS|FAIL):[^]]*\]' "$D"/serial_*.log 2>/dev/null | head -1)"
  LATEPUB="$(grep -haoE '\[POLL_TCP_ORACLE:LATE_PUBLISH:[^]]*\]' "$D"/serial_*.log 2>/dev/null | sort -u | tr '\n' '|')"
  TMO="$(grep -hac 'POLL_TCP_TIMEOUT' "$D"/serial_*.log 2>/dev/null | awk '{s+=$1} END {print s+0}')"
  LOST="$(grep -hac 'POLL_TCP_READY_LOST' "$D"/serial_*.log 2>/dev/null | awk '{s+=$1} END {print s+0}')"
  LOADED="$(grep -hac "Loaded 'poll_tcp_oracle' from test disk" "$D/serial_kernel.log" 2>/dev/null | awk '{s+=$1} END {print s+0}')"
  echo "[$TAG boot $i] marker_seen=$SEEN oracle_loaded=$LOADED poll_tcp_timeout=$TMO ready_lost=$LOST"
  echo "   verdict: ${VERDICT:-<none>}"
  echo "   latepub: ${LATEPUB:-<none>}"
done
