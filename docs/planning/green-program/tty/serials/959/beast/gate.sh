#!/bin/bash
set -u
cd /root/breenix-t959
unset BREENIX_RUST_FORK_LIBRARY
export BREENIX_GATE_TMP=/root/breenix-t959-tmp TMPDIR=/root/breenix-t959-tmp
git rev-parse HEAD
if ! compgen -G 'userspace/programs/x86_64/*.elf' > /dev/null; then
 ./userspace/programs/build.sh || exit $?
fi
uptime | tee /root/breenix-t959-tmp/load-initial.txt
load=$(uptime | sed -E 's/.*load average: ([0-9.]+).*/\1/')
if awk "BEGIN {exit !($load > 8)}"; then
 for round in $(seq 1 24); do
  sleep 300
  uptime
  load=$(uptime | sed -E 's/.*load average: ([0-9.]+).*/\1/')
  if awk "BEGIN {exit !($load < 6)}"; then break; fi
 done
 if ! awk "BEGIN {exit !($load < 6)}"; then echo 'LOAD_RULE_BLOCKED'; exit 2; fi
fi
uptime | tee /root/breenix-t959-tmp/load-launch.txt
{ git rev-parse HEAD; bash docker/qemu/run-x86-boot-tests.sh 1; } > /root/breenix-t959-tmp/boot-tests.txt 2>&1
rc=$?
if [ "$rc" -ne 0 ] && grep -q 'GATE_PREFLIGHT: FAIL' /root/breenix-t959-tmp/boot-tests.txt && grep -q 'exit=124' /root/breenix-t959-tmp/boot-tests.txt; then
 echo 'Structure-preflight timeout: retry once with 900 seconds.'
 export BREENIX_STRUCTURE_SUITE_TIMEOUT_SECS=900
 { git rev-parse HEAD; bash docker/qemu/run-x86-boot-tests.sh 1; } > /root/breenix-t959-tmp/boot-tests-retry.txt 2>&1
 rc=$?
fi
echo "$rc" > /root/breenix-t959-tmp/gate.exit
exit "$rc"
