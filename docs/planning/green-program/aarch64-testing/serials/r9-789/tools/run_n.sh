#!/bin/bash
# Boot the aarch64 production profile N times, scoring pass / wedge.
# usage: run_n.sh <id> <gdbport> <kernel-elf> <n>
set -u
ID="$1"; PORT="$2"; KERNEL="$3"; N="${4:-4}"
SP=/private/tmp/claude-501/-Users-wrb-fun-code-breenix/d69ffb9d-4539-4cf3-8a3d-a872ff7c830b/scratchpad
ROOT=/Users/wrb/fun/code/breenix/.claude/worktrees/wf_c85d943c-7ff-1
BASE="$SP/runs/$ID"
rm -rf "$BASE"; mkdir -p "$BASE"

for (( att=1; att<=N; att++ )); do
  DIR="$BASE/att$att"
  mkdir -p "$DIR"
  cp "$ROOT/target/ext2-aarch64.img" "$DIR/ext2.img"
  qemu-system-aarch64 \
      -M virt,gic-version=3 -cpu max -m 512 -smp 4 \
      -kernel "$KERNEL" \
      -display none -no-reboot \
      -device virtio-gpu-device \
      -device virtio-keyboard-device \
      -device virtio-tablet-device \
      -device virtio-blk-device,drive=ext2 \
      -drive if=none,id=ext2,format=raw,file="$DIR/ext2.img" \
      -device virtio-net-device,netdev=net0 \
      -netdev user,id=net0 \
      -serial "file:$DIR/serial.txt" \
      -gdb "tcp::$PORT" > "$DIR/qemu.log" 2>&1 &
  QPID=$!
  last_size=0
  same=0
  verdict=""
  for (( t=1; t<=130; t++ )); do
    sleep 1
    if ! kill -0 "$QPID" 2>/dev/null; then verdict="qemu-exited"; break; fi
    sz=$(wc -c < "$DIR/serial.txt" 2>/dev/null || echo 0)
    if grep -qF 'bsshd: listening' "$DIR/serial.txt" 2>/dev/null; then verdict="pass"; break; fi
    if [ "$sz" -eq "$last_size" ]; then same=$((same+1)); else same=0; last_size=$sz; fi
    if [ "$same" -ge 12 ] && [ "$sz" -gt 12000 ]; then verdict="stalled-output"; break; fi
    if [ "$t" -ge 80 ]; then verdict="no-bsshd-80s"; break; fi
  done
  [ -z "$verdict" ] && verdict="window-elapsed"
  cpu=$(ps -o %cpu= -p "$QPID" 2>/dev/null | tr -d ' ')
  echo "attempt=$att verdict=$verdict qemu_cpu=${cpu:-na} bytes=$(wc -c < "$DIR/serial.txt" 2>/dev/null)" | tee -a "$BASE/log.txt"
  kill "$QPID" 2>/dev/null
  wait "$QPID" 2>/dev/null
done
echo "done $ID" | tee -a "$BASE/log.txt"
