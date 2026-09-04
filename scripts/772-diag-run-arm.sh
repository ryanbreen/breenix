#!/usr/bin/env bash
# #772 diag battery (R111/R112 measure slot): build one feature-flag arm,
# then boot it N times, reading the DISPATCH_* counters out-of-band over GDB
# on each boot -- 0 reads from inside the guest under test.
#
# Each boot is scored the same way run-x86-gate.sh's full mode scores a
# boot -- scripts/x86-gate-verdict.sh with EXPECTED_EXITS=10, not a marker
# grep -- via scripts/772-dispatch-boot.sh, which this script loops. That
# driver already does the two things this battery needs that
# run-x86-gate.sh's own boot loop does not: it opens the QEMU gdbstub
# (`-s`) and keeps the guest alive long enough after the completion markers
# to connect GDB and read each `DISPATCH_*` counter symbol before killing
# the guest by PID (R84 -- not by process name).
#
# This script does NOT repack the userspace test disk or ext2 image --
# that is arm-independent (no arm here touches userspace/), so the caller
# does it once, before the first arm, the same way run-x86-gate.sh does.
#
# Usage: 772-diag-run-arm.sh <arm-tag> <features> <num-boots> <outdir> [repo-root]

set -uo pipefail

ARM="${1:?usage: 772-diag-run-arm.sh <arm-tag> <features> <num-boots> <outdir> [repo-root]}"
FEATURES="${2:?usage}"
NBOOTS="${3:?usage}"
OUTDIR="${4:?usage}"
REPO="${5:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

cd "$REPO" || exit 1
# shellcheck disable=SC1091
source "$HOME/.cargo/env" 2>/dev/null || export PATH="$HOME/.cargo/bin:$PATH"

mkdir -p "$OUTDIR"

echo "[$ARM] building features=$FEATURES"
BUILD_LOG="$OUTDIR/build.log"
BUILD_START=$SECONDS
if ! cargo build --release --features "$FEATURES" --bin qemu-uefi >"$BUILD_LOG" 2>&1; then
  echo "[$ARM] BUILD FAILED -- see $BUILD_LOG"
  tail -60 "$BUILD_LOG"
  exit 1
fi
if grep -qE "^(warning|error)" "$BUILD_LOG"; then
  echo "[$ARM] BUILD PRODUCED WARNINGS/ERRORS -- see $BUILD_LOG"
  grep -E "^(warning|error)" "$BUILD_LOG"
  exit 1
fi
echo "[$ARM] build clean (0 warnings) in $((SECONDS - BUILD_START))s"

PASS=0
FAIL=0
for i in $(seq 1 "$NBOOTS"); do
  BOOTDIR="$OUTDIR/boot_$(printf '%02d' "$i")"
  DRIVER_LOG="$OUTDIR/boot_$(printf '%02d' "$i").driver.log"
  echo "[$ARM] boot $i/$NBOOTS -> $BOOTDIR"
  "$REPO/scripts/772-dispatch-boot.sh" "$BOOTDIR" "${ARM}_${i}" "$REPO" \
    >"$DRIVER_LOG" 2>&1
  RESULT_LINE=$(grep '^RESULT ' "$DRIVER_LOG" | tail -1)
  echo "  $RESULT_LINE"
  if printf '%s' "$RESULT_LINE" | grep -q 'verdict_rc=0'; then
    PASS=$((PASS + 1))
  else
    FAIL=$((FAIL + 1))
  fi
done

echo "[$ARM] DONE: $PASS/$NBOOTS verdict_rc=0"
