#!/bin/bash
#
# #728 ext2 lock-discipline gate.
#
# Boots a kernel built with `--features boot_tests,ext2_lock_race`, whose
# in-kernel leg (kernel/src/fs/ext2_lock_race.rs) deterministically
# constructs the #728 shape for BOTH mounted filesystems: a "holder" kthread
# acquires root_fs_read()/home_fs_read() and deliberately parks *while still
# holding the guard* (a scratch Completion that is never completed, so the
# wait always runs its full three-second deadline — no real device I/O or
# fault injection needed to force this), while contender kthreads
# concurrently attempt root_fs_write()/home_fs_write() and mkdir on success.
#
# Unfixed ext2 lock code cannot survive this: a contended acquisition
# busy-spins with preemption disabled, denying the CPU the timer ISR would
# otherwise use to dispatch the holder's own completion once it fires --
# hence the kernel's own EXT2_LOCK_SPIN_STALL marker
# (kernel/src/fs/ext2/mod.rs) and, downstream, its own soft-lockup detector.
# Fixed code parks contenders instead, resolves the race, and the leg prints
# a verdict line for each filesystem plus one COMPLETE tally.
#
# ---------------------------------------------------------------------------
# Why the verdict is read two ways
# ---------------------------------------------------------------------------
# On the pathological case this leg constructs, EVERY CPU ends up occupied by
# a non-yielding contender -- including the CPU running this leg's own
# driver thread, which is itself blocked in kthread_join() waiting for the
# holder. Nothing is left to print a "the test hung" verdict; the boot
# simply goes silent. So the RED signal is NOT "COMPLETE never printed" (a
# raw hang is also what "slow" looks like) -- it is the presence of
# EXT2_LOCK_SPIN_STALL, which fires from *inside* the still-executing spin
# itself, or the kernel's own soft-lockup detector, either of which the boot
# can produce even while otherwise wedged. A green run requires ALL of:
# no stall marker, no soft lockup, the leg's own COMPLETE tally with
# fail=0, and (non-anti-vacuity runs) the boot's normal liveness markers
# after it.
#
# ---------------------------------------------------------------------------
# Anti-vacuity
# ---------------------------------------------------------------------------
# The oracle's own red/green split was proven by hand across this fix's
# commits, not re-derived by this script every run (a script that reverted
# kernel source on every invocation would be its own hazard). The record, as
# actually observed (not aspired to) at the time this header was last edited:
#   - Observer-only commit (spin instrumented, no park path) + this same
#     harness, as actually archived (review round-2 finding B3 -- corrected
#     here from an earlier version of this header that overstated both of
#     the following): EXT2_LOCK_SPIN_STALL fires -- x3 on aarch64
#     (`728-prove-round2/aarch64-oracle/red-serial.txt`), x1 on x86
#     (`728-prove/x86-oracle/red-serial-all.txt`) -- and in every archived
#     capture, both arches, the lock name printed is `ROOT_EXT2_write` only
#     (the leg wedges acquiring ROOT before HOME ever runs, so "BOTH
#     filesystems" is not something either archive shows). The aarch64
#     capture's serial log ends immediately after its third stall line with
#     no further output; the x86 capture's last line is its single stall.
#     Neither archived capture contains a "soft lockup detected" line or any
#     other output after the stall(s) -- the kernel's own soft-lockup
#     detector firing is NOT something either archive demonstrates. (It may
#     still fire in practice past the archived window; it just isn't
#     evidence on file, so it is not asserted here.)
#   - The fix commit + the identical harness, aarch64: verdict=PASS for both
#     filesystems, COMPLETE:pass=2:fail=0 (both disks attached), a nonzero
#     EXT2_LOCK_PARKS delta on both races (not merely an absence of stall --
#     the fix's own park path is entered at least once during the window;
#     the counter is global, not per-race, so this is strong corroboration
#     on this leg's dedicated profile rather than a per-thread proof --
#     review round-2 finding M5), and the boot continues live on a line
#     genuinely after COMPLETE. Reconfirmed on two independent reruns in fix
#     round 2.
#   - The fix commit + the identical harness, x86: NOT CAPTURED as a
#     COMPLETE/verdict line, on either fix round. Every GREEN attempt reaches
#     the leg (holder + contender kthreads spawned, actively scheduled back
#     and forth for as long as observed, zero EXT2_LOCK_SPIN_STALL) but none
#     has reached `[LOCKRACE:COMPLETE:...]` within the time budget spent.
#     Fix round 2 investigated this specifically (review finding B2) rather
#     than re-asserting round 1's framing: the round-1 explanation ("x86's
#     testing-profile boot sits behind a slow pre-existing boot_tests
#     battery") only accounts for the time *before* the leg's own call site;
#     it does not explain why the leg itself, once reached, stays at
#     "actively scheduling, zero stalls" for many further minutes without a
#     verdict. Round 2's own dedicated-clone run on beast measured the
#     line-production rate directly *inside* the leg (holder/contender
#     threads 1194/1195 already spawned and switching): roughly one kernel-
#     log line per 12-13 seconds of wall clock, sustained across multiple
#     independently-sampled windows, while `uptime` on the physical beast
#     host showed a load average of 21-29 for the entire observation window
#     from unrelated tenants (one single non-breenix process alone was
#     measured at ~960% CPU with ~6 weeks 6 days of accumulated runtime).
#     Also established this round: `ext2_lock_race.rs`'s holder/contender
#     kthreads run with IF=1 (kthread_entry() calls arch_enable_interrupts()
#     before the thread body runs, task/kthread.rs:366-371), so this specific
#     harness was never gated by review finding B1's interrupts_enabled()
#     defect on x86 either before or after B1's fix -- B1 only mattered for
#     real syscall callers (matching #728's own sys_mkdir repro, exercised
#     instead by leg 2 / run-boot-parallel.sh). Taken together: the evidence
#     points at severe host-contention-driven wall-clock slowdown of a
#     verbose, high-frequency debug-logging boot profile under nested
#     virtualization (matching this project's own prior finding that KVM
#     acceleration "did not materially change the pace" -- the bottleneck is
#     not instruction-emulation speed), not a logic defect in the shared
#     ext2_acquire()/ext2_acquire_write() code path the aarch64 run above
#     already proves parks and resolves correctly. This is reported as an
#     explained, still-not-captured x86 GREEN -- not as a pass record, and
#     not waved off as "probably fine" either. See
#     docs/planning/green-program/nic-bus/serials/728-prove/x86-oracle/ for
#     round 1's artifacts and this fix round's PR/issue history for round 2's.
# Reproduce by hand: `git checkout <harness-fix commit> -- kernel/src/fs`
# reverts only the lock-discipline commit while keeping this same harness,
# rebuild, rerun this script -- it must redden the same way.
#
# ---------------------------------------------------------------------------
# Usage
# ---------------------------------------------------------------------------
#   docker/qemu/run-ext2-lock-race-gate.sh                # aarch64 (default)
#   docker/qemu/run-ext2-lock-race-gate.sh --x86           # x86_64 (beast)
#   docker/qemu/run-ext2-lock-race-gate.sh --no-build       # reuse the built kernel
#   docker/qemu/run-ext2-lock-race-gate.sh --x86 --park-only  # #748, see below
#
# x86's full `testing` profile runs the same ~10+ minute userspace/teardown
# suite every other x86 boot_tests gate sits behind before reaching this
# leg's own call site; X86_BOOT_TIMEOUT defaults to 1800s to give it room
# (X86_POLL_BOUND tracks half of it by default -- see the poll loop).
#
# ---------------------------------------------------------------------------
# --park-only (#748): a second-best oracle when the full leg won't capture
# ---------------------------------------------------------------------------
# #748 found the x86 leg's own in-leg pace pathological (~11-12s per printed
# kernel log line once the holder/contender kthreads are running, ~145-370x
# slower than the identical boot's own pre-leg pace, measured on a confirmed-
# quiet host) -- multiple independent x86 attempts at the FULL leg above
# (COMPLETE + both filesystems' verdicts + post-COMPLETE liveness) have
# never reached a verdict within any budget tried, on either arch's fixed or
# reverted-defect code (see #748's own issue body and the header above).
#
# `--park-only` does not wait for the full leg. It watches for exactly one
# leg-scoped fact -- kernel/src/fs/ext2/mod.rs's `EXT2_LOCK_PARK_FIRST`
# marker, printed by `ext2_record_park()` the instant (not after any
# timeout elapses) a contended ext2 acquisition is enqueued via
# `prepare_to_wait_checked`'s `Queued` outcome -- and exits the moment it,
# or a spin stall from the same construction, appears (see the "stall takes
# precedence over park" note at the verdict block below -- the two are NOT
# mutually exclusive; `ext2_acquire`/`ext2_acquire_write` park for up to
# `EXT2_LOCK_PARK_ROUNDS` rounds and then fall through to the spin path by
# construction, so a stall can legitimately follow a park in the same leg),
# without waiting for either race to resolve, both filesystems to be
# attempted, or the leg's own `kthread_join()`-gated COMPLETE line.
#
# "Leg-scoped" is defense-in-depth, not a fix for an observed miss:
# `EXT2_LOCK_PARK_FIRST` is a one-shot, whole-boot marker, and
# `ext2_lock_race`'s own `boot_tests` profile runs a long battery of other
# tests before this leg's own call site. If any of them ever incidentally
# contends the ext2 lock, it would consume the boot's one-and-only marker
# before the leg's own holder/contender threads run, so a naive "did
# EXT2_LOCK_PARK_FIRST appear anywhere" check would report PARK OBSERVED
# for a park that had nothing to do with this leg's own #728 repro shape.
# (No such pre-leg park has actually happened in either recorded x86 run to
# date -- `run_x86_tombstone_join_gate()` is the literal last call in
# `main.rs`'s `boot_tests` battery, immediately preceding this leg's own
# entry point, so there is no unrelated test between them that could park
# first. A prior draft of this comment and fix-notes.md claimed an
# unrelated tombstone-join park had been observed; that was checked against
# `main.rs`'s call ordering and was wrong -- see the PR body for the
# correction. The mechanism below is kept because it is unconditionally
# correct and the battery or its ordering could change.)
# `ext2_reset_lock_park_first_marker()`
# (`kernel/src/fs/ext2/mod.rs`) fixes the *source*: `run_one()`
# (`ext2_lock_race.rs`) re-arms the marker immediately before spawning each
# race attempt's own holder/contender, so a park during THAT race prints
# again regardless of what consumed the boot's first-ever one. This script
# fixes the *observer* side to match: LEG_ENTRY_SEEN/PARK_BASELINE/
# LEG_PARK_SEEN below track the EXT2_LOCK_PARK_FIRST occurrence *count*
# from the poll immediately before the leg's own "Added thread ...
# 'lockrace_holder'" spawn line first appears, and only declare PARK
# OBSERVED once that count rises above the pre-leg baseline (see the
# PREV_PARK_COUNT comment at the poll loop for why it's the *prior* tick's
# count, not the detecting tick's own). This also caught and fixed a latent,
# pre-existing bug in `check_live_after_complete()` above: `[LOCKRACE:...]`
# lines print via `crate::serial_println!` (COM1, `serial_user.txt` on
# x86) while the x86 `LIVENESS_PATTERN` (`log::info!`) and `PRIMARY_LOG`
# (`serial_kernel.txt`) are COM2 -- two files with independently-numbered,
# un-timestamped lines that a single-file "position-aware" check can never
# correctly order on x86. That bug is disclosed, not fixed, here: it has
# never yet been exercised (x86 has never reached COMPLETE at all), fixing
# it needs either cross-port timestamps or a same-port liveness marker this
# round could not validate without the very multi-hour capture #748 exists
# to route around, and `--park-only` does not depend on it (see
# `count_marker()`'s comment for why its own delta tracking sidesteps the
# same hazard entirely). See #748's fix-notes.md for the full account.
#
# The marker is a companion to (not a replacement for) `EXT2_LOCK_PARKS`
# (`ext2_lock_parks()`); ratcheted by `tests/ext2_lock_structure.rs`
# properties 6 and 9 (per-function routing so neither `ext2_acquire` nor
# `ext2_acquire_write` can silently lose it).
#
# Three distinct, honestly-labeled outcomes (not park-only's own pass/fail
# collapsed into two). A stall takes precedence over a park whenever both
# are seen, matching the full gate's own precedence (stalls are checked
# before COMPLETE there too) -- see POST_PARK_GRACE below for how a stall
# arriving shortly after a park is still caught:
#   PARK OBSERVED  -- EXT2_LOCK_PARK_FIRST fired and no stall followed it
#                      within POST_PARK_GRACE poll ticks: x86 DOES enter the
#                      park path in this construction. The positive #728
#                      fact this probe exists to obtain. This is NOT
#                      equivalent to the leg's own in-kernel PASS verdict --
#                      it only proves entry, and it cannot see a stall that
#                      arrives after its bounded grace window (the leg's own
#                      pace is the pathology #748 is about, so a later stall
#                      is not a remote possibility this probe can rule out).
#   SPIN, NO PARK  -- EXT2_LOCK_SPIN_STALL (or a soft-lockup) fired, whether
#                      or not a park was also recorded (a stall AFTER a park
#                      is the code's own park-then-fall-through-to-spin
#                      structure, not a surprise -- see `ext2_acquire`): the
#                      contender took the spin fallback -- a real, different,
#                      also-actionable fact (`ext2_lock_can_sleep()` false in
#                      this exact call context, the park path timing out, or
#                      the park path not being reached), not "the probe
#                      failed."
#   INCONCLUSIVE   -- neither fired within the probe's timeout. This proves
#                      nothing either way and must never be reported as
#                      either of the above.
# Still governed by the same panic/CPU-exception aborts as the full gate.

set -euo pipefail
set -E

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
# #826/R181: serializes qemu-system-aarch64 boots host-wide (aarch64 leg
# only -- see the ARCH branch below; the x86 leg is unchanged, beast has no
# equivalent contention and is out of this lane's scope).
# shellcheck source=lib/qemu-host-lock.sh
source "$SCRIPT_DIR/lib/qemu-host-lock.sh"

# #797: concurrent lanes sharing one host (e.g. the beast Incus container,
# reached here via --x86) each invoking this script hardcode the identical
# /tmp/breenix_ext2_lock_race_gate_$ARCH path, so one lane's rm -rf/mkdir can
# clobber another lane's in-flight run. Defaulting to /tmp keeps every
# existing caller byte-identical; a concurrent-lane launcher sets this to a
# per-clone directory instead. Must be absolute -- a relative value would
# resolve against whatever directory happens to be current when each command
# runs, and the ERR trap below can fire before OUTPUT_DIR is even assigned
# its real value (review finding F6 on #797).
# claim-lint:ok: #797, diff-empty against origin/main once substituted -- see
# docs/planning/green-program/gates/GATE-TMP-BASEDIR-2026-09-05.md
# The VALUE is not judged here -- that check is spent in the BASE-DIR
# PREFLIGHT block right after the ERR trap is installed below (#802/#805
# idiom, widened to this gate: a bare `exit` here would run before the trap
# exists and could end the gate with no verdict line at all).
BREENIX_GATE_TMP="${BREENIX_GATE_TMP:-/tmp}"

OUTPUT_DIR=""
report_gate_failure() {
    local exit_code=$?
    local line_no="$1"
    local failing_cmd="$2"
    echo "ext2 lock-race gate: FAIL (set -e abort at ${BASH_SOURCE[0]}:${line_no}, exit ${exit_code})"
    echo "  failing command: ${failing_cmd}"
    if [ -n "$OUTPUT_DIR" ] && compgen -G "$OUTPUT_DIR/serial*.txt" >/dev/null 2>&1; then
        echo "--- serial tail (last 120 lines per file, $OUTPUT_DIR) ---"
        tail -n 120 "$OUTPUT_DIR"/serial*.txt
    fi
    exit "$exit_code"
}
trap 'report_gate_failure "$LINENO" "$BASH_COMMAND"' ERR

# --- BASE-DIR PREFLIGHT (#797 F6, routed through the verdict path by the
# #802/#805 idiom widened to this gate) ---
case "$BREENIX_GATE_TMP" in
    /*) ;;
    *) echo "ext2 lock-race gate preflight: BREENIX_GATE_TMP must be an absolute path, got: $BREENIX_GATE_TMP" >&2
       false ;;
esac

# #748 F10: --park-only's own verdict carries three distinct process exit
# codes (0 PARK OBSERVED, 1 SPIN/NO PARK, 2 INCONCLUSIVE), deliberately kept
# apart from a plain gate FAIL and from each other so a caller that ever
# checks $? cannot collide two different facts onto the same code. `redden`
# is how a non-1 code still routes through the ERR trap above instead of a
# bare `exit`: a `return` inside a function is an ordinary command as far as
# `set -e`/`errtrace` are concerned (verified: it fires the trap with $?
# equal to the returned code, the same way a failing built-in would), so
# report_gate_failure's `exit "$exit_code"` re-raise still ends up carrying
# the intended code, and the trap's own re-raise stays the only literal
# `exit` statement in this script.
redden() {
    return "$1"
}

ARCH="aarch64"
BUILD=1
PARK_ONLY=0
while [ $# -gt 0 ]; do
    case "$1" in
        --x86|--x86_64) ARCH="x86"; shift ;;
        --aarch64) ARCH="aarch64"; shift ;;
        --no-build) BUILD=0; shift ;;
        --park-only) PARK_ONLY=1; shift ;;
        # #748 F10: a distinct exit code from --park-only's own INCONCLUSIVE
        # (2) -- nothing parses either today, but if this script is ever
        # wired into a caller that checks $?, a usage error and an honest
        # "proves nothing" verdict must never collide on the same code.
        *) echo "unknown argument: $1" >&2; redden 64 ;;
    esac
done

FEATURES="boot_tests,ext2_lock_race"

cd "$BREENIX_ROOT"

echo "========================================="
echo "#728 ext2 lock-race gate"
echo "  arch:     $ARCH"
echo "  features: $FEATURES"
echo "========================================="

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
BUILD_LOG="/tmp/ext2-lock-race-gate-build.log"
if [ "$ARCH" = "aarch64" ]; then
    KERNEL="$BREENIX_ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
    if [ "$BUILD" -eq 1 ]; then
        echo "[gate] building aarch64 kernel..."
        # The soft-float kernel target is mandatory; building the NEON target
        # here would re-arm #528 (see scripts/check-kernel-no-neon.sh).
        cargo build --release --features "$FEATURES" \
            --target aarch64-breenix-kernel.json \
            -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
            -p kernel --bin kernel-aarch64 >"$BUILD_LOG" 2>&1
    fi
    test -f "$KERNEL"
    "$BREENIX_ROOT/scripts/check-kernel-no-neon.sh" "$KERNEL" >/dev/null
    EXT2_ROOT_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
    test -f "$EXT2_ROOT_DISK"
    EXT2_HOME_DISK="$BREENIX_ROOT/target/ext2-home-aarch64.img"
    if [ ! -f "$EXT2_HOME_DISK" ]; then
        cp "$EXT2_ROOT_DISK" "$EXT2_HOME_DISK"
    fi
else
    if [ "$BUILD" -eq 1 ]; then
        echo "[gate] building x86_64 kernel..."
        cargo build --release --features "$FEATURES,testing,external_test_bins" \
            --bin qemu-uefi >"$BUILD_LOG" 2>&1
        BREENIX_PRINT_UEFI_IMAGE=1 cargo run --release \
            --features "$FEATURES,testing,external_test_bins" --bin qemu-uefi >/dev/null
        # Repack both disks every run. Both are gitignored build outputs, so a
        # cached image silently boots the previous branch's binaries (#564).
        rm -f target/test_binaries.img
        cargo run -p xtask -- create-test-disk >/dev/null
        rm -f target/ext2.img
        ./scripts/create_ext2_disk.sh >/dev/null
    fi
    UEFI_IMG=$(ls -t target/release/build/breenix-*/out/breenix-uefi.img | head -1)
    test -n "$UEFI_IMG"
    EXT2_HOME_DISK="$BREENIX_ROOT/target/ext2-home.img"
    if [ ! -f "$EXT2_HOME_DISK" ]; then
        cp "$BREENIX_ROOT/target/ext2.img" "$EXT2_HOME_DISK"
    fi
fi

# Zero-warning build, with one documented exclusion: cargo's
# "packages contain code that will be rejected by a future version of Rust"
# notice is emitted for the rustup-vendored `core` crate that -Z build-std
# compiles, not for anything in this repository. It is present on an unmodified
# tree and cannot be fixed here. Every other warning is a gate failure.
if [ "$BUILD" -eq 1 ] && [ -f "$BUILD_LOG" ]; then
    if grep -E "^(warning|error)" "$BUILD_LOG" \
        | grep -vF "contain code that will be rejected by a future version of Rust" \
        | grep -q .; then
        echo "ext2 lock-race gate: FAIL (build produced warnings/errors, see $BUILD_LOG)"
        grep -E "^(warning|error)" "$BUILD_LOG" \
            | grep -vF "contain code that will be rejected by a future version of Rust" | head -20
        false
    fi
fi

# ---------------------------------------------------------------------------
# Boot
# ---------------------------------------------------------------------------
OUTPUT_DIR="$BREENIX_GATE_TMP/breenix_ext2_lock_race_gate_$ARCH"
rm -rf "$OUTPUT_DIR"
mkdir -p "$OUTPUT_DIR"

if [ "$ARCH" = "aarch64" ]; then
    PRIMARY_LOG="$OUTPUT_DIR/serial.txt"
    cp "$EXT2_ROOT_DISK" "$OUTPUT_DIR/ext2-root-writable.img"
    cp "$EXT2_HOME_DISK" "$OUTPUT_DIR/ext2-home-writable.img"
    qemu_host_lock_acquire
    timeout "${AARCH64_BOOT_TIMEOUT:-90}" qemu-system-aarch64 \
        -M virt,gic-version=3 -cpu max -m 512 -smp 4 \
        -kernel "$KERNEL" \
        -display none -no-reboot \
        -device virtio-gpu-device \
        -device virtio-keyboard-device \
        -device virtio-tablet-device \
        -device virtio-blk-device,drive=ext2root \
        -drive if=none,id=ext2root,format=raw,file="$OUTPUT_DIR/ext2-root-writable.img" \
        -device virtio-blk-device,drive=ext2home \
        -drive if=none,id=ext2home,format=raw,file="$OUTPUT_DIR/ext2-home-writable.img" \
        -device virtio-net-device,netdev=net0 \
        -netdev user,id=net0 \
        -serial file:"$OUTPUT_DIR/serial.txt" >"$OUTPUT_DIR/qemu.log" 2>&1 &
    QEMU_PID=$!
    LIVENESS_PATTERN='(\[heartbeat\]|\[EXEC_SMOKE:TARGET_OK\]|\[bcheck\] Complete:|\[bwm\] Display:)'
else
    PRIMARY_LOG="$OUTPUT_DIR/serial_kernel.txt"
    cp target/ovmf/x64/code.fd "$OUTPUT_DIR/OVMF_CODE.fd"
    cp target/ovmf/x64/vars.fd "$OUTPUT_DIR/OVMF_VARS.fd"
    timeout "${X86_BOOT_TIMEOUT:-1800}" qemu-system-x86_64 \
        -pflash "$OUTPUT_DIR/OVMF_CODE.fd" \
        -pflash "$OUTPUT_DIR/OVMF_VARS.fd" \
        -drive "if=none,id=hd,format=raw,readonly=on,file=$BREENIX_ROOT/$UEFI_IMG" \
        -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
        -drive "if=none,id=testdisk,format=raw,readonly=on,file=$BREENIX_ROOT/target/test_binaries.img" \
        -device virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off \
        -drive "if=none,id=ext2disk,format=raw,readonly=on,file=$BREENIX_ROOT/target/ext2.img" \
        -device virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off \
        -drive "if=none,id=homedisk,format=raw,readonly=on,file=$EXT2_HOME_DISK" \
        -device virtio-blk-pci,drive=homedisk,disable-modern=on,disable-legacy=off \
        -machine "pc,accel=${BREENIX_QEMU_ACCEL:-tcg}" -cpu "${BREENIX_QEMU_CPU:-qemu64}" -smp 1 -m 512 \
        -display none -no-reboot -no-shutdown \
        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
        -serial "file:$OUTPUT_DIR/serial_user.txt" \
        -serial "file:$OUTPUT_DIR/serial_kernel.txt" >"$OUTPUT_DIR/qemu.log" 2>&1 &
    QEMU_PID=$!
    LIVENESS_PATTERN='USERSPACE TEST COMPLETE'
fi

# Position-aware liveness check (review finding B4c): a plain grep over the
# whole capture for LIVENESS_PATTERN is order-insensitive — heartbeats print
# early in boot, long before the leg ever runs, so that check could never
# fail regardless of what happened afterward. LOCKRACE markers and every
# LIVENESS_PATTERN marker on both arches are emitted through the same kernel
# log sink (SERIAL2/COM2 -> serial.txt on aarch64, serial_kernel.txt on x86 —
# log::info! and serial_println! both route there, see kernel/src/serial.rs),
# so they share one chronological, line-numbered stream: a liveness marker
# only counts if it appears on a line strictly AFTER the leg's own COMPLETE
# line in that same file.
check_live_after_complete() {
    [ -f "$PRIMARY_LOG" ] || return 1
    local complete_line
    complete_line="$(grep -na "\[LOCKRACE:COMPLETE:" "$PRIMARY_LOG" 2>/dev/null | tail -1 | cut -d: -f1)"
    # review round-2 finding M4: a live run once hit
    # "line 278: lock: unbound variable" here, meaning $complete_line held
    # something non-numeric when the arithmetic context below evaluated it.
    # The trigger wasn't reproducible from the archived serial (this
    # function returns 1 cleanly against it), so guard defensively rather
    # than leave `set -u` to fail unexplained mid-gate: require a plain
    # decimal line number before ever reaching `$((...))`.
    [[ "$complete_line" =~ ^[0-9]+$ ]] || return 1
    tail -n "+$((complete_line + 1))" "$PRIMARY_LOG" | grep -qaE "$LIVENESS_PATTERN"
}

# #748 --park-only: total occurrences of $1 across every serial file so far.
# Deliberately NOT position-aware the way check_live_after_complete() is --
# EXT2_LOCK_PARK_FIRST (ext2_record_park(), crate::serial_println!, COM1 on
# x86) and "Added thread ... 'lockrace_holder'" (scheduler.rs,
# log_serial_println!, COM2 on x86) land in DIFFERENT files with
# independently-numbered lines that QEMU's plain `-serial file:` capture
# never timestamps, so no line-position comparison between them can ever be
# correct on x86 (confirmed live: the first probe run's marker printed in
# serial_user.txt while the leg's own spawn lines were in serial_kernel.txt,
# a real ordering gap this discovered in check_live_after_complete() too --
# see fix-notes.md). A monotonic occurrence COUNT within the ever-growing
# glob sidesteps this entirely: it only relies on each individual file
# being append-only, never on comparing positions across files.
count_marker() {
    # `grep -c` exits 1 when a file has zero matches (even though it still
    # prints "0"), and with `set -o pipefail` that makes the whole pipeline
    # exit 1 whenever every file's count is 0 -- which is the ordinary case
    # for most of this probe's run. Under this script's `set -e` + ERR trap,
    # a plain assignment from an unparenthesized failing command
    # substitution aborts the gate outright (caught live: the very first
    # local rerun after adding this function did exactly that). `|| true`
    # neutralizes the pipeline's own exit status; the awk-computed sum on
    # stdout is what callers actually consume, and it is correct either way.
    grep -coa "$1" "$OUTPUT_DIR"/serial*.txt 2>/dev/null | awk -F: '{s+=$NF} END{print s+0}' || true
}

# Poll for the leg's terminal marker (or a red signal that fires without it —
# see the header comment on why COMPLETE alone is not the red/green split)
# and, on a green run, the boot's own liveness marker genuinely after it.
STALL_SEEN=0
LOCKUP_SEEN=0
COMPLETE_SEEN=0
COMPLETE_AT=0
LIVE=0
# #748 --park-only: set the moment ext2_record_park()'s EXT2_LOCK_PARK_FIRST
# marker appears anywhere in the capture (informational only below -- the
# verdict uses the leg-scoped LEG_PARK_SEEN/LEG_STALL_SEEN pair instead,
# as defense-in-depth against a hypothetical unscoped hit from incidental
# boot_tests contention before the leg ever runs; no such hit has actually
# been observed on either recorded x86 run -- see fix-notes.md and the PR
# body for the correction of an earlier claim to the contrary).
PARK_FIRST_SEEN=0
# #748 --park-only leg-scoping: LEG_ENTRY_SEEN latches the poll where the
# leg's own "Added thread ... 'lockrace_holder'" spawn line first appears.
# PARK_BASELINE/STALL_BASELINE are set, on that same poll, to the occurrence
# counts from the PRIOR poll (PREV_PARK_COUNT/PREV_STALL_COUNT below) --
# deliberately one tick stale, not "as of right now" -- because the leg's
# reset-then-spawn-then-park sequence can complete inside a single 2s poll
# gap: a same-tick snapshot could already include that one-and-only park,
# permanently hiding it (LEG_PARK_SEEN would then need a SECOND park to
# ever latch, which this construction does not produce). Using the prior
# tick's count closes that false-negative gap: if entry wasn't visible on
# that prior poll, no leg-caused park can exist yet either --
# ext2_reset_lock_park_first_marker() runs strictly before the spawn that
# produces the very line we're detecting (run_one(), ext2_lock_race.rs), so
# "entry not yet visible" proves the reset already happened before this
# poll, ruling out a leg-caused park predating the baseline. It does NOT
# rule out the opposite-direction case: a park printed inside the SAME 2s
# window, strictly before the spawn line becomes grep-visible, is included
# in this prior-tick baseline and so is correctly excluded from
# LEG_PARK_SEEN -- but a park in that same window *after* the reset and
# *before* the spawn line would also be excluded, and nothing here proves
# that ordering can't happen; it is only ruled out in practice by
# `main.rs`'s call ordering (nothing else runs between the reset and the
# spawn) and by x86 running `-smp 1` (nothing else can contend the lock
# concurrently during that window). So: this baseline is airtight against
# false negatives (a genuine leg park will not be hidden), not against
# false positives from something else parking in the same 2s tick -- that
# guarantee comes from the call-ordering argument above, not from the
# tick arithmetic alone.
LEG_ENTRY_SEEN=0
PARK_BASELINE=0
STALL_BASELINE=0
LEG_PARK_SEEN=0
LEG_STALL_SEEN=0
PARK_SEEN_AT=0
PREV_PARK_COUNT=0
PREV_STALL_COUNT=0
POLL_BOUND=150
# #748 B3: after LEG_PARK_SEEN first latches, keep polling for this many
# more ticks (rather than breaking immediately) so a stall arriving shortly
# after the park -- the code's own park-then-fall-through-to-spin structure
# -- still has a chance to be observed and take precedence in the verdict
# below. Bounded and small on purpose: it cannot and does not claim to rule
# out a stall arriving after this window, which the PARK OBSERVED message
# says explicitly.
POST_PARK_GRACE=3
# The poll loop sleeps 2s/iteration, so its own worst-case duration is
# POLL_BOUND*2s. review finding m3: this defaulted to a flat 1800
# independent of X86_BOOT_TIMEOUT, so it could poll for up to an hour after
# `timeout` had already killed the QEMU process at the 1800s default --
# derive the default from the same timeout instead, so the loop cannot
# meaningfully outlive the process it is polling.
[ "$ARCH" = "x86" ] && POLL_BOUND="${X86_POLL_BOUND:-$(( ${X86_BOOT_TIMEOUT:-1800} / 2 ))}"
# Extra ticks given *after* COMPLETE is first seen, to let a genuinely later
# liveness marker actually print, before the boot is torn down.
POST_COMPLETE_GRACE=20
for i in $(seq 1 "$POLL_BOUND"); do
    if grep -qa "EXT2_LOCK_SPIN_STALL" "$OUTPUT_DIR"/serial*.txt 2>/dev/null; then
        STALL_SEEN=1
    fi
    if grep -qaE "soft lockup detected|SOFT LOCKUP DETECTED" "$OUTPUT_DIR"/serial*.txt 2>/dev/null; then
        LOCKUP_SEEN=1
    fi
    if [ "$COMPLETE_SEEN" -eq 0 ] && grep -qa "\[LOCKRACE:COMPLETE:" "$OUTPUT_DIR"/serial*.txt 2>/dev/null; then
        COMPLETE_SEEN=1
        COMPLETE_AT="$i"
    fi
    if [ "$PARK_FIRST_SEEN" -eq 0 ] && grep -qa "EXT2_LOCK_PARK_FIRST" "$OUTPUT_DIR"/serial*.txt 2>/dev/null; then
        PARK_FIRST_SEEN=1
    fi
    if grep -qaE "KERNEL PANIC" "$OUTPUT_DIR"/serial*.txt 2>/dev/null; then
        break
    fi
    if [ "$PARK_ONLY" -eq 1 ]; then
        cur_park_count="$(count_marker "EXT2_LOCK_PARK_FIRST")"
        cur_stall_count="$(count_marker "EXT2_LOCK_SPIN_STALL")"
        # #748 leg-scoping: see the PREV_PARK_COUNT/PREV_STALL_COUNT comment
        # above for why the baseline is the PRIOR tick's count, not this
        # one's -- latch leg entry once, snapshot both baselines from
        # PREV_*_COUNT at that instant, then only ever compare against
        # those baselines (never re-snapshot for this race attempt).
        if [ "$LEG_ENTRY_SEEN" -eq 0 ] \
            && grep -qa "'lockrace_holder' to scheduler" "$OUTPUT_DIR"/serial*.txt 2>/dev/null; then
            LEG_ENTRY_SEEN=1
            PARK_BASELINE="$PREV_PARK_COUNT"
            STALL_BASELINE="$PREV_STALL_COUNT"
        fi
        if [ "$LEG_ENTRY_SEEN" -eq 1 ]; then
            if [ "$LEG_PARK_SEEN" -eq 0 ] && [ "$cur_park_count" -gt "$PARK_BASELINE" ]; then
                LEG_PARK_SEEN=1
                PARK_SEEN_AT="$i"
            fi
            [ "$LEG_STALL_SEEN" -eq 0 ] && [ "$cur_stall_count" -gt "$STALL_BASELINE" ] && LEG_STALL_SEEN=1
        fi
        PREV_PARK_COUNT="$cur_park_count"
        PREV_STALL_COUNT="$cur_stall_count"
        # #748: exit as soon as a stall, lockup, or COMPLETE is captured --
        # those are decisive the instant they're seen (see the header
        # comment's three-outcome list and the "stall takes precedence"
        # note at the verdict below). A park alone does NOT break
        # immediately: keep polling up to POST_PARK_GRACE more ticks so a
        # stall following the park (the code's normal park-then-spin
        # structure, not an edge case) still gets the chance to latch and
        # take precedence, instead of being silently raced against the
        # break. LOCKUP_SEEN/COMPLETE_SEEN are deliberately left unscoped,
        # matching the full gate's own precedent (STALL_SEEN there is
        # unscoped too): a soft lockup or a COMPLETE line is abort/verdict-
        # worthy whenever it happens, not only after this leg specifically
        # starts.
        if [ "$LEG_STALL_SEEN" -eq 1 ] || [ "$LOCKUP_SEEN" -eq 1 ] || [ "$COMPLETE_SEEN" -eq 1 ]; then
            break
        fi
        if [ "$LEG_PARK_SEEN" -eq 1 ] && [ $((i - PARK_SEEN_AT)) -ge "$POST_PARK_GRACE" ]; then
            break
        fi
        sleep 2
        continue
    fi
    if [ "$STALL_SEEN" -eq 1 ] || [ "$LOCKUP_SEEN" -eq 1 ]; then
        # Red signal fired. No point waiting out the rest of the timeout —
        # a boot that reaches this state does not reliably recover.
        break
    fi
    if [ "$COMPLETE_SEEN" -eq 1 ]; then
        if check_live_after_complete; then
            LIVE=1
            break
        fi
        if [ $((i - COMPLETE_AT)) -ge "$POST_COMPLETE_GRACE" ]; then
            break
        fi
    fi
    sleep 2
done
kill "$QEMU_PID" 2>/dev/null || true
wait "$QEMU_PID" 2>/dev/null || true
qemu_host_lock_release

SERIAL_ALL="$OUTPUT_DIR/serial-all.txt"
cat "$OUTPUT_DIR"/serial*.txt >"$SERIAL_ALL" 2>/dev/null || true

# Authoritative, final position-aware liveness check against the fully
# flushed log (the polling loop above is only an early-exit optimization).
if check_live_after_complete; then
    LIVE=1
fi
# (No separate final recompute for park-only's leg-scoping: unlike
# check_live_after_complete()'s single grep, the delta tracking below is
# fundamentally a running computation across successive polls -- see the
# PREV_PARK_COUNT/PREV_STALL_COUNT comment in the loop above. A panic/abort
# that cuts the loop short before park-only's own block runs that iteration
# is already caught separately by the KERNEL PANIC/CPU exception checks
# just below, before park-only's verdict block is ever reached.)

# ---------------------------------------------------------------------------
# Verdict
# ---------------------------------------------------------------------------
fail() {
    echo "ext2 lock-race gate ($ARCH): FAIL - $1"
    echo "--- LOCKRACE / stall lines ---"
    grep -a "LOCKRACE\|EXT2_LOCK_SPIN_STALL\|soft lockup\|SOFT LOCKUP" "$SERIAL_ALL" || echo "(none)"
    false
}

if grep -qa "KERNEL PANIC" "$SERIAL_ALL"; then
    fail "kernel panic during or after the race leg"
fi
if grep -qaE "(DATA_ABORT|INSTRUCTION_ABORT|Unhandled sync exception)" "$SERIAL_ALL"; then
    fail "CPU exception during or after the race leg"
fi

# The two verdict shapes below (--park-only's own three-outcome probe, and
# the ordinary full-leg verdict) are now one if/else rather than each ending
# in its own early `exit`: this gate's own widened verdict-discipline rule
# (tests/teardown_structure.rs's verdict_trap_has_no_preempting_exit) scans
# the whole script for an `exit` statement other than the trap's own
# re-raise above, so PARK_ONLY's branch runs as an if/elif cascade (first
# match wins, matching the original sequential early-returns) and the
# ordinary verdict moves into the `else`, unreachable while PARK_ONLY=1 --
# the same effect the old unconditional exits had, expressed as control
# flow instead of early termination. `redden N` (defined above) is how the
# two FAIL-shaped outcomes (SPIN/NO PARK = 1, INCONCLUSIVE = 2) still reach
# the trap with their own distinct code; PARK OBSERVED is a genuine
# success, so it takes no further action beyond its own echo, and the
# cascade falls out to the end of the script, matching
# run-x86-prod-profile-boot-test.sh's PASS path (implicit status 0 from the
# last command run).
if [ "$PARK_ONLY" -eq 1 ]; then
    # #748: report exactly one of the three outcomes the header comment
    # names, distinctly -- "no observation yet" must never read as either
    # of the other two, and a spin/lockup without a park is a real,
    # different fact from park-only's own success case, not a gate FAIL.
    # The verdict below is leg-scoped (LEG_PARK_SEEN/LEG_STALL_SEEN, gated
    # on LEG_ENTRY_SEEN) rather than "did EXT2_LOCK_PARK_FIRST/
    # EXT2_LOCK_SPIN_STALL appear anywhere" -- as defense-in-depth against a
    # hypothetical unscoped hit from incidental boot_tests contention
    # entirely unrelated to this leg's own construction. No such hit has
    # actually occurred on either recorded x86 run to date (see fix-notes.md
    # and count_marker()'s comment above for the correction of an earlier
    # claim to the contrary). PARK_FIRST_SEEN (unscoped) is echoed only as
    # extra context.
    #
    # A stall takes precedence over a park below: `ext2_acquire`/
    # `ext2_acquire_write` park for up to EXT2_LOCK_PARK_ROUNDS rounds and
    # then fall through to the spin path by construction, so a stall
    # arriving after a park is the code's normal structure, not a surprise
    # -- reporting PARK OBSERVED in that case would green a run the leg's
    # own in-kernel oracle (`ext2_lock_race.rs::run_one()`) would call FAIL
    # (stall-dominant is checked before the park count there too).
    echo "--- LOCKRACE / park / stall lines (leg entry seen: $LEG_ENTRY_SEEN) ---"
    grep -a "LOCKRACE\|EXT2_LOCK_PARK_FIRST\|EXT2_LOCK_SPIN_STALL\|soft lockup\|SOFT LOCKUP" "$SERIAL_ALL" || echo "(none)"
    if [ "$LEG_STALL_SEEN" -eq 1 ] || [ "$LOCKUP_SEEN" -eq 1 ]; then
        if [ "$LEG_PARK_SEEN" -eq 1 ]; then
            echo "ext2 lock-race park-probe ($ARCH): SPIN, NO PARK - this leg's own construction also recorded a park (EXT2_LOCK_PARK_FIRST count rose from $PARK_BASELINE) but then stalled (or hit a soft-lockup) -- a stall is decisive regardless of an earlier park, matching the leg's own in-kernel oracle"
        else
            echo "ext2 lock-race park-probe ($ARCH): SPIN, NO PARK - this leg's own construction spun (or a soft-lockup fired) before recording a park of its own"
        fi
        redden 1
    elif [ "$LEG_PARK_SEEN" -eq 1 ]; then
        echo "ext2 lock-race park-probe ($ARCH): PARK OBSERVED - a park recorded strictly after this leg's own holder/contender spawn (EXT2_LOCK_PARK_FIRST count rose from $PARK_BASELINE); polled ${POST_PARK_GRACE}x2s past the park with no stall following in that window, but this probe stops there and therefore cannot exclude a later spin stall in the same leg beyond that bounded window -- it is not equivalent to the leg's own in-kernel PASS verdict"
    elif [ "$COMPLETE_SEEN" -eq 1 ] && [ "$LEG_ENTRY_SEEN" -eq 1 ]; then
        # The leg reached COMPLETE without this leg-scoped window ever
        # recording a park; the in-kernel no-park-observed=FAIL
        # classification (ext2_lock_race.rs) already covers this shape from
        # the leg's own side -- report it in the same bucket rather than
        # inventing a fourth one.
        echo "ext2 lock-race park-probe ($ARCH): SPIN, NO PARK - the leg reached COMPLETE without this leg-scoped window ever recording a park"
        redden 1
    elif [ "$LEG_ENTRY_SEEN" -eq 0 ] && [ "$COMPLETE_SEEN" -eq 1 ]; then
        # Reached the ARCH0 case caught live in local testing: the leg-entry
        # signal ("Added thread ... 'lockrace_holder' to scheduler") is
        # itself x86_64-only by construction (scheduler.rs gates it out on
        # ARM64 to avoid a documented log_serial_println!/SERIAL1 deadlock
        # risk there), so on aarch64 LEG_ENTRY_SEEN can never latch even
        # though the leg genuinely runs and reaches COMPLETE. Reporting
        # this as SPIN, NO PARK would be a real false negative (confirmed
        # live: an aarch64 run with 2 real parks, both PASS, per the full
        # gate's own verdict on the identical boot) -- INCONCLUSIVE with the
        # reason named is the honest call. --park-only's leg-scoping is
        # validated for --x86 only; use the full gate on other arches.
        echo "ext2 lock-race park-probe ($ARCH): INCONCLUSIVE - the leg's own entry signal never appeared on this arch (it is x86_64-only by construction; see the header comment), so leg-scoped tracking could not engage even though the leg reached COMPLETE -- this arch's --park-only result proves nothing either way; use the full gate (no --park-only) here instead"
        redden 2
    elif [ "$LEG_ENTRY_SEEN" -eq 0 ]; then
        echo "ext2 lock-race park-probe ($ARCH): INCONCLUSIVE - the leg's own 'Added thread ... lockrace_holder' spawn line never appeared within the probe's ${POLL_BOUND}x2s budget (unscoped EXT2_LOCK_PARK_FIRST seen so far: $PARK_FIRST_SEEN) -- this proves nothing either way, only that the budget wasn't enough to reach the leg"
        redden 2
    else
        echo "ext2 lock-race park-probe ($ARCH): INCONCLUSIVE - the leg was reached but neither a leg-scoped park nor a leg-scoped stall was observed within the probe's ${POLL_BOUND}x2s budget -- this proves nothing either way, only that the budget wasn't enough"
        redden 2
    fi
else
    if [ "$STALL_SEEN" -eq 1 ]; then
        fail "EXT2_LOCK_SPIN_STALL observed — a contended acquisition spun instead of parking (#728 live)"
    fi
    if [ "$LOCKUP_SEEN" -eq 1 ]; then
        fail "kernel soft-lockup detector fired during the race leg"
    fi
    if [ "$COMPLETE_SEEN" -ne 1 ]; then
        fail "the leg never reached its COMPLETE marker (hang with no stall/lockup signal caught, or it never ran)"
    fi

    COMPLETE_LINE="$(grep -a "\[LOCKRACE:COMPLETE:" "$SERIAL_ALL" | head -1)"
    LEG_PASS="$(echo "$COMPLETE_LINE" | sed -n 's/.*pass=\([0-9]*\):fail=\([0-9]*\)\].*/\1/p')"
    LEG_FAIL="$(echo "$COMPLETE_LINE" | sed -n 's/.*pass=\([0-9]*\):fail=\([0-9]*\)\].*/\2/p')"
    echo "[gate] $COMPLETE_LINE"
    RACE_VERDICT_LINES="$(grep -a "\[LOCKRACE:.*:race:verdict=" "$SERIAL_ALL" || true)"
    echo "$RACE_VERDICT_LINES" | sed 's/^/[gate]   /'

    # B4d: pass=0:fail=0 (no filesystem raced -- e.g. a construction that did
    # not reach run_one()) must not read as a pass, so LEG_PASS is
    # floor-checked rather than only requiring LEG_FAIL == 0.
    if [ -z "$LEG_PASS" ] || [ "$LEG_FAIL" != "0" ] || [ "$LEG_PASS" -lt 1 ]; then
        fail "the leg reported $LEG_FAIL failing filesystem(s) and $LEG_PASS passing (need >=1 pass, 0 fail)"
    fi

    # B4d ("require a per-race verdict line to exist"): the COMPLETE tally must
    # be backed by exactly that many printed :race:verdict= lines, not merely
    # asserted by the tally itself (run_one() in ext2_lock_race.rs prints
    # exactly one per attempted race, including each failure path -- setup
    # failure included -- so this can only diverge from LEG_PASS+LEG_FAIL if the
    # harness itself regresses).
    RACE_VERDICT_COUNT="$(echo "$RACE_VERDICT_LINES" | grep -c . || true)"
    EXPECTED_VERDICTS=$((LEG_PASS + LEG_FAIL))
    if [ "$RACE_VERDICT_COUNT" -ne "$EXPECTED_VERDICTS" ]; then
        fail "COMPLETE reported pass=$LEG_PASS:fail=$LEG_FAIL ($EXPECTED_VERDICTS total) but $RACE_VERDICT_COUNT :race:verdict= line(s) were printed"
    fi

    # B4b: a green run should confirm the fix's new park path was actually
    # entered. ext2_acquire()/ext2_acquire_write() already score a
    # construction that skipped parking as FAIL (see ext2_lock_race.rs's
    # no-park-observed detail), so LEG_FAIL==0 above already implies this --
    # this is a second, independent assertion straight off the printed
    # parks= fields, in case that in-kernel classification itself regresses.
    TOTAL_PARKS="$(echo "$RACE_VERDICT_LINES" | sed -n 's/.*parks=\([0-9]*\)\].*/\1/p' | awk '{s+=$1} END {print s+0}')"
    if [ "$TOTAL_PARKS" -lt 1 ]; then
        fail "pass=$LEG_PASS but no parks were observed across the passing race(s) -- a green that skipped the park path is not meaningful evidence (#728 review B4)"
    fi
    echo "[gate] total parks observed: $TOTAL_PARKS"

    [ "$LIVE" -eq 1 ] || fail "the boot did not reach a liveness marker on a line after the race leg's own COMPLETE line in $PRIMARY_LOG"

    echo "ext2 lock-race gate ($ARCH): PASSED - $LEG_PASS filesystem(s) raced clean ($TOTAL_PARKS total parks), kernel live after"
fi
