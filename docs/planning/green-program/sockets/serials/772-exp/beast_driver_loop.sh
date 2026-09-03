#!/bin/bash
set -uo pipefail
export BREENIX_REPO_DIR=/root/breenix-772-measure
export BREENIX_RUST_FORK=/root/breenix/rust-fork-real
cd "$BREENIX_REPO_DIR" || exit 1
mkdir -p /root/772-exp-serials
RESULTS=/root/772-exp-results.jsonl
: > "$RESULTS"
LOADLOG=/root/772-exp-load.log
: > "$LOADLOG"
BOOT_COUNT=0
MULTI_TURN_COUNT=0
GROUP=1
while [ "$BOOT_COUNT" -lt 60 ] && [ "$MULTI_TURN_COUNT" -lt 3 ]; do
  echo "=== GROUP $GROUP boot_count=$BOOT_COUNT multi_turn=$MULTI_TURN_COUNT ===" | tee -a "$LOADLOG"
  date >> "$LOADLOG"
  uptime >> "$LOADLOG"
  GROUP_LOG=/root/772-exp-group-$GROUP.log
  bash docker/qemu/run-x86-gate.sh 4 full > "$GROUP_LOG" 2>&1
  GATE_EXIT=$?
  echo "group $GROUP gate_exit=$GATE_EXIT" >> "$LOADLOG"
  for i in 1 2 3 4; do
    BOOT_COUNT=$((BOOT_COUNT+1))
    BOOT_TAG=$(printf '%02d' "$BOOT_COUNT")
    DEST=/root/772-exp-serials/boot_$BOOT_TAG
    mkdir -p "$DEST"
    cp /tmp/breenix_gate_$i/serial_kernel.log "$DEST"/ 2>/dev/null
    cp /tmp/breenix_gate_$i/serial_user.log "$DEST"/ 2>/dev/null
    cp /tmp/breenix_gate_$i/stdout.log "$DEST"/ 2>/dev/null
    grep "^  Test $i:" "$GROUP_LOG" > "$DEST/verdict.txt" 2>/dev/null
    TURNS_FILE=$DEST/turns.txt
    python3 /root/measure_boot.py "$BOOT_COUNT" "$DEST" "$TURNS_FILE" >> "$RESULTS"
    TURNS=$(cat "$TURNS_FILE" 2>/dev/null || echo -1)
    echo "boot=$BOOT_COUNT group=$GROUP idx=$i turns=$TURNS" >> "$LOADLOG"
    if [ "$TURNS" -ge 2 ]; then
      MULTI_TURN_COUNT=$((MULTI_TURN_COUNT+1))
    fi
    if [ "$BOOT_COUNT" -ge 60 ]; then
      break
    fi
  done
  GROUP=$((GROUP+1))
  if [ "$BOOT_COUNT" -ge 60 ]; then
    break
  fi
  if [ "$MULTI_TURN_COUNT" -ge 3 ]; then
    break
  fi
done
echo "DONE boots=$BOOT_COUNT multi_turn=$MULTI_TURN_COUNT" | tee -a "$LOADLOG"
