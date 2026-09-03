#!/bin/bash
# #693 aarch64 repeat driver. Boots the worktree's own kernel + ext2 image once
# per iteration, one QEMU at a time, and cuts each boot short as soon as the
# poll oracle's verdict marker lands. Each boot gets a private writable copy of
# the ext2 image so a filesystem write in one boot cannot reach the next.
set -u
COUNT="${1:-25}"
TAG="${2:-run}"
ROOT="${ROOT:-/Users/wrb/fun/code/breenix.worktrees/693-poll-wake-loss}"
OUT="${OUT:-/tmp/breenix693/$TAG}"
HARD_TIMEOUT="${HARD_TIMEOUT:-90}"
GRACE="${GRACE:-3}"
KERNEL="$ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
EXT2="$ROOT/target/ext2-aarch64.img"
mkdir -p "$OUT"

for i in $(seq 1 "$COUNT"); do
  D="$OUT/boot$i"
  rm -rf "$D"; mkdir -p "$D"
  cp "$EXT2" "$D/ext2.img"
  qemu-system-aarch64 \
    -M virt,gic-version=3 -cpu max -m 512 -smp 4 \
    -kernel "$KERNEL" \
    -display none -no-reboot \
    -device virtio-gpu-device \
    -device virtio-keyboard-device \
    -device virtio-tablet-device \
    -device virtio-blk-device,drive=ext2 \
    -drive if=none,id=ext2,format=raw,file="$D/ext2.img" \
    -device virtio-net-device,netdev=net0 \
    -netdev user,id=net0 \
    -serial file:"$D/serial.txt" > "$D/stdout.log" 2>&1 &
  QPID=$!
  SEEN=0
  for ((t=0; t<HARD_TIMEOUT; t+=2)); do
    sleep 2
    kill -0 "$QPID" 2>/dev/null || break
    if grep -qaE '\[POLL_TCP_ORACLE:(PASS|FAIL)' "$D/serial.txt" 2>/dev/null; then
      SEEN=1; sleep "$GRACE"; break
    fi
  done
  # Kill only the PID this loop started. No name-based pkill.
  kill -9 "$QPID" 2>/dev/null
  wait "$QPID" 2>/dev/null
  sleep 1

  VERDICT="$(grep -haoE '\[POLL_TCP_ORACLE:(PASS|FAIL):[^]]*\]' "$D/serial.txt" 2>/dev/null | head -1)"
  LATEPUB="$(grep -haoE '\[POLL_TCP_ORACLE:LATE_PUBLISH:[^]]*\]' "$D/serial.txt" 2>/dev/null | tr '\n' '|')"
  TMO="$(grep -hac 'POLL_TCP_TIMEOUT' "$D/serial.txt" 2>/dev/null)"
  LOST="$(grep -hac 'POLL_TCP_READY_LOST' "$D/serial.txt" 2>/dev/null)"
  echo "[$TAG boot $i] marker_seen=$SEEN poll_tcp_timeout=$TMO ready_lost=$LOST"
  echo "   verdict: ${VERDICT:-<none>}"
  echo "   latepub: ${LATEPUB:-<none>}"
done
