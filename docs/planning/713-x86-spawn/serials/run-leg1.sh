#!/bin/bash
set -u
cd /root/breenix
N=12
PASS=0
FAIL=0
for i in $(seq 1 $N); do
  echo "=== BOOT $i/$N $(date) ==="
  ./docker/qemu/run-x86-prod-profile-boot-test.sh > /root/p713-prove/leg1/boot-$i.log 2>&1
  rc=$?
  if [ $rc -eq 0 ]; then
    PASS=$((PASS+1))
    echo "boot $i: PASS"
  else
    FAIL=$((FAIL+1))
    echo "boot $i: FAIL rc=$rc"
  fi
  cp -r /tmp/breenix_x86_prod_profile /root/p713-prove/leg1/boot-$i-serials 2>/dev/null || true
done
echo "LEG1 DONE: PASS=$PASS FAIL=$FAIL of $N"
