#!/bin/bash
set -u
cd /root/breenix-s891 || exit 1
export BREENIX_GATE_TMP=/root/breenix-s891-tmp TMPDIR=/root/breenix-s891-tmp
unset BREENIX_RUST_FORK_LIBRARY
mkdir -p "$TMPDIR/landing-merged"
out="$TMPDIR/landing-merged"
load_check() {
 uptime
 local load=$(awk '{print $1}' /proc/loadavg)
 if awk -v load="$load" 'BEGIN {exit !(load > 8)}'; then
  echo 'R238: waiting below 6, up to 2 hours'
  local ready=0
  for ((i=0;i<24;i++)); do
   sleep 300
   uptime
   load=$(awk '{print $1}' /proc/loadavg)
   if awk -v load="$load" 'BEGIN {exit !(load < 6)}'; then ready=1; break; fi
  done
  [ "$ready" -eq 1 ] || return 75
 fi
 echo "R238 launch load=$load"
}
for gate in x86 parallel; do
 if [ "$gate" = parallel ]; then
  git rev-parse HEAD > "$out/parallel-build.txt"
  cargo build --release --features testing,external_test_bins --bin qemu-uefi >> "$out/parallel-build.txt" 2>&1 || exit $?
  if grep -Eq '^warning:|^error' "$out/parallel-build.txt"; then echo PROJECT_DIAGNOSTICS; exit 1; fi
  git rev-parse HEAD > "$out/parallel-preflight.txt"
  load_check >> "$out/parallel-preflight.txt" 2>&1 || exit $?
  bash scripts/run-structure-tests.sh >> "$out/parallel-preflight.txt" 2>&1
  result=$?
  echo "GATE_EXIT=$result" >> "$out/parallel-preflight.txt"
  [ "$result" -eq 0 ] || exit "$result"
 fi
 git rev-parse HEAD > "$out/$gate.txt"
 load_check >> "$out/$gate.txt" 2>&1 || exit $?
 if [ "$gate" = x86 ]; then
  bash docker/qemu/run-x86-boot-tests.sh >> "$out/$gate.txt" 2>&1
 else
  bash docker/qemu/run-boot-parallel.sh 5 >> "$out/$gate.txt" 2>&1
 fi
 result=$?
 echo "GATE_EXIT=$result" >> "$out/$gate.txt"
 [ "$result" -eq 0 ] || exit "$result"
done
echo LANDING_GATES_GREEN
