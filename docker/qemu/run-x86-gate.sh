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
#   BREENIX_BOOT_SUITE  an effort-suite id (docs/suites/<id>.json). The gate then
#                       builds the production kernel (no features, whatever the
#                       mode argument says), boots it with /etc/breenix/boot-target
#                       ("suite <id>") written onto a copy of the ext2 disk so the
#                       kernel runs /sbin/suite-<id> as PID 1, stops the VM a few
#                       seconds after the suite's `SUITE <id> DONE` line, and scores
#                       the boot with scripts/suite-verdict.py: one START, every
#                       manifest case's CASE line in order, a DONE agreeing with
#                       them with failed=0, and no fatal kernel output.
#   BREENIX_QMP_SOCKET  if set, QEMU opens a QMP socket at this path (qemu-uefi.rs)
#                       so a screendump can be taken while the VM runs. In suite
#                       mode the gate also saves the final screen as screen.png in
#                       the boot's output directory.
#   BREENIX_GATE_TMP    base dir for boot output (default: /tmp). #797:
#                       concurrent lanes on the shared beast container each
#                       hardcoded /tmp/breenix_gate_$i, so one lane's rm -rf +
#                       mkdir could clobber another lane's in-flight run and
#                       score its serial as its own; a concurrent-lane
#                       launcher sets this to a per-clone directory instead.
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
source "$SCRIPT_DIR/lib/run-inspector-import.sh" || :
BREENIX_RUNS_GATE_ARGV=("$0" "$@")
DEFAULT_REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

COUNT="${1:-1}"
MODE="${2:-kthread}"
MAX_CONCURRENCY=4
REPO_DIR="${BREENIX_REPO_DIR:-$DEFAULT_REPO_DIR}"
TIMEOUT_SECS="${BREENIX_GATE_TIMEOUT:-150}"
PROFILE="${BREENIX_QEMU_PROFILE-default}"
export BREENIX_QEMU_PROFILE="$PROFILE"
echo "[gate] profile=$PROFILE"
BREENIX_GATE_TMP="${BREENIX_GATE_TMP:-/tmp}"
# Must be absolute: a relative value would resolve against whatever
# directory happens to be current when each command runs (review finding F6
# on #797 -- carried here from the same fix in the other four gate scripts).
case "$BREENIX_GATE_TMP" in
    /*) ;;
    *) echo "GATE: FAIL (BREENIX_GATE_TMP must be an absolute path, got: $BREENIX_GATE_TMP)"; exit 1 ;;
esac

# Non-interactive shells don't source .bashrc/.profile, so put cargo on PATH.
source "$HOME/.cargo/env" 2>/dev/null || export PATH="$HOME/.cargo/bin:$PATH"

if [ "$COUNT" -gt "$MAX_CONCURRENCY" ]; then
  echo "[gate] Capping concurrency at $MAX_CONCURRENCY (requested $COUNT)"
  COUNT=$MAX_CONCURRENCY
fi

cd "$REPO_DIR" || { echo "GATE: FAIL (repo dir missing: $REPO_DIR)"; exit 1; }

# Validate against the catalog before any build or disk packing. An explicitly
# empty name is invalid, while an unset variable selects default.
if ! python3 - "$PROFILE" "$REPO_DIR/docs/x86-profiles.json" <<'PYPROFILE'
import json, sys
names = [row["name"] for row in json.load(open(sys.argv[2]))]
if sys.argv[1] not in names:
    print("GATE: FAIL (invalid QEMU profile %r; valid profiles: %s)" %
          (sys.argv[1], ", ".join(names)))
    sys.exit(1)
PYPROFILE
then
  exit 1
fi
# This gate repacks and requires all three disks; IDE skips the test disks.
if [ "${BREENIX_QEMU_STORAGE:-virtio}" != virtio ]; then
  echo "GATE: FAIL (this gate requires BREENIX_QEMU_STORAGE=virtio)"; exit 1
fi
export BREENIX_QEMU_STORAGE=virtio

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

# Suite mode: the production kernel (no features) runs /sbin/suite-<id> as PID 1.
SUITE="${BREENIX_BOOT_SUITE:-}"
if [ -n "$SUITE" ]; then
  if [[ ! "$SUITE" =~ ^[a-z0-9]+(-[a-z0-9]+)*$ ]]; then
    echo "GATE: FAIL (BREENIX_BOOT_SUITE is not a suite id: $SUITE)"; exit 1
  fi
  if [ ! -f "docs/suites/$SUITE.json" ]; then
    echo "GATE: FAIL (no manifest docs/suites/$SUITE.json)"; exit 1
  fi
  MODE="suite"
  FEATURES=""
  echo "[gate] suite=$SUITE (production kernel, /sbin/suite-$SUITE as PID 1)"
fi
# Seconds the VM stays up after the suite's DONE line, so its final screen can be captured.
SUITE_HOLD_SECS="${BREENIX_SUITE_HOLD:-5}"

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
if [ -n "$SUITE" ]; then
  if [ ! -f "userspace/programs/suite-$SUITE.elf" ]; then
    echo "GATE: FAIL (no suite binary userspace/programs/suite-$SUITE.elf; is suite-$SUITE in userspace/programs/build.sh?)"; exit 1
  fi
  # The boot target goes on a copy: qemu-uefi.rs copies BREENIX_EXT2_SOURCE (default
  # testdata/ext2.img) to target/ext2.img for each boot, and testdata/ext2.img stays clean.
  # write-boot-target.sh also checks that the copy's /sbin/suite-$SUITE is the binary
  # just built, so a stale or partial copy is never booted.
  rm -f target/ext2-boot-target.img
  if ! cp testdata/ext2.img target/ext2-boot-target.img; then
    echo "GATE: FAIL (could not copy testdata/ext2.img for the boot target)"; exit 1
  fi
  if ! ./scripts/write-boot-target.sh target/ext2-boot-target.img "$SUITE" "userspace/programs/suite-$SUITE.elf"; then
    echo "GATE: FAIL (could not write the boot target onto the ext2 disk)"; exit 1
  fi
  export BREENIX_EXT2_SOURCE="$REPO_DIR/target/ext2-boot-target.img"
fi

echo "[gate] === Building (release, features=${FEATURES:-none}) ==="
BUILD_START=$SECONDS
FEATURE_ARGS=()
[ -n "$FEATURES" ] && FEATURE_ARGS=(--features "$FEATURES")
if ! cargo build --release ${FEATURE_ARGS[@]+"${FEATURE_ARGS[@]}"} --bin qemu-uefi > /tmp/gate-build.log 2>&1; then
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

# Ask the launcher for the selected hardware's census, rather than counting
# source strings (AHCI/NVMe attach no VirtIO block devices).
if ! expected_census=$(BREENIX_PRINT_QEMU_CENSUS=1 ./target/release/qemu-uefi); then
  echo "GATE: FAIL (invalid QEMU profile/storage configuration)"; exit 1
fi
if [[ ! "$expected_census" =~ ^[0-9]+\ [0-9]+$ ]]; then
  echo "GATE: FAIL (malformed QEMU census: $expected_census)"; exit 1
fi
read -r expected_virtio_block expected_network <<< "$expected_census"
if ! expected_pci=$(BREENIX_PRINT_QEMU_PCI=1 ./target/release/qemu-uefi) || [ -z "$expected_pci" ]; then
  echo "GATE: FAIL (missing QEMU PCI requirements)"; exit 1
fi
while read -r pci_id pci_count; do
  if [[ ! "$pci_id" =~ ^[0-9a-f]{4}:[0-9a-f]{4}$ ]] || [[ ! "$pci_count" =~ ^[1-9][0-9]*$ ]]; then
    echo "GATE: FAIL (malformed QEMU PCI requirement: $pci_id $pci_count)"; exit 1
  fi
done <<< "$expected_pci"

echo "[gate] === Running $COUNT boot test(s), mode=$MODE ==="
# Sequential, not wall-clock-parallel: the qemu-uefi binary opens the shared
# breenix-uefi.img read-write, so simultaneous instances collide on QEMU's image
# write lock. Back-to-back runs still exercise N independent boots.
PASS=0
FAIL=0
BOOT_START=$SECONDS
for i in $(seq 1 "$COUNT"); do
  OUTDIR="$BREENIX_GATE_TMP/breenix_gate_$i"
  rm -rf "$OUTDIR"; mkdir -p "$OUTDIR"
  # BREENIX_NET_MODE=none: the qemu-uefi binary hardcodes a SLIRP hostfwd on
  # host port 2323; disabling networking avoids lingering port state between
  # runs and is not needed for these boot markers.
  # claim-lint:ok: src/bin/qemu-uefi.rs resolves the hostfwd source.
  INSPECTOR_START_MS="$(date +%s)000" || INSPECTOR_START_MS=""
  BREENIX_NET_MODE=none timeout "$TIMEOUT_SECS" ./target/release/qemu-uefi \
    -serial file:"$OUTDIR/serial_user.log" \
    -serial file:"$OUTDIR/serial_kernel.log" \
    > "$OUTDIR/stdout.log" 2>&1 &
  QEMU_TIMEOUT_PID=$!
  if [ -n "$SUITE" ]; then
    # A suite never exits (it is PID 1 and idles with its final panel up): stop the VM
    # once its DONE line is out, after saving the screen and holding it briefly. Only a
    # whole DONE line on the suite's own serial (COM1) counts.
    done_shape="^SUITE $SUITE DONE passed=[0-9]+ failed=[0-9]+ skipped=[0-9]+ total=[0-9]+\$"
    while kill -0 "$QEMU_TIMEOUT_PID" 2>/dev/null; do
      if tr -d '\r' < "$OUTDIR/serial_user.log" 2>/dev/null | grep -qE "$done_shape"; then
        sleep 2
        if [ -n "${BREENIX_QMP_SOCKET:-}" ] && \
            python3 "$REPO_DIR/scripts/qmp-screendump.py" "$BREENIX_QMP_SOCKET" "$OUTDIR/screen.png" >/dev/null 2>&1; then
          echo "  Final screen: $OUTDIR/screen.png"
        fi
        sleep "$SUITE_HOLD_SECS"
        # timeout forwards TERM to its process group: qemu-uefi and QEMU itself.
        kill -TERM "$QEMU_TIMEOUT_PID" 2>/dev/null
        break
      fi
      sleep 1
    done
  fi
  wait "$QEMU_TIMEOUT_PID"

  # Require enumeration to finish and match the selected profile's block
  # count and network floor. Default still requires 3 VirtIO block and >=1 NIC.
  census_ok=true
  census_reason=""
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
      census_reason="device-enumeration census reports $census_virtio_block VirtIO block device(s), expected $expected_virtio_block for profile=$PROFILE"
    elif [ "$census_network" -lt "$expected_network" ]; then
      census_ok=false
      census_reason="device-enumeration census reports $census_network network device(s); profile=$PROFILE requires >=$expected_network"
    fi
  fi

  # Require the profile's actual controllers and NIC, independently of drivers.
  while read -r pci_id pci_count; do
    observed=$(grep -h -E "^PCI_FN [0-9a-f]{2}:[0-9a-f]{2}\\.[0-7] $pci_id " \
        "$OUTDIR"/serial_*.log 2>/dev/null | wc -l)
    if [ "$observed" -ne "$pci_count" ]; then
      census_ok=false
      census_reason="${census_reason:+$census_reason; }PCI $pci_id count=$observed expected=$pci_count for profile=$PROFILE"
    fi
  done <<< "$expected_pci"

  # The census is an ADDITIONAL requirement, not a short-circuit (review
  # finding B5). In full mode, x86-gate-verdict.sh runs UNCONDITIONALLY --
  # even when census_ok is already false -- because that script runs the
  # strand census FIRST. A snapshot that still lists a saved-blocked thread
  # names it; an unavailable snapshot, and an overflowed ledger, are not strand
  # evidence in either direction and the verdict continues to the existing
  # ordered checks. That distinction preserves the reason #702's filing used
  # threads_saved_blocked=0: a silent PCI-enumeration hang must not be folded
  # into the strand family (#695). A short-circuit that skips the verdict script
  # on census failure would remove that distinction from exactly the gate #702
  # lives on. Both consumers run on every boot, pass or fail.
  # claim-lint:ok: #775 ruling R134 items 1-2; the boots behind the census and
  # overflow arms are in docs/planning/green-program/sockets/775-CENSUS-EQUIVALENCE-2026-09-04.md
  #
  # The snapshots are on COM2, so serial_kernel.log below is the argument that
  # actually carries them (kernel/src/task/dispatch_strand_census.rs). The COM1
  # capture is passed as well because the rest of this script's checks read it,
  # and the census reads whichever argument the markers are in -- it selects by
  # snapshot `seq`, not by argument order.
  # claim-lint:ok: #775 ruling R134 defines the census input contract.
  if [ -n "$SUITE" ]; then
    suite_verdict=$(python3 "$REPO_DIR/scripts/suite-verdict.py" "docs/suites/$SUITE.json" \
        "$OUTDIR/serial_user.log" "$OUTDIR/serial_kernel.log" 2>&1)
    if [ $? -eq 0 ]; then
      verdict_ok=true
      verdict_reason=""
      echo "  ${suite_verdict#PASS: }"
    else
      verdict_ok=false
      verdict_reason="${suite_verdict#FAIL: } (see $OUTDIR/serial_user.log)"
    fi
  elif [ "$MODE" = "full" ]; then
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
    INSPECTOR_VERDICT=PASS
    INSPECTOR_STATUS=0
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
    INSPECTOR_VERDICT=FAIL
    INSPECTOR_STATUS=1
  fi
  breenix_runs_import_nonfatal "$OUTDIR" x86_64 gate "$INSPECTOR_VERDICT" "$INSPECTOR_STATUS" "$INSPECTOR_START_MS" "${BREENIX_RUNS_GATE_ARGV[@]}" || :
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
