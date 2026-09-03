#!/bin/bash
set -uo pipefail
export BREENIX_REPO_DIR=/root/breenix-772-r2
export BREENIX_RUST_FORK=/root/breenix/rust-fork-real
cd "$BREENIX_REPO_DIR" || exit 1

RESULTS_DIR=/root/772r2-serials
mkdir -p "$RESULTS_DIR"
RESULTS_JSONL=/root/772r2-results.jsonl
: > "$RESULTS_JSONL"
LOADLOG=/root/772r2-load.log
: > "$LOADLOG"
PROGRESS=/root/772r2-progress.txt
echo "boot_count=0 multi_turn=0 status=running" > "$PROGRESS"

BOOT_COUNT=0
MULTI_TURN_COUNT=0
GROUP=1
MAX_BOOTS=120

while [ "$BOOT_COUNT" -lt "$MAX_BOOTS" ] && [ "$MULTI_TURN_COUNT" -lt 3 ]; do
  echo "=== GROUP $GROUP boot_count=$BOOT_COUNT multi_turn=$MULTI_TURN_COUNT ===" | tee -a "$LOADLOG"
  date >> "$LOADLOG"
  uptime >> "$LOADLOG"
  GROUP_LOG=/root/772r2-group-$GROUP.log
  bash docker/qemu/run-x86-gate.sh 4 full > "$GROUP_LOG" 2>&1
  GATE_EXIT=$?
  echo "group $GROUP gate_exit=$GATE_EXIT" >> "$LOADLOG"

  for i in 1 2 3 4; do
    BOOT_COUNT=$((BOOT_COUNT+1))
    BOOT_TAG=$(printf 'boot_%03d' "$BOOT_COUNT")
    DEST="$RESULTS_DIR/$BOOT_TAG"
    mkdir -p "$DEST"
    cp "/tmp/breenix_gate_$i/serial_kernel.log" "$DEST/serial_kernel.txt" 2>/dev/null
    cp "/tmp/breenix_gate_$i/serial_user.log" "$DEST/serial_user.txt" 2>/dev/null
    cp "/tmp/breenix_gate_$i/stdout.log" "$DEST/stdout.txt" 2>/dev/null
    grep "^  Test $i:" "$GROUP_LOG" > "$DEST/verdict.txt" 2>/dev/null

    python3 /root/census_r2.py "$BOOT_TAG" "$DEST" "$DEST/census.json" >> "$RESULTS_JSONL"
    DATA_TURNS=$(python3 -c "import json
d=json.load(open('$DEST/census.json'))
v=d.get('data_turns')
print(v if v is not None else -1)")
    EOF_TURNS=$(python3 -c "import json
d=json.load(open('$DEST/census.json'))
v=d.get('eof_turns')
print(v if v is not None else -1)")
    echo "boot=$BOOT_COUNT group=$GROUP idx=$i data_turns=$DATA_TURNS eof_turns=$EOF_TURNS" >> "$LOADLOG"

    if [ "$DATA_TURNS" -ge 2 ]; then
      MULTI_TURN_COUNT=$((MULTI_TURN_COUNT+1))
    fi
    echo "boot_count=$BOOT_COUNT multi_turn=$MULTI_TURN_COUNT status=running" > "$PROGRESS"

    if [ "$BOOT_COUNT" -ge "$MAX_BOOTS" ]; then
      break
    fi
    if [ "$MULTI_TURN_COUNT" -ge 3 ]; then
      break
    fi
  done

  GROUP=$((GROUP+1))
done

echo "boot_count=$BOOT_COUNT multi_turn=$MULTI_TURN_COUNT status=done" > "$PROGRESS"
echo "DONE boots=$BOOT_COUNT multi_turn=$MULTI_TURN_COUNT" | tee -a "$LOADLOG"
