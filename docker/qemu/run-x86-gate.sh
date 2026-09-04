#!/bin/bash
#
# x86_64 build + boot gate, in the repository.
#
# This is the gate that guards merges on the beast x86 VM. It used to exist only
# as a hand-maintained `/root/run-x86-gate.sh` on that VM, which is #564: every
# hardening applied to it was one re-provision away from being lost, and two of
# its properties lived nowhere else. Both are now versioned here:
# claim-lint:ok: #564 records the gate migration and stale-image failure.
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
# claim-lint:ok: #564 records the separate build and packing steps.
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
  # claim-lint:ok: src/bin/qemu-uefi.rs resolves the hostfwd source.
  BREENIX_NET_MODE=none timeout "$TIMEOUT_SECS" ./target/release/qemu-uefi \
    -serial file:"$OUTDIR/serial_user.log" \
    -serial file:"$OUTDIR/serial_kernel.log" \
    > "$OUTDIR/stdout.log" 2>&1

  # Device-enumeration census leg (green arc 5, bus+NIC blended). Neither
  # branch below proves pci::enumerate() found the device set this boot
  # actually attached -- #702 is a silent hang inside PCI enumeration right
  # after "E1000 network device found", and every check below reads that
  # failure only as "marker not found" / "USERSPACE TEST COMPLETE was
  # absent", with no signal naming where the boot actually stopped. This
  # makes that region legible without a new QEMU invocation: the census
  # line's mere absence is itself signal, and its VirtIO-block count is
  # checked against what this binary itself attaches, self-counted from
  # src/bin/qemu-uefi.rs rather than a second hand-pinned literal here (the
  # #549/#551/[[gate-target-fidelity-528]] census-not-literal lesson).
  # BREENIX_NET_MODE=none only skips the explicit -netdev/-device args in
  # qemu-uefi.rs; it never passes QEMU its own `-nic none`. QEMU 8.2 (the
  # beast host's version) auto-attaches its own default e1000 NIC whenever
  # no -net/-netdev/-nic option is given at all, so a real NIC (00:02.0
  # [8086:100e]) IS present on every boot of this gate -- confirmed
  # empirically (a boot here reports "PCI: ... Found 9 devices (3 VirtIO
  # block, 1 network)" and "E1000 network device found"), not assumed from
  # reading qemu-uefi.rs alone. The honest expected floor is therefore >=1.
  # claim-lint:ok: #702 records the silent enumeration failure and gate requirement.
  census_ok=true
  census_reason=""
  # Anchored to the emitted -device arg form (leading whitespace then the
  # opening quote), not a bare substring match: an unanchored grep for the
  # literal text can equally match a future comment or doc string that
  # merely mentions the flag, permanently inflating the count and
  # permanently reddening this gate (review finding F9 -- the same
  # self-referential-vacuity class the aarch64 leg and run-x86-boot-tests.sh
  # each hit once already, hardened here before it was hit a third time).
  # All three sites are conditional on BREENIX_QEMU_STORAGE: this gate never
  # sets it, so storage_mode defaults to "virtio" and all three attach --
  # BREENIX_QEMU_STORAGE=ide (used elsewhere, e.g. CI's OVMF-discovery
  # profile) would attach zero and make this expected count wrong for that
  # profile; it is not read by this script today.
  # claim-lint:ok: #702 and src/bin/qemu-uefi.rs resolve the attachment count.
  expected_virtio_block=$(grep -cE -- '^[[:space:]]*"virtio-blk-pci,drive=' "$REPO_DIR/src/bin/qemu-uefi.rs")
  pci_census_line=$(grep -h -E 'PCI: Enumeration complete\. Found [0-9]+ devices \([0-9]+ VirtIO block, [0-9]+ network\)' \
      "$OUTDIR"/serial_*.log 2>/dev/null | tail -1)
  if [ -z "$pci_census_line" ]; then
    census_ok=false
    census_reason="device-enumeration census absent -- see kernel/src/drivers/{pci.rs,mod.rs}"
  else
    census_virtio_block=$(printf '%s\n' "$pci_census_line" | \
        sed -n 's/.*Found [0-9]* devices (\([0-9]*\) VirtIO block, [0-9]* network).*/\1/p')
    census_network=$(printf '%s\n' "$pci_census_line" | \
        sed -n 's/.*Found [0-9]* devices ([0-9]* VirtIO block, \([0-9]*\) network).*/\1/p')
    if [ -z "$census_virtio_block" ] || [ -z "$census_network" ]; then
      census_ok=false
      census_reason="device-enumeration census line malformed: $pci_census_line"
    elif [ "$census_virtio_block" -ne "$expected_virtio_block" ]; then
      census_ok=false
      census_reason="device-enumeration census reports $census_virtio_block VirtIO block device(s), self-counted expected $expected_virtio_block from src/bin/qemu-uefi.rs"
    elif [ "$census_network" -lt 1 ]; then
      census_ok=false
      census_reason="device-enumeration census reports $census_network network device(s); QEMU's implicit default NIC (BREENIX_NET_MODE=none never passes -nic none) should always yield >=1"
    fi
  fi

  # The census is an ADDITIONAL requirement, not a short-circuit (review
  # finding B5). In full mode, x86-gate-verdict.sh runs UNCONDITIONALLY --
  # even when census_ok is already false -- because that script runs the
  # strand census FIRST. A positive latest heartbeat names the stranded
  # thread; an unavailable heartbeat is not strand evidence and the verdict
  # continues to the existing ordered checks. That distinction preserves the
  # reason #702's filing used threads_saved_blocked=0: a silent PCI-enumeration
  # hang must not be folded into the strand family (#695). A short-circuit that
  # skips the verdict script on census failure would remove that distinction
  # from exactly the gate #702 lives on. Both consumers run on every boot,
  # pass or fail.
  # claim-lint:ok: #775 ruling R125 defines positive and unavailable census handling.
  if [ "$MODE" = "full" ]; then
    # EXPECTED_EXITS is mandatory for the verdict script; 10 is the count for
    # this profile's userspace program set.
    if EXPECTED_EXITS="${BREENIX_EXPECTED_EXITS:-10}" \
        "$REPO_DIR/scripts/x86-gate-verdict.sh" \
        "$OUTDIR/serial_user.log" "$OUTDIR/serial_kernel.log"; then
      verdict_ok=true
      verdict_reason=""
    else
      verdict_ok=false
      verdict_reason="see $OUTDIR/serial_kernel.log"
    fi
  elif grep -q "$MARKER_GREP" "$OUTDIR/serial_kernel.log" "$OUTDIR/serial_user.log" 2>/dev/null; then
    verdict_ok=true
    verdict_reason=""
  else
    verdict_ok=false
    verdict_reason="marker '$MARKER_GREP' not found; see $OUTDIR/serial_kernel.log"
  fi

  if [ "$census_ok" = true ] && [ "$verdict_ok" = true ]; then
    echo "  Test $i: PASS"
    echo "  Device census: $pci_census_line"
    PASS=$((PASS+1))
  else
    combined_reason="$verdict_reason"
    if [ "$census_ok" != true ]; then
      if [ -n "$combined_reason" ]; then
        combined_reason="$census_reason; $combined_reason"
      else
        combined_reason="$census_reason"
      fi
    fi
    echo "  Test $i: FAIL ($combined_reason)"
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
