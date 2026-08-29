#!/bin/bash
#
# x86_64 build + boot gate, in the repository.
#
# This is the gate that guards merges on the beast x86 VM. It used to exist only
# as a hand-maintained `/root/run-x86-gate.sh` on that VM, which is #564: every
# hardening applied to it was one re-provision away from being lost, and two of
# its properties lived nowhere else. Both are now versioned here:
#
#   1. IT REPACKS THE USERSPACE TEST DISK. `./userspace/programs/build.sh`
#      rebuilds the ELFs but `target/test_binaries.img` is only PACKED by
#      `cargo run -p xtask -- create-test-disk`. Both are gitignored build
#      outputs, so without the repack a gate run on a branch that touches
#      `userspace/` or `libs/libbreenix-libc` boots the PREVIOUS branch's
#      binaries and reports green. This was hit for real: the kernel logged
#      `Loaded 'brk_test' from test disk (182448 bytes)` while the rebuilt ELF
#      on disk was 182496 bytes. The ext2 image carries the same binaries and is
#      rebuilt for the same reason.
#   2. IT SCORES `full` MODE WITH scripts/x86-gate-verdict.sh, not a liveness
#      marker grep, and passes the mandatory EXPECTED_EXITS. Marker-grep
#      blindness is what that verdict script exists to end.
#
# The VM-specific bits are env vars with sane defaults, so the same script runs
# on any x86 host:
#
#   BREENIX_REPO_DIR    repository to run in (default: this checkout)
#   BREENIX_QEMU_ACCEL  QEMU accelerator     (default: kvm on Linux, else tcg)
#   BREENIX_QEMU_CPU    QEMU cpu model       (default: host with kvm, else qemu64)
#   BREENIX_RUST_FORK   if set, `rust-fork` is repointed at this path first. The
#                       committed `rust-fork` symlink names a Mac-only path; the
#                       beast VM keeps a real clone and needs the repoint. Not
#                       committed, not required elsewhere.
#   BREENIX_GATE_TIMEOUT per-boot timeout in seconds (default: 150)
#
# What is NOT here, and cannot be: the fetch/checkout of the branch under test.
# Something outside the working tree has to put the code there before a script
# inside it can run, and a script that `git reset --hard`s the checkout it is
# itself being read from is a self-modification hazard. The VM keeps a ~10-line
# bootstrap that fetches, checks out, and then execs THIS file. Everything that
# can be versioned is versioned.
#
# Usage: docker/qemu/run-x86-gate.sh [count] [mode]
#   count : boot tests to run, capped at 4 (default 1)
#   mode  : kthread (default, fast) or full (testing,external_test_bins)

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEFAULT_REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

COUNT="${1:-1}"
MODE="${2:-kthread}"
MAX_CONCURRENCY=4
REPO_DIR="${BREENIX_REPO_DIR:-$DEFAULT_REPO_DIR}"
TIMEOUT_SECS="${BREENIX_GATE_TIMEOUT:-150}"

# Non-interactive shells don't source .bashrc/.profile, so put cargo on PATH.
source "$HOME/.cargo/env" 2>/dev/null || export PATH="$HOME/.cargo/bin:$PATH"

if [ "$COUNT" -gt "$MAX_CONCURRENCY" ]; then
  echo "[gate] Capping concurrency at $MAX_CONCURRENCY (requested $COUNT)"
  COUNT=$MAX_CONCURRENCY
fi

cd "$REPO_DIR" || { echo "GATE: FAIL (repo dir missing: $REPO_DIR)"; exit 1; }

TOTAL_START=$SECONDS
echo "[gate] repo: $REPO_DIR  head: $(git rev-parse --short HEAD 2>/dev/null || echo unknown)"

# Accelerator defaults: nested KVM where it exists (beast), TCG elsewhere. TCG
# boot times under host contention are 10-50x slower, which is why the VM sets
# these; qemu-uefi.rs reads both env vars directly.
if [ -z "${BREENIX_QEMU_ACCEL:-}" ]; then
  if [ -w /dev/kvm ]; then BREENIX_QEMU_ACCEL=kvm; else BREENIX_QEMU_ACCEL=tcg; fi
fi
if [ -z "${BREENIX_QEMU_CPU:-}" ]; then
  if [ "$BREENIX_QEMU_ACCEL" = "kvm" ]; then BREENIX_QEMU_CPU=host; else BREENIX_QEMU_CPU=qemu64; fi
fi
export BREENIX_QEMU_ACCEL BREENIX_QEMU_CPU
echo "[gate] accel=$BREENIX_QEMU_ACCEL cpu=$BREENIX_QEMU_CPU"

if [ -n "${BREENIX_RUST_FORK:-}" ]; then
  echo "[gate] repointing rust-fork at $BREENIX_RUST_FORK (not committed)"
  rm -f rust-fork
  ln -s "$BREENIX_RUST_FORK" rust-fork
fi

case "$MODE" in
  full)
    FEATURES="testing,external_test_bins"
    MARKER_GREP='USERSPACE TEST COMPLETE'
    ;;
  kthread|*)
    MODE="kthread"
    FEATURES="kthread_test_only"
    MARKER_GREP='KTHREAD_TEST_ONLY_COMPLETE'
    ;;
esac

echo "[gate] === Building userspace ELFs ==="
if ! ./userspace/programs/build.sh > /tmp/gate-userspace-build.log 2>&1; then
  echo "GATE: FAIL (userspace build failed) - see /tmp/gate-userspace-build.log"; exit 1
fi

# #564: repack every run. The ELF build above does NOT touch the images the
# kernel actually boots from.
echo "[gate] === Repacking the userspace test disk and the ext2 image ==="
rm -f target/test_binaries.img
if ! cargo run -p xtask -- create-test-disk > /tmp/gate-test-disk.log 2>&1; then
  echo "GATE: FAIL (create-test-disk failed) - see /tmp/gate-test-disk.log"; exit 1
fi
rm -f target/ext2.img
if ! ./scripts/create_ext2_disk.sh > /tmp/gate-ext2-disk.log 2>&1; then
  echo "GATE: FAIL (ext2 disk creation failed) - see /tmp/gate-ext2-disk.log"; exit 1
fi

echo "[gate] === Building (release, features=$FEATURES) ==="
BUILD_START=$SECONDS
if ! cargo build --release --features "$FEATURES" --bin qemu-uefi > /tmp/gate-build.log 2>&1; then
  echo "GATE: FAIL (build failed) - see /tmp/gate-build.log"
  tail -40 /tmp/gate-build.log
  exit 1
fi
if grep -qE "^(warning|error)" /tmp/gate-build.log; then
  echo "GATE: FAIL (build produced warnings/errors) - see /tmp/gate-build.log"
  grep -E "^(warning|error)" /tmp/gate-build.log
  exit 1
fi
BUILD_SECS=$((SECONDS - BUILD_START))
echo "[gate] Build clean (0 warnings) in ${BUILD_SECS}s"

echo "[gate] === Running $COUNT boot test(s), mode=$MODE ==="
# Sequential, not wall-clock-parallel: the qemu-uefi binary opens the shared
# breenix-uefi.img read-write, so simultaneous instances collide on QEMU's image
# write lock. Back-to-back runs still exercise N independent boots.
PASS=0
FAIL=0
BOOT_START=$SECONDS
for i in $(seq 1 "$COUNT"); do
  OUTDIR="/tmp/breenix_gate_$i"
  rm -rf "$OUTDIR"; mkdir -p "$OUTDIR"
  # BREENIX_NET_MODE=none: the qemu-uefi binary hardcodes a SLIRP hostfwd on
  # host port 2323; disabling networking avoids lingering port state between
  # runs and is not needed for these boot markers.
  BREENIX_NET_MODE=none timeout "$TIMEOUT_SECS" ./target/release/qemu-uefi \
    -serial file:"$OUTDIR/serial_user.log" \
    -serial file:"$OUTDIR/serial_kernel.log" \
    > "$OUTDIR/stdout.log" 2>&1
  if [ "$MODE" = "full" ]; then
    # EXPECTED_EXITS is mandatory for the verdict script; 10 is the count for
    # this profile's userspace program set.
    if EXPECTED_EXITS="${BREENIX_EXPECTED_EXITS:-10}" \
        "$REPO_DIR/scripts/x86-gate-verdict.sh" \
        "$OUTDIR/serial_user.log" "$OUTDIR/serial_kernel.log"; then
      echo "  Test $i: PASS"
      PASS=$((PASS+1))
    else
      echo "  Test $i: FAIL (see $OUTDIR/serial_kernel.log)"
      FAIL=$((FAIL+1))
    fi
  elif grep -q "$MARKER_GREP" "$OUTDIR/serial_kernel.log" "$OUTDIR/serial_user.log" 2>/dev/null; then
    echo "  Test $i: PASS"
    PASS=$((PASS+1))
  else
    echo "  Test $i: FAIL (marker '$MARKER_GREP' not found; see $OUTDIR/serial_kernel.log)"
    FAIL=$((FAIL+1))
  fi
done
BOOT_SECS=$((SECONDS - BOOT_START))
TOTAL_SECS=$((SECONDS - TOTAL_START))

if [ "$FAIL" -eq 0 ]; then
  echo "GATE: PASS ($PASS/$COUNT boot tests passed; mode=$MODE build=${BUILD_SECS}s boot=${BOOT_SECS}s total=${TOTAL_SECS}s)"
  exit 0
else
  echo "GATE: FAIL ($PASS/$COUNT passed, $FAIL/$COUNT failed; mode=$MODE build=${BUILD_SECS}s boot=${BOOT_SECS}s total=${TOTAL_SECS}s)"
  exit 1
fi
