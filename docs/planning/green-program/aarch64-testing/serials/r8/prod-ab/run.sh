#!/bin/bash
# R7-003 attribution A/B: 8 production-profile boots on main d6b7a186 and 8 on
# the branch head, alternating, 2 concurrent (one arm each round).
HEAD_WT=/Users/wrb/fun/code/breenix/.claude/worktrees/wf_af0ecd9a-65b-1
MAIN_WT=/Users/wrb/fun/code/breenix.worktrees/r8-main-prod-ab
OUT=/tmp/r8_prod_ab
mkdir -p "$OUT/serials"
for i in $(seq 1 8); do
  echo "=== round $i $(date -u +%H:%M:%SZ) load=$(uptime | sed 's/.*averages: //') ==="
  (
    cd "$MAIN_WT" || exit 1
    BREENIX_PROD_PROFILE_OUTPUT_DIR=/tmp/r8_prod_main \
    BREENIX_PROD_PROFILE_FAILURE_DIR=/tmp/r8_prod_main_failures \
    ./docker/qemu/run-aarch64-prod-profile-boot-test.sh > "$OUT/main_$i.log" 2>&1
    echo "main_$i exit=$?" >> "$OUT/exitcodes.txt"
    cp /tmp/r8_prod_main/serial.txt "$OUT/serials/main_$i-serial.txt" 2>/dev/null
  ) &
  P1=$!
  (
    cd "$HEAD_WT" || exit 1
    BREENIX_PROD_PROFILE_OUTPUT_DIR=/tmp/r8_prod_head \
    BREENIX_PROD_PROFILE_FAILURE_DIR=/tmp/r8_prod_head_failures \
    ./docker/qemu/run-aarch64-prod-profile-boot-test.sh > "$OUT/head_$i.log" 2>&1
    echo "head_$i exit=$?" >> "$OUT/exitcodes.txt"
    cp /tmp/r8_prod_head/serial.txt "$OUT/serials/head_$i-serial.txt" 2>/dev/null
  ) &
  P2=$!
  wait $P1 $P2
  echo "--- round $i verdicts:"
  tail -3 "$OUT/main_$i.log" | sed 's/^/  main: /'
  tail -3 "$OUT/head_$i.log" | sed 's/^/  head: /'
done
echo "=== A/B COMPLETE $(date -u +%H:%M:%SZ) ==="
