#!/bin/bash
set -uo pipefail
cd /root/breenix-x927b
unset BREENIX_RUST_FORK_LIBRARY
export TMPDIR=/root/breenix-x927b-tmp BREENIX_GATE_TMP=/root/breenix-x927b-tmp
mkdir -p "$TMPDIR/reverify"
exec > "$TMPDIR/reverify/driver-after-fixture.log" 2>&1
echo "REVISION=$(git rev-parse HEAD)"
if ! compgen -G 'userspace/programs/x86_64/*.elf' >/dev/null; then
 { echo "REVISION=$(git rev-parse HEAD)"; ./userspace/programs/build.sh; } > "$TMPDIR/reverify/userspace-setup.log" 2>&1
 rc=$?; echo "userspace setup exit=$rc"; test "$rc" = 0 || exit "$rc"
fi
for item in 'oracle-fixed|bash docker/qemu/run-blocking-io-oracle-gate.sh --arch x86_64' 'precommit|cargo build --release --features testing,external_test_bins --bin qemu-uefi' 'prod|bash docker/qemu/run-x86-prod-profile-boot-test.sh'; do
 name=${item%%|*}; command=${item#*|}; logfile="$TMPDIR/reverify/$name.log"
 { echo "REVISION=$(git rev-parse HEAD)"; echo "COMMAND=$command"; bash -lc "$command"; rc=$?; echo "COMMAND_EXIT=$rc"; exit "$rc"; } > "$logfile" 2>&1 &
 child=$!; wait "$child"; rc=$?
 echo "$name exit=$rc"
 if test "$rc" != 0; then
  if grep -qiE 'SCSI.*error|I/O error|Input/output error' "$logfile"; then echo 'STOP: issue 871'; exit "$rc"; fi
  if grep -qiE 'TIMEOUT|timed out' "$logfile" && grep -qi 'preflight' "$logfile"; then
   echo "$name structure timeout: one retry with BREENIX_STRUCTURE_SUITE_TIMEOUT_SECS=900"
   export BREENIX_STRUCTURE_SUITE_TIMEOUT_SECS=900
   { echo "REVISION=$(git rev-parse HEAD)"; echo 'BREENIX_STRUCTURE_SUITE_TIMEOUT_SECS=900'; echo "COMMAND=$command"; bash -lc "$command"; rc=$?; echo "COMMAND_EXIT=$rc"; exit "$rc"; } > "$TMPDIR/reverify/$name-retry.log" 2>&1 &
   child=$!; wait "$child"; rc=$?; unset BREENIX_STRUCTURE_SUITE_TIMEOUT_SECS
   echo "$name retry exit=$rc"
  fi
  test "$rc" = 0 || exit "$rc"
 fi
done
echo DRIVER_COMPLETE
