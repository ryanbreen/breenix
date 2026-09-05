#!/bin/bash
set -u
cd /root/breenix
N=12
PASS=0
FAIL=0
# #797 review F4: this script predates BREENIX_GATE_TMP and was run with it
# unset, so /tmp/breenix_x86_prod_profile was in fact where the boot below
# wrote (this default is unchanged by #797's fix). Reading it back through
# the same variable, rather than a bare /tmp literal, keeps this driver
# correct if it is ever re-run under a caller that now sets
# BREENIX_GATE_TMP -- the exact downstream-reader breakage #797 warns about.
BREENIX_GATE_TMP="${BREENIX_GATE_TMP:-/tmp}"
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
  if ! cp -r "$BREENIX_GATE_TMP/breenix_x86_prod_profile" "/root/p713-prove/leg1/boot-$i-serials"; then
    echo "boot $i: WARN could not capture serials from $BREENIX_GATE_TMP/breenix_x86_prod_profile" >&2
  fi
done
echo "LEG1 DONE: PASS=$PASS FAIL=$FAIL of $N"
