#!/bin/bash
# x86_64 PRODUCTION-PROFILE boot and teardown-census gate (#540).
#
# WHAT THIS GATE IS FOR
#
# Every x86 counter, census and teardown claim this project has ever made was
# measured by docker/qemu/run-x86-boot-tests.sh, which hard-codes
# `--features boot_tests,testing,external_test_bins`. aarch64 has had a
# zero-feature counterpart (run-aarch64-prod-profile-boot-test.sh) for a while;
# x86 had none, so no x86 boot had ever executed the kernel that actually ships.
# That gap is #540. This script closes it: it builds `qemu-uefi` with NO
# `--features` flag at all and asserts against the shipped profile.
#
# The absence of --features is the point. Neither Cargo.toml nor kernel/Cargo.toml
# declares a `default` feature and every test feature is additive, so omitting the
# flag is a real production build rather than a differently-configured test build.
# The profile-fidelity block below is the negative control that proves it: a dozen
# markers that only a boot_tests/testing kernel can emit are asserted absent, so an
# accidental rebuild of the test kernel reddens this gate instead of passing it.
#
# WHAT THE SHIPPED x86 PROFILE ACTUALLY DOES (measured, not assumed)
#
# A zero-feature x86 boot initialises memory/GDT/IDT/PCI/VirtIO, mounts the ext2
# root, emits the root-custody and tombstone censuses once from
# kernel/src/main.rs (that call site is OUTSIDE the `boot_tests` cfg block above
# it, which is why no kernel change is needed for this gate), finishes kernel
# init, and drops into the async executor running the keyboard and serial-command
# tasks. That is its steady state, and this gate polls for it.
#
# WHAT IT NOW DOES (#673, fixed)
#
# The shipped x86_64 kernel now launches a real init process. kernel_main_continue()
# (kernel/src/main.rs) gained a third block, mutually exclusive with the existing
# `testing`/`interactive` blocks: `#[cfg(not(any(feature = "testing", feature =
# "interactive", feature = "disable_x86_prod_init")))]`. It reads `/sbin/init` from
# the already-mounted ext2 root and drives it through the exact same arch-neutral
# ProcessManager transaction aarch64's `launch_init_from_elf()` uses --
# `create_init_process` -> `designate_init` -> `publish_init` (all in
# kernel/src/process/manager.rs, no target_arch cfg) -- then hands the published
# thread to `task::scheduler::spawn()`, the ordinary x86_64 dispatch entry point
# every other process on this arch already uses. That single call was the gap:
# `publish_init()` already left the thread scheduler-ready, but nothing on x86 had
# ever called `spawn()` on it before #673.
#
# `disable_x86_prod_init` is an anti-vacuity knob only (not meant to ship enabled):
# building with `--features disable_x86_prod_init` and neither `testing` nor
# `interactive` compiles ONLY the init-launch block back out (#673 review, M3 --
# not "byte-for-byte" the pre-fix kernel: the rest of this branch's fixes stay
# in place, including the scheduler blocked-state fix, the timer TOCTOU fix, and
# the console-executor kthread). It reproduces the one property this gate's
# anti-vacuity leg needs: a kernel that never constructs a userspace process, so
# this same gate can be run against it to prove the assertions below actually
# discriminate the fix rather than passing either way. Set
# X86_PROD_PROFILE_EXTRA_FEATURES=disable_x86_prod_init before invoking this
# script to run that leg.
#
# CENSUS SCOPE, UNCHANGED BY THE FIX
#
# `emit_root_custody_summary()`/`emit_tombstone_census()` (kernel/src/main.rs:676-677)
# still run exactly where they always did: inside `kernel_main`, strictly BEFORE
# `kernel_main_continue()` is ever called, and therefore strictly before the new
# init-launch block above. The pinned all-zero census literals below are therefore
# still a true "before any user process exists" baseline -- the fix does not move
# that emission point and does not require re-deriving those literals. What remains
# true, and was true before #673 too, is that this gate does not drive init through
# a reap/retire cycle, so it proves the census is at rest at its own emission point,
# not a return-to-zero after a teardown: nothing here asserts anything about
# teardown behavior once init is actually running its own workload.
#
# NEW EVIDENCE THIS GATE REQUIRES (#673)
#
# Construction is not dispatch, dispatch is not execution, and execution is not
# survival, so four independent signals are asserted, each proving a strictly
# stronger claim than the last:
#   1. `[INIT_DESIGNATION:x86_64:designated_pid=1:...]` (emitted from the new call
#      site) -- proves the ProcessManager transaction ran and named PID 1 as init.
#   2. PRECONDITION 5 ("Scheduler has runnable threads") flips from its old,
#      attributed FAIL to PASS -- proves the thread reached the scheduler's
#      runnable set, checked live via `task::scheduler::with_scheduler` at
#      kernel/src/main.rs:1970 (was main.rs:1798-1807 pre-fix, per #673's
#      original RCA -- the fix's own new block shifted every line after it; re-
#      derived again for the #673 review's M4 finding).
#   3. `RING3_SYSCALL: First syscall from userspace` present exactly once --
#      proves init's userspace code actually ran and reached the syscall
#      handler. This is a pre-existing, one-time marker
#      (kernel/src/syscall/handler.rs's emit_ring3_syscall_marker(), raw
#      serial output, no locks) that already exists for the test framework's
#      own stage-advance bookkeeping; #673 does not add it, only asserts on
#      it in a profile that had never reached Ring 3 before.
#   4. init's own first line, followed by bsshd actually starting AND
#      reaching its listening state, PLUS a dedicated spawn-smoke child
#      (/bin/spawn_smoke_target) observed exiting 0, reaped DIRECTLY by
#      run_spawn_smoke() before start_bsshd() runs -- NOT through init's
#      ordinary end-of-main() reap loop (#713, fixed -- x86 SPAWN syscall).
#      Before #713, SPAWN was unconditionally ENOSYS on x86, so this block
#      used to pin only a graceful-failure warning line; that is now
#      replaced with real survival/execution evidence, matching what the
#      aarch64 production gate has always pinned for bsshd. The signal
#      this ORIGINALLY replaced (pre-#673) -- "init was never reported
#      killed by signal" -- could never fire either way (it is init.rs's
#      reaped-CHILD message; PID 1 never reaps itself) and stayed removed
#      rather than reintroduced
#      as an unfalsifiable pin. See "INIT SURVIVAL EVIDENCE" below for
#      exactly what is and is not proven -- in particular, init's full
#      boot-script chain (bsh --init-shell -> /etc/init.js's further spawns)
#      is deliberately NOT pinned here; that chain has never run on x86
#      before #713 and is tracked separately as #722, not folded into this
#      gate's green.
#
# WHY INIT CANNOT RUN UNTIL BOOT'S OWN WORK IS DONE (#673, the real fight)
#
# The straightforward version of this fix -- spawn init, let it compete --
# reliably stalled the boot thread forever partway through its own remaining
# work. Root cause: main.rs's init_with_current() makes the boot thread
# BECOME the scheduler's idle thread ("Linux where the boot thread becomes
# the idle task"). The moment any thread's first syscall is confirmed,
# syscall::handler::is_ring3_confirmed() latches true, and
# interrupts/context_switch.rs permanently stops restoring idle's saved
# boot context whenever idle is next selected -- by design, to avoid
# resuming stale boot-time RIPs. Separately, schedule() never re-enqueues
# the outgoing thread when it IS the idle thread (correct for genuine idle).
# Combined: once init exists as a thread that keeps re-readying itself (its
# infinite reap loop nanosleep()s between waitpid attempts), the ready queue
# is never truly empty, idle is never selected as the last-resort fallback,
# and boot's own remaining PRECONDITION-checking/timer/census work -- tagged
# as "idle" -- can never resume. The fix (kernel/src/main.rs) is a scheduling
# brake taken unconditionally before init is even read from disk and released
# unconditionally at a single matching site, once every line of boot's own
# sequential work is behind it. This gate's evidence signals 1-3 above are
# what proves that brake is correctly placed: construction, then dispatch,
# then execution, in that order, with the shipped profile still reaching
# steady state afterward.
#
# INTERRUPT BOOT-ORDER CHANGE (#673 review, m1/MA5)
#
# The brake above is taken alongside a new `interrupts::enable()` call that
# moves hardware interrupt-enable to before this block, in every profile that
# takes it (including the ext2-read-failure path, since the enable is
# unconditional and outside the `match` arms). Before #673, the shipped
# production profile's first hardware interrupt-enable was several hundred
# lines later, immediately before the executor starts; every PRECONDITION
# check, `int3`, and the timer/clock_gettime test between here and there now
# run with interrupts genuinely enabled for the first time in production,
# matching what the `testing` profile has always done -- this is what
# surfaced time_test.rs's TOCTOU (m5).
#
# WHY THE CONSOLE SURVIVES INIT (#673 review, B1 -- a second, independent fight)
#
# The brake above only protects BOOT's own remaining work; it says nothing
# about what happens to the async executor (keyboard + serial console) once
# that brake lifts. The straightforward version of THAT -- run the executor
# from the boot thread's own tail, exactly like every other x86 profile --
# reliably loses the console within about a second of init starting: the
# instant init's first syscall lands, idle's saved context (which is what
# `executor.run()` would have to resume from) becomes exactly the kind of
# stale boot-time context the paragraph above says is permanently abandoned,
# and nothing else ever polls the executor again -- Waker::wake() only
# re-queues a task id, and Executor::run()'s own loop is the only thing that
# ever drains that queue. The fix is a SEPARATE mechanism from the brake
# above: the console executor runs in its own dedicated kernel thread
# (`task::kthread::kthread_run`, kernel/src/main.rs), spawned before the
# brake lifts. A kthread is never the scheduler's idle thread, so it never
# falls into the "abandon this context" rule at all -- it is preserved by the
# same ordinary dispatch path every other kernel thread uses, indefinitely.
# STEADY_STATE_LITERAL and the liveness check further below are what prove
# this: they can only pass if the console is still being serviced well after
# init has been running (see the LIVENESS section for exactly how).
#
# DISK LAYOUT
#
# kernel/src/fs/ext2/mod.rs's init_root_fs() looks for VirtIO block device index
# 2 (falling back to index 0), so the ext2 root only mounts when it is the third
# virtio-blk device -- the layout src/bin/qemu-uefi.rs builds. Production carries
# no test-binaries disk, so index 1 here is a zero-filled placeholder rather than
# target/test_binaries.img: that keeps the layout faithful while proving the
# shipped kernel needs no test artifacts to reach its steady state.
#
# VERDICT DISCIPLINE (#668)
#
# Every assertion below is a `test` under `set -e`, and `set -E` + the ERR trap
# make each of them loud: a silent `set -e` abort prints nothing of its own, so a
# genuine red used to die with no verdict text and no serial pointer. The trap
# fires on every uncaught nonzero exit, names the failing command and line, tails
# the serial, preserves it in a timestamped directory, and re-raises the same
# nonzero status.

set -euo pipefail
# errtrace: without this the ERR trap is not inherited into shell functions, and
# report_gate_failure is itself invoked from that trap.
set -E

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUTPUT_DIR="/tmp/breenix_x86_prod_profile"
QEMU_PID=""
# #673 anti-vacuity knob. Empty (default) builds the real shipped profile;
# set to "disable_x86_prod_init" to build the pre-fix, zero-userspace kernel
# and confirm this same gate discriminates the fix (see the header).
X86_PROD_PROFILE_EXTRA_FEATURES="${X86_PROD_PROFILE_EXTRA_FEATURES:-}"

# ---------------------------------------------------------------------------
# Production milestones. Each must appear exactly once. `Kernel initialization
# complete!` and the executor/serial-task pair are the shipped profile's own
# progress markers, not test-runner markers -- deliberately so, because a marker
# only a test profile emits would make this gate impossible to satisfy in the
# profile it exists to measure.
# ---------------------------------------------------------------------------
EXT2_ROOT_LITERAL='ext2 root filesystem mounted'
KERNEL_INIT_LITERAL='Kernel initialization complete!'
EXECUTOR_LITERAL='Starting async executor...'
# Steady state: the last line the production boot prints before the executor
# parks on the keyboard and serial streams. The poll loop waits for this.
STEADY_STATE_LITERAL='Serial command task started'
CONSOLE_PROMPT_LITERAL='breenix> '

# ---------------------------------------------------------------------------
# The teardown census, read in the shipped profile. Both lines are emitted once
# from kernel/src/main.rs outside every test cfg. The *_PREFIX counts are what
# stop a drifted-value line from hiding: pinning only the all-zero literal would
# pass a boot that emitted a nonzero census as well, and pinning only the prefix
# would pass any values at all.
#
# The x86 serial console carries the scheduler's raw single-character trace
# stream on the same port, so these lines can carry a prefix ("[SW]<K>..."):
# every match here is a substring match and must stay one.
# ---------------------------------------------------------------------------
TOMBSTONE_CENSUS_PREFIX='[TOMBSTONE_CENSUS:'
TOMBSTONE_CENSUS_PROD_LITERAL='[TOMBSTONE_CENSUS:resident=0:removed=0:reap_second=0:retire_second=0:abandoned_unqueued=0]'
ROOT_CUSTODY_PREFIX='[PT_ROOT_CUSTODY:'
ROOT_CUSTODY_PROD_LITERAL='[PT_ROOT_CUSTODY:no_proof=0:no_arch=0:terminated=0:undecided=0:mid_retire=0:retired=0]'

# ---------------------------------------------------------------------------
# Profile fidelity. Every literal here can only be emitted by a kernel built
# with boot_tests/testing/external_test_bins. One of them appearing means this
# gate measured the wrong kernel, which is the single failure mode that would
# make everything else it asserts meaningless.
#
# RING3_SMOKE: creating (not bare RING3_SMOKE:, #673) -- context_switch.rs
# emits an unconditional, once-per-boot "[ OK ] RING3_SMOKE: userspace
# executed + syscall path verified" canary on the FIRST real transition to
# Ring 3 in ANY profile (raw_serial_str, no cfg at all -- it exists so CI can
# verify userspace ran regardless of build). Before #673 that canary could
# never fire in production because no process ever reached Ring 3, which
# masked the bare RING3_SMOKE: substring here being wrong: it was only ever
# a valid test-only signal by accident of the OTHER defect, not because it is
# actually behind a testing cfg. #673 is the first thing that makes a
# production boot exercise that canary, and it does -- correctly -- so this
# entry is narrowed to RING3_SMOKE: creating, the prefix every genuinely
# testing-gated RING3_SMOKE print in main.rs shares and the canary does not.
# ---------------------------------------------------------------------------
TEST_ONLY_MARKERS=(
    'TEST_TALLY:'
    '[TEST:'
    'TEST RUNNER:'
    '[BOOT_TESTS:'
    'RING3_SMOKE: creating'
    '[TOMBSTONE_QUIESCE:'
    '[RECLAIM_DRAIN:'
    '[TOMBSTONE_JOIN_ORACLE:'
    '[KSTACK_OWNER_ORACLE:'
    '[KSTACK_QUIESCE_LEAK:'
    '[PT_CUSTODY_COUNTERS:'
    '[FRAME_CUSTODY_COUNTERS:'
    '[PT_RETIRE_COHORT:'
    '[PT_EXEC_COHORT:'
    '[EXEC_DETACH_ORACLE:'
    '[CLONE_ADMISSION_ORACLE:'
    '[INIT_DESIGNATION_ORACLE:'
    '[SCHED_STRAND_ORACLE:'
    '[CENSUS_WIDEN_ORACLE:'
    'Testing features enabled'
)

# ---------------------------------------------------------------------------
# Fault markers the shipped kernel can emit from unconditional code. These are
# not test scaffolding: a production boot printing any of them is a real defect
# report, so they are asserted at zero.
# ---------------------------------------------------------------------------
FAULT_MARKERS=(
    '[PMGUARD]'
    '[CREATION_LOCK_ORDER:VIOLATION'
    'DISK LOADING FAILED'
)
CRASH_MARKERS_PATTERN='KERNEL PANIC|panic!|DOUBLE FAULT|TRIPLE FAULT|soft lockup detected'
# ---------------------------------------------------------------------------
# PRECONDITION pins. R52 forbids an unattributed failure, not a disclosed one
# left unfixed forever: both entries that used to live here (#672, #673) are
# now FIXED, and both pins are re-derived in the direction that proves it --
# the FAIL line must be ABSENT and the PASS line present exactly once. A
# kernel that simply stopped emitting the check would satisfy a bare absence
# assertion, which is why the PASS line is pinned too, not just the FAIL
# line's absence.
#
# PRECONDITION 5 (#673, fixed) -- "Scheduler has runnable threads". The
# shipped x86 kernel launched no userspace process at all; kernel_main_continue()
# now reads /sbin/init from ext2 and hands it to task::scheduler::spawn() before
# this check runs (see the header). Do not relax the FAIL-absent / PASS-present
# pair back to a bare presence check or delete it -- that is how a fixed defect
# regresses silently.
#
# PRECONDITION 7 (#672, fixed) -- "Preemption disabled". kernel_main_continue()
# used to call per_cpu::preempt_disable() only inside `#[cfg(all(feature =
# "testing", not(feature = "interactive")))]` while calling the matching
# preempt_enable() unconditionally, so the shipped profile decremented a
# preempt_count that was already zero and the bare `sub dword ptr gs:[..], 1`
# wrapped it to 0xFFFFFFFF. The disable is now unconditional, so the bracket is
# symmetric in every build profile.
# ---------------------------------------------------------------------------
PRECOND_RUNNABLE_FAIL_LITERAL='PRECONDITION 5: Scheduler has runnable threads ✗ FAIL'
PRECOND_RUNNABLE_PASS_LITERAL='PRECONDITION 5: Scheduler has runnable threads ✓ PASS'
PRECOND_PREEMPT_FAIL_LITERAL='PRECONDITION 7: Preemption disabled ✗ FAIL'
PRECOND_PREEMPT_PASS_LITERAL='PRECONDITION 7: Preemption disabled ✓ PASS'

# ---------------------------------------------------------------------------
# The preempt-bracket census (#672), emitted once per boot from
# kernel/src/main.rs on the way to the executor, outside every test cfg. A
# nonzero value means some path called preempt_enable() without a matching
# preempt_disable(); per_cpu::preempt_enable() saturates at zero and counts the
# violation instead of wrapping the count. Prefix and all-zero literal are both
# pinned for the same reason as the teardown censuses above: the prefix alone
# would pass a drifted value, the literal alone would pass a boot that also
# emitted a nonzero line.
# ---------------------------------------------------------------------------
PREEMPT_CENSUS_PREFIX='[PREEMPT_BRACKET_CENSUS:'
PREEMPT_CENSUS_PROD_LITERAL='[PREEMPT_BRACKET_CENSUS:underflow=0]'
# #673 review, mi5/R3-B1: the census above is emitted before the production
# block's OWN preempt_enable() (kernel/src/main.rs, B3's release) runs, so
# it cannot see an underflow caused by that specific release. A second
# marker closes that gap -- but emitting it from boot's own tail right
# after the release raced the next timer tick (boot IS the scheduler's
# idle thread in this profile, #712, and any preemption of its remaining
# code abandons it for good): 1/6 shipping-profile boots at the #673
# fix-round-3 review lost the line to that race. R3-B1 moved the emission
# into the console_executor kthread's own first-run code instead -- a
# kthread's context is never abandoned that way, and it cannot start
# running until the release above has already completed (see the
# emission's own comment in kernel/src/main.rs for the full derivation
# against per_cpu::can_schedule()), so the read is now deterministic
# rather than a footrace.
PROD_BRACKET_RELEASE_PREFIX='[PROD_BRACKET_RELEASE_CENSUS:'
PROD_BRACKET_RELEASE_PROD_LITERAL='[PROD_BRACKET_RELEASE_CENSUS:underflow=0]'

# #673 review, MA6/R3-m4/R4-m1: test_timer_resolution() (kernel/src/time_test.rs)
# demotes a >1-tick window between its two reads from a panic to a counted,
# non-fatal log line (rare host scheduling jitter under a TCG-emulated PIT
# or a loaded CI runner, not by itself proof of a kernel defect -- see that
# function's own comment). Demoting it also made a genuine widening of the
# tolerance invisible to this gate; pin it at zero so a real drift (the
# window growing on every boot, not just an occasional stall) still reddens
# here rather than passing silently. The implemented check cannot itself
# distinguish the two: a single occasional stall also reds this gate at
# -eq 0, same as genuine drift would -- a red here is attributable to MA6's
# documented host-jitter tolerance until shown otherwise; adjudicate against
# the observed-values count before re-running.
TIMER_RESOLUTION_WINDOW_EXCEEDED_PREFIX='[TIMER_RESOLUTION_WINDOW_EXCEEDED:'

# ---------------------------------------------------------------------------
# #673 new evidence. Four independent signals, each a strictly stronger claim
# than the last (construction, dispatch, execution, survival) -- see the
# header's "NEW EVIDENCE THIS GATE REQUIRES" section for the full rationale.
# INIT_DESIGNATION is matched by prefix (not full literal) because
# reserved_collisions is a live counter, not a fixed value; designated_pid=1
# is the part that must never drift.
# ---------------------------------------------------------------------------
INIT_DESIGNATION_X86_PREFIX='[INIT_DESIGNATION:x86_64:designated_pid=1:'
RING3_SYSCALL_LITERAL='RING3_SYSCALL: First syscall from userspace'

# ---------------------------------------------------------------------------
# #673 review, M6/MA4: proves init did not just start but ran past its own
# startup sequence. INIT_FIRST_LINE is init.rs's own first print (the exact
# literal, not a prefix, since pid=1 is fixed for the singleton designated
# init this profile constructs).
#
# The pin this replaced (INIT_KILLED_PREFIX, absent) was structurally
# vacuous: it matched init.rs's waitpid(-1) reap-loop message for a REAPED
# CHILD, printed with the CHILD's own pid. PID 1 is init itself, and init
# never reaps itself via its own waitpid(-1) call, so that message could
# never be emitted whatever init did -- the assertion could not fail
# regardless of whether init actually survived. It is removed rather than
# kept as an unfalsifiable pin.
#
# INIT SURVIVAL EVIDENCE (#713, fixed -- was: "why this does NOT also pin
# bsshd, unlike the aarch64 production gate"): SPAWN (syscall nr 440) is no
# longer unconditionally stubbed to ENOSYS on x86_64 in
# kernel/src/syscall/handler.rs -- it now dispatches to a real x86
# implementation (handlers.rs's sys_spawn, backed by
# ProcessManager::spawn_process/create_process_with_argv/
# build_process_with_argv_at in manager.rs), mirroring aarch64's
# sys_spawn_aarch64. init's x86 main() still calls start_bsshd()
# unconditionally right after its own startup print, exactly like
# aarch64 -- and now that call succeeds:
#   [init] bsshd started (PID <pid>)
#   bsshd: listening on 0.0.0.0:<port>
# The QEMU launch below was also given a NIC (-netdev user,id=net0 -device
# e1000,netdev=net0,...) so bsshd's socket()/bind_inet()/listen() sequence
# (userspace/programs/src/bsshd.rs) has something to bind to -- e1000, not
# virtio-net-pci, because that is what x86's own net::init()
# (kernel/src/net/mod.rs) actually probes for
# (e1000::mac_address()); the aarch64 driver stack uses a different NIC
# family (net_pci, Parallels-specific) and its production gate's netdev
# line is not directly portable by device type, only by intent (give the
# kernel something to bind bsshd to). This matches the proven QEMU syntax
# already used elsewhere in this project (docker/qemu/run-dns-test.sh).
#
# What this gate deliberately does NOT pin, even now that SPAWN works:
# init's full boot-script chain (run_boot_script()'s `/bin/bsh
# --init-shell` child, which evaluates /etc/init.js and issues seven
# further spawns of its own -- telnetd, bwm, bterm, blog, bounce, bcheck,
# blogd). None of that chain has ever executed on x86 before #713 (x86 had
# no working spawn() at all until now), so it is not audited yet and is not
# safe to gate on in the same round that first makes spawn() work at all.
# Tracked separately as #722; not folded into this gate.
#
# Instead, spawn()'s end-to-end path (create + exec + run + exit + reap) is
# proven independently via a dedicated, minimal spawned child:
# run_spawn_smoke() (userspace/programs/src/init.rs) spawns
# /bin/spawn_smoke_target (a tiny userspace binary that just exits 0)
# before start_bsshd(), waits on it directly, and prints:
#   [init] spawn smoke: exited (code 0)
# This deliberately waits on the child directly rather than leaving it for
# the ordinary end-of-main() reap loop: that loop does not run until AFTER
# run_boot_script() returns, and run_boot_script()'s own chain (loading
# /bin/bsh, itself larger than anything loaded earlier in boot, then bsh's
# own /etc/init.js issuing seven further spawns) can take considerably
# longer under TCG emulation than this gate's timing budget -- entirely
# unrelated to whether spawn() itself works. Waiting directly proves the
# same thing (create + exec + run + exit + reap, not just "spawn returned
# Ok") without depending on the boot-script chain's own completion time,
# which stays out of scope per #722.
#
# INIT_BSSHD_WARNING_LITERAL, BSSHD_STARTED_LITERAL, BSSHD_LISTENING_LITERAL
# and INIT_SPAWN_SMOKE_REAP_LITERAL together are the actual survival pins
# now (#713): the warning literal must be ABSENT (a regression back to
# ENOSYS would make it reappear), and the other three must each be present
# exactly once.
# ---------------------------------------------------------------------------
INIT_FIRST_LINE_LITERAL='[init] Breenix init starting (PID 1)'
INIT_BSSHD_WARNING_LITERAL='[init] Warning: failed to start bsshd'
# #713: SPAWN now works on x86, so bsshd should start cleanly -- the warning
# above must now be ABSENT (see the assertion further down), and these three
# new literals are the actual survival/execution evidence in its place.
BSSHD_STARTED_LITERAL='[init] bsshd started (PID'
BSSHD_LISTENING_LITERAL='bsshd: listening'
INIT_SPAWN_SMOKE_REAP_LITERAL='[init] spawn smoke: exited (code 0)'
# The distinct failure literal run_spawn_smoke() now prints on a genuine
# waitpid() error (as opposed to a successful reap of a nonzero exit code,
# which is a different, still-legitimate-reap message this gate does not
# assert on): its presence would mean the "exited (code 0)" pin above was
# never actually reached via a real reap, so it must stay absent.
INIT_SPAWN_SMOKE_REAP_FAILED_LITERAL='[init] Warning: spawn smoke reap failed'
# TTY-x86 port: a light canary, not the full per-arm proof. The full
# per-arm/`pass=N` evidence lives in the dedicated gate
# (docker/qemu/run-x86-tty-oracle-gate.sh, ported from
# run-aarch64-tty-oracle-gate.sh); this standing gate only needs to know
# the leg ran and reported no failure, the same "increasingly strong
# signals, not re-proven at every gate" shape aarch64's own standing prod
# gate already uses for BLOCK_EINTR_ORACLE/POLL_TCP_ORACLE
# (run-aarch64-prod-profile-boot-test.sh).
#
# This file's OWN ratchet (tests/teardown_structure.rs,
# x86_production_profile_gate_verdict_discipline_holds) requires every
# marker assertion here to be an exact `-eq` count -- unlike
# run-aarch64-prod-profile-boot-test.sh's equivalent canaries, which
# legitimately use `-ge 1` under that file's own, looser house law. A first
# attempt at this canary pinned `[TTY_ORACLE:COMPLETE:` (the oracle's own
# tally line) at `-ge 1`/`-eq 2`, on the theory that its count is
# "structurally always exactly 2". That theory was false: the 2 comes
# entirely from `tty_oracle.rs`'s `emit()`, which deliberately prints the
# line TWICE ("console output interleaves at byte granularity, so a single
# shredded copy must not be able to hide a verdict") -- so a shred landing
# inside the literal itself (as this branch's own mutation battery observed
# once, verbatim: `<S>[SW]<K>[SW]<T><U><R>[TTY_ORACLE:FAIL:...`) can legally
# drop the count to 1, making an `-eq 2` pin flaky by construction. Pin the
# SINGLE-emit init record instead: init's own post-wait line prints exactly
# once per boot (`run_tty_oracle()`'s one `print!` call, on a genuine reap),
# with a distinct literal on a reap failure -- the same shape #713's own
# INIT_SPAWN_SMOKE_REAP_LITERAL/INIT_SPAWN_SMOKE_REAP_FAILED_LITERAL pair
# already uses just above.
INIT_TTY_ORACLE_EXIT_LITERAL='[init] tty_oracle exited pid='
INIT_TTY_ORACLE_REAP_FAILED_LITERAL='[init] Warning: tty_oracle reap failed'
TTY_ORACLE_FAIL_LITERAL='[TTY_ORACLE:FAIL'
# #721, fixed: x86 exec() production wiring. exec_smoke (userspace/programs/src/init.rs's
# run_exec_smoke(), positioned after run_tty_oracle() and before start_bsshd(), matching
# #713's own call-site convention) is the boot path's only execve caller on x86 too now --
# it execs into /bin/exec_smoke_target, which sleeps and yields at least once before
# printing its success marker (the exact scenario ExecSchedCommit exists to make correct:
# the scheduler-side thread copy must be written before the first post-exec preemption, or
# the exec'd thread resumes with stale pre-exec context on redispatch). Four positive
# markers proving a completed, argv-correct exec; three negative markers (#721 spec section
# 4, anti-vacuity) proving exec didn't merely appear to run.
EXEC_SMOKE_LAUNCH_LITERAL='[EXEC_SMOKE:LAUNCH]'
EXEC_SMOKE_TARGET_ENTER_LITERAL='[EXEC_SMOKE:TARGET_ENTER argc=2]'
EXEC_SMOKE_TARGET_OK_LITERAL='[EXEC_SMOKE:TARGET_OK]'
EXEC_SMOKE_LAUNCHER_EXIT_LITERAL='[EXEC_SMOKE:LAUNCHER_EXIT code=0]'
EXEC_SMOKE_SPAWN_FAILED_PREFIX='[EXEC_SMOKE:SPAWN_FAILED'
EXEC_SMOKE_EXEC_FAILED_LITERAL='[EXEC_SMOKE:EXEC_FAILED]'
EXEC_SMOKE_TARGET_ARGV_FAIL_PREFIX='[EXEC_SMOKE:TARGET_ARGV_FAIL'
# #721 K7: the kernel-side scheduler-commit receipt oracles (ExecSchedCommit::apply,
# kernel/src/task/scheduler.rs), unconditional in every profile -- not the
# [EXEC_LOCK_ORDER:commits=...] summary line, which is an x86_64 no-op stub
# (kernel/src/test_framework/executor.rs) whose only exec-path caller is
# boot_tests-gated and therefore never fires here.
EXEC_LOCK_ORDER_FIRST_COMMIT_LITERAL='[EXEC_LOCK_ORDER:FIRST_COMMIT]'
EXEC_LOCK_ORDER_PM_HELD_LITERAL='[EXEC_LOCK_ORDER:VIOLATION:PM_HELD]'
EXEC_LOCK_ORDER_UNPINNED_LITERAL='[EXEC_LOCK_ORDER:VIOLATION:UNPINNED]'
EXEC_LOCK_ORDER_NO_SCHED_THREAD_LITERAL='[EXEC_LOCK_ORDER:VIOLATION:NO_SCHED_THREAD]'
# #745, fixed: x86 fork() production wiring. fork_smoke (userspace/programs/src/init.rs's
# run_fork_smoke(), positioned after run_exec_smoke() and before start_bsshd(), matching
# #713/#721's own call-site convention) is the boot path's only fork() caller on x86 too now
# -- it fork()s, the child forces at least one voluntary yield before exiting (the exact
# "descheduled and redispatched at least once" scenario a masked-interrupt regression in the
# fork syscall path would need to survive), and both parent and child force a real CoW write
# fault on their own copy of a shared page. Positive markers proving a completed fork, a real
# reschedule, a genuine reap, and CoW isolation actually held; negative markers (#745 spec
# section 4, anti-vacuity) proving fork didn't fail, corrupt shared memory, or resume twice
# into the same branch (a real historical fork-bug shape).
FORK_SMOKE_LAUNCH_LITERAL='[FORK_SMOKE:LAUNCH]'
FORK_SMOKE_CHILD_PREFIX='[FORK_SMOKE:CHILD pid='
FORK_SMOKE_COW_ISOLATION_OK_PREFIX='[FORK_SMOKE:COW_ISOLATION_OK'
FORK_SMOKE_COW_ISOLATION_CORRUPTED_PREFIX='[FORK_SMOKE:COW_ISOLATION_CORRUPTED'
FORK_SMOKE_PARENT_REAPED_PREFIX='[FORK_SMOKE:PARENT_REAPED child='
# The child's EXIT CODE, spent separately (#745 review round 2, B1). The
# PARENT_REAPED prefix above pins only that a reap happened -- it matches
# `code=-1` (the `!wifexited` arm, i.e. the child was KILLED) just as
# happily as a clean `code=37`. A child that prints its CHILD marker and
# then dies during or after its voluntary yield is still reaped, and the
# userspace-fault kill path emits nothing in CRASH_MARKERS_PATTERN or
# FAULT_MARKERS, so without this pin the whole property fork_smoke exists
# to prove -- the freshly-published child survived a real reschedule round
# trip and exited cleanly -- was unasserted. The child pid varies boot to
# boot, so the exit-code half is pinned on its own: ` code=37]` cannot
# collide with `[FORK_SMOKE:LAUNCHER_EXIT code=0]` (the DIFFERENT process
# below) under grep -F, and 37 is fork_smoke.rs's CHILD_EXIT_CODE.
# NOT bound to the PARENT_REAPED prefix itself -- this is a bare suffix
# literal matched anywhere in the boot log by grep -F, so a future marker
# that also happened to end in ` code=37]` would satisfy it too. Only
# PARENT_REAPED produces that suffix today. Because the assertion is exact
# (-eq 1), such a collision would REDDEN the gate (two matches) rather than
# silently pass, so this fails safe rather than fails quiet.
# claim-lint:ok: run with CHILD_EXIT_CODE mutated 37 -> 38, this pin is the
# assertion that reddens while PARENT_REAPED stays 1 and crash markers stay 0 --
# docs/planning/745-x86-fork/serials/review-round-2/b1-mutation-child-exit-38-gate-FAIL.txt
FORK_SMOKE_PARENT_REAPED_CODE_LITERAL=' code=37]'
# fork_smoke's own top-level process (the one init directly spawns and
# reaps here) exits 0 via its normal `main()` return once the PARENT branch
# finishes printing -- code=37 is a DIFFERENT process, fork_smoke's own
# internal child C, which its own internal parent reaps and reports via
# FORK_SMOKE_PARENT_REAPED_PREFIX / FORK_SMOKE_PARENT_REAPED_CODE_LITERAL
# above.
FORK_SMOKE_LAUNCHER_EXIT_LITERAL='[FORK_SMOKE:LAUNCHER_EXIT code=0]'
FORK_SMOKE_SPAWN_FAILED_PREFIX='[FORK_SMOKE:SPAWN_FAILED'
FORK_SMOKE_FORK_FAILED_PREFIX='[FORK_SMOKE:FORK_FAILED'
FORK_SMOKE_CHILD_UNEXPECTED_RETURN_LITERAL='[FORK_SMOKE:CHILD_UNEXPECTED_RETURN]'
FORK_SMOKE_REAP_FAILED_PREFIX='[FORK_SMOKE:REAP_FAILED'
# #745 precheck C3: the x86 CoW *fault* path (handle_cow_fault et al.) had never
# executed in a zero-feature x86 build before fork_smoke existed. C3(2) asks
# for TWO distinct things and this gate now spends both:
#   (i) the faults ACTUALLY OCCURRED -- the literal below. A first attempt
#       pinned the bare `[COW FAULT #` prefix, whose count (8, observed)
#       depends on how many distinct 4KB pages each side touches after fork
#       and so cannot be an exact `-eq 0`/`-eq 1` under this harness's own
#       verdict-discipline rule (teardown_structure.rs's
#       x86_production_profile_gate_verdict_discipline_holds); it was
#       deleted in `411975c9` and, in round 1, nothing replaced it. The
#       fault NUMBER is what makes it pinnable: handle_cow_fault
#       (kernel/src/interrupts.rs) prints `[COW FAULT #N] addr=...` with N
#       from a boot-global fetch_add, so `[COW FAULT #0] addr=` is emitted
#       exactly once on any boot that takes at least one CoW fault and not
#       at all on a boot that takes none. The `addr=` suffix is load-bearing:
#       the same #0 also appears in the direct-path line
#       (`[COW FAULT #0] lock held, using direct path`), which would make a
#       bare `[COW FAULT #0]` pin count 2 on the signal-delivery path.
#       What this pins is "this boot took at least one CoW fault", NOT "this
#       fault was fork_smoke's": the re-measurement below timed fault #0 at
#       t=19.67s and [FORK_SMOKE:LAUNCH] at t=32.67s, so on the shipped x86
#       profile the first CoW fault is TTY arm 14's own fork, several seconds
#       earlier. Both are production forks that could not run on x86 before
#       #745, which is the property C3 is about; the fork_smoke-specific half
#       is the isolation receipt in (ii).
# claim-lint:ok: the "never executed in a zero-feature x86 build" finding is
# precheck C3, docs/planning/745-x86-fork/precheck.md; the receipt's own
# ability to fire is a run, not an assertion --
# docs/planning/745-x86-fork/serials/review-round-2/m2-mutation-cow-isolation-broken-serial_user.txt
#  (ii) the isolation actually HELD -- FORK_SMOKE_COW_ISOLATION_OK/CORRUPTED
#       above, a functional receipt (a broken refcount check corrupts memory
#       silently rather than crashing). This is a STRENGTHENING of C3, not
#       the "or, better" alternative it names: C3's "or, better" is C11's
#       count_cow_fault() counter, which this round still leaves unwired
#       (see docs/planning/745-x86-fork/README.md).
FORK_SMOKE_COW_FAULT_FIRST_LITERAL='[COW FAULT #0] addr='
# claim-lint:ok: the two known-gap notes below restate filed issues rather than
# making new claims -- #720 and #722.
# #720 — x86 user-stack VA bump allocator never reclaims (spawn-heavy
# exhaustion after ~240 creations).
# #722 — x86 prod-profile gate does not yet exercise init's full
# boot-script chain (`bsh --init-shell` -> `/etc/init.js`'s further spawns);
# deferred out of #713's scope. #745 precheck C13: that chain includes
# bsshd/bcheck/bterm, each of which now also has newly-reachable fork()
# call sites once fork_smoke proves the syscall itself works -- still not
# pinned here, same deferral.

# Measured on beast under TCG: steady state at 14s from QEMU launch. The bound is
# an order of magnitude above that so host contention cannot score a slow-but-
# healthy boot as a failure, and it is still far below run-x86-boot-tests.sh's
# 900s because this profile runs no oracle cohort.
POLL_BOUND_SECONDS=240
# Liveness. STIMULUS-RESPONSE, and it has to be, but not for the reason a
# pre-#673 header could give: post-fix, production has a real userspace init
# (unconditional prints) and every context switch emits an unconditional raw-
# serial marker (context_switch.rs's `[SW]`/`<K>`/`<U>`/`<I>`, outside every
# cfg) -- so a healthy shipped kernel at steady state is NOT silent, and a
# bare byte-growth check cannot fail for the reason it would need to: it was
# measured on this branch's own boot growing 54346 -> 63572 bytes (+9226) in
# 15s with NO console input at all, which is that switch/init traffic, not an
# echo -- exactly the signature "console dead, kernel still switching" would
# leave (#673 review, B1/B2). Total byte growth cannot distinguish that from
# a live console, so it cannot redden on B1's failure mode and does not try
# to here.
#
# So the gate asks a question only the console echo path can answer: it
# sends a bare newline and requires serial_command_task's `breenix> ` prompt
# COUNT to grow by exactly one. That prompt is printed only when the
# console-executor kthread (#673's B1 fix) is actually scheduled, polls its
# executor, and serial_command_task processes an RX-interrupt-delivered
# newline through its read-line loop -- switch-trace noise and init's own
# stdout cannot forge it. A kernel wedged in its halt loop, or one where the
# console-executor kthread is dead (B1's exact failure mode: the boot
# thread's saved executor.run() context abandoned with nothing left to poll
# it), answers zero delta either way -- this is the prompt-count check below,
# strengthened from a bare presence pin into a before/after delta.
#
# #721: this window doubles as the wall-clock budget for every marker assertion
# below it (spawn-smoke, tty-oracle, exec-smoke, fork-smoke, bsshd-started) --
# the script kills QEMU and reads final counts the moment this sleep returns.
# The window opens once steady state is reached, so what has to fit inside it is
# the span from `Serial command task started` to the LAST pinned marker.
# claim-lint:ok: "every marker assertion below it" is this file's own assertion
# block; the timings that bound them are in
# docs/planning/745-x86-fork/serials/review-round-2/liveness-window-remeasure-2026-09-02.txt
#
# RE-MEASURED post-#745 (precheck C13(b) made this mandatory, not conditional:
# the previous figure was taken before run_fork_smoke() and before TTY arm 14 --
# a fork+exec -- were added inside this same window; #745 review round 2, M3).
# Method: a read-only poller sampling this gate's own serial files every 0.25s
# and recording each marker's first appearance relative to QEMU launch, run
# beside an ordinary passing gate on beast under TCG at #745's round-2 bytes.
# Observed, one boot:
#   11.18s  Serial command task started   (steady state -- the window opens)
#   14.13s  [init] spawn smoke: exited (code 0)
#   19.67s  [COW FAULT #0] addr=
#   23.11s  [TTY_ORACLE:COMPLETE
#   27.37s  [EXEC_SMOKE:LAUNCH]      29.07s  [EXEC_SMOKE:TARGET_OK]
#   32.67s  [FORK_SMOKE:LAUNCH]      33.50s  [FORK_SMOKE:PARENT_REAPED child=
#   38.49s  bsshd: listening         (the last pinned marker)
# Span from steady state to the last pinned marker: 27.3s, i.e. the added fork
# work did not measurably widen the 28s span #721 recorded. 60s keeps ~2.2x
# margin over it, without the "order of magnitude" POLL_BOUND_SECONDS margin
# above, which would make every gate run needlessly slow for a bound this tight
# to the measured figure. Raw timing artifact:
# docs/planning/745-x86-fork/serials/review-round-2/liveness-window-remeasure-2026-09-02.txt.
# claim-lint:ok: every figure above is a first-appearance timestamp from that
# one file, not an estimate: docs/planning/745-x86-fork/serials/review-round-2/liveness-window-remeasure-2026-09-02.txt
LIVENESS_STIMULUS_BYTE=$'\n'
LIVENESS_WINDOW_SECONDS=60

report_gate_failure() {
    local exit_code=$?
    local line_no="$1"
    local failing_cmd="$2"
    # Disarm before doing anything else: the diagnosis below must not be able to
    # re-enter this handler and bury the original failure.
    trap - ERR
    echo "x86 production-profile gate: FAIL (set -e abort at ${BASH_SOURCE[0]}:${line_no}, exit ${exit_code})"
    echo "  failing command: ${failing_cmd}"
    if [ -n "$QEMU_PID" ]; then
        kill "$QEMU_PID" 2>/dev/null || true
        wait "$QEMU_PID" 2>/dev/null || true
        QEMU_PID=""
    fi
    if compgen -G "$OUTPUT_DIR/serial_*.txt" >/dev/null 2>&1; then
        local failure_dir
        failure_dir="/tmp/breenix_x86_prod_profile_failures/$(date -u +%Y%m%dT%H%M%SZ)_$$"
        mkdir -p "$failure_dir"
        cp "$OUTPUT_DIR"/serial_*.txt "$failure_dir/"
        echo "  preserved failing serial: $failure_dir"
        echo "--- observed values ---"
        print_observed_values
        echo "--- serial tail (last 60 lines per file) ---"
        tail -n 60 "$OUTPUT_DIR"/serial_*.txt
    fi
    exit "$exit_code"
}
trap 'report_gate_failure "$LINENO" "$BASH_COMMAND"' ERR

# Matching-LINE count across both serial files (#673 review, mi7 -- grep -c
# counts lines, not substring occurrences, so two matches on one line would
# still count once). The x86 console carries init's stdout on the same
# serial stream as the shell prompt, so a same-line collision between a
# pinned literal and the prompt could in principle misreport a 1->2 delta as
# 1->1 (false red) or hide a real increase; 25/25 production-profile boots
# (#673 fix round 2 -- the battery that measured this ran in round 2, NOT
# round 3; corrected #673 review R3-MA2/R3-m3, which caught this comment
# repeating round 2's figure as if it were round 3's own) observed no such
# collision at that round's landed bytes. Re-measured at round 4's landed
# bytes (`4aee31ea`): 12/12 production-profile boots green, 0 same-line
# marker/prompt collisions (#673 fix round 4, `r4-prove.md` leg 1). This is
# a disclosed sharp edge that has been checked, not a silently mislabeled
# one. grep exits 1
# when nothing matches, which under `set -e`/`pipefail` would abort before
# the assertion that wants to read the zero, so the status is swallowed
# inside the group and awk -- which always exits 0 -- produces the number.
marker_count() {
    local literal="$1"
    local total
    total=$( { grep -F -h -c -- "$literal" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true; } \
        | awk '{ total += $1 } END { print total + 0 }')
    printf '%s' "$total"
}

crash_count() {
    local total
    total=$( { grep -E -h -c -- "$CRASH_MARKERS_PATTERN" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true; } \
        | awk '{ total += $1 } END { print total + 0 }')
    printf '%s' "$total"
}

serial_bytes() {
    local total
    total=$( { cat "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true; } | wc -c | awk '{ print $1 + 0 }')
    printf '%s' "$total"
}


print_observed_values() {
    echo "  ext2 root mounted:            $(marker_count "$EXT2_ROOT_LITERAL")"
    echo "  kernel init complete:         $(marker_count "$KERNEL_INIT_LITERAL")"
    echo "  async executor started:       $(marker_count "$EXECUTOR_LITERAL")"
    echo "  steady state reached:         $(marker_count "$STEADY_STATE_LITERAL")"
    echo "  console prompt:               $(marker_count "$CONSOLE_PROMPT_LITERAL")"
    echo "  tombstone census lines:       $(marker_count "$TOMBSTONE_CENSUS_PREFIX")"
    echo "  tombstone census at rest:     $(marker_count "$TOMBSTONE_CENSUS_PROD_LITERAL")"
    echo "  root custody lines:           $(marker_count "$ROOT_CUSTODY_PREFIX")"
    echo "  root custody at rest:         $(marker_count "$ROOT_CUSTODY_PROD_LITERAL")"
    echo "  crash markers:                $(crash_count)"
    local marker
    for marker in "${TEST_ONLY_MARKERS[@]}"; do
        echo "  test-only marker '$marker': $(marker_count "$marker")"
    done
    for marker in "${FAULT_MARKERS[@]}"; do
        echo "  fault marker '$marker': $(marker_count "$marker")"
    done
    echo "  fixed PRECONDITION 5 fail (#673): $(marker_count "$PRECOND_RUNNABLE_FAIL_LITERAL")"
    echo "  fixed PRECONDITION 5 pass (#673): $(marker_count "$PRECOND_RUNNABLE_PASS_LITERAL")"
    echo "  fixed PRECONDITION 7 fail (#672): $(marker_count "$PRECOND_PREEMPT_FAIL_LITERAL")"
    echo "  fixed PRECONDITION 7 pass (#672): $(marker_count "$PRECOND_PREEMPT_PASS_LITERAL")"
    echo "  preempt census lines:          $(marker_count "$PREEMPT_CENSUS_PREFIX")"
    echo "  preempt census at rest:        $(marker_count "$PREEMPT_CENSUS_PROD_LITERAL")"
    echo "  prod bracket release census:   $(marker_count "$PROD_BRACKET_RELEASE_PREFIX")"
    echo "  prod bracket release at rest:  $(marker_count "$PROD_BRACKET_RELEASE_PROD_LITERAL")"
    echo "  timer resolution window exceeded (#673 MA6/R3-m4): $(marker_count "$TIMER_RESOLUTION_WINDOW_EXCEEDED_PREFIX")"
    echo "  init designation (#673):      $(marker_count "$INIT_DESIGNATION_X86_PREFIX")"
    echo "  ring3 syscall confirmed (#673): $(marker_count "$RING3_SYSCALL_LITERAL")"
    echo "  init first line (#673 M6):    $(marker_count "$INIT_FIRST_LINE_LITERAL")"
    echo "  init bsshd-launch warning (must be absent, #713): $(marker_count "$INIT_BSSHD_WARNING_LITERAL")"
    echo "  bsshd started (#713):          $(marker_count "$BSSHD_STARTED_LITERAL")"
    echo "  bsshd listening (#713):        $(marker_count "$BSSHD_LISTENING_LITERAL")"
    echo "  spawn-smoke child reaped exit 0 (#713): $(marker_count "$INIT_SPAWN_SMOKE_REAP_LITERAL")"
    echo "  spawn-smoke reap failed (must be absent, #713 fix-round-2): $(marker_count "$INIT_SPAWN_SMOKE_REAP_FAILED_LITERAL")"
    echo "  tty oracle exit record (TTY-x86 port): $(marker_count "$INIT_TTY_ORACLE_EXIT_LITERAL")"
    echo "  tty oracle reap failed (must be absent, TTY-x86 port): $(marker_count "$INIT_TTY_ORACLE_REAP_FAILED_LITERAL")"
    echo "  tty oracle failed (must be absent, TTY-x86 port): $(marker_count "$TTY_ORACLE_FAIL_LITERAL")"
    echo "  exec smoke launch (#721):      $(marker_count "$EXEC_SMOKE_LAUNCH_LITERAL")"
    echo "  exec smoke target enter argc=2 (#721): $(marker_count "$EXEC_SMOKE_TARGET_ENTER_LITERAL")"
    echo "  exec smoke target ok (#721):   $(marker_count "$EXEC_SMOKE_TARGET_OK_LITERAL")"
    echo "  exec smoke launcher exit code=0 (#721): $(marker_count "$EXEC_SMOKE_LAUNCHER_EXIT_LITERAL")"
    echo "  exec smoke spawn failed (must be absent, #721): $(marker_count "$EXEC_SMOKE_SPAWN_FAILED_PREFIX")"
    echo "  exec smoke exec failed (must be absent, #721): $(marker_count "$EXEC_SMOKE_EXEC_FAILED_LITERAL")"
    echo "  exec smoke target argv fail (must be absent, #721): $(marker_count "$EXEC_SMOKE_TARGET_ARGV_FAIL_PREFIX")"
    echo "  exec lock order first commit (#721 K7): $(marker_count "$EXEC_LOCK_ORDER_FIRST_COMMIT_LITERAL")"
    echo "  exec lock order PM-held violation (must be absent, #721 K7): $(marker_count "$EXEC_LOCK_ORDER_PM_HELD_LITERAL")"
    echo "  exec lock order unpinned violation (must be absent, #721 K7): $(marker_count "$EXEC_LOCK_ORDER_UNPINNED_LITERAL")"
    echo "  exec lock order no-sched-thread violation (must be absent, #721 K7): $(marker_count "$EXEC_LOCK_ORDER_NO_SCHED_THREAD_LITERAL")"
    echo "  fork smoke launch (#745):      $(marker_count "$FORK_SMOKE_LAUNCH_LITERAL")"
    echo "  fork smoke child (#745):       $(marker_count "$FORK_SMOKE_CHILD_PREFIX")"
    echo "  fork smoke CoW isolation OK (#745): $(marker_count "$FORK_SMOKE_COW_ISOLATION_OK_PREFIX")"
    echo "  fork smoke CoW isolation corrupted (must be absent, #745): $(marker_count "$FORK_SMOKE_COW_ISOLATION_CORRUPTED_PREFIX")"
    echo "  fork smoke parent reaped (#745): $(marker_count "$FORK_SMOKE_PARENT_REAPED_PREFIX")"
    echo "  fork smoke launcher exit code=0 (#745): $(marker_count "$FORK_SMOKE_LAUNCHER_EXIT_LITERAL")"
    echo "  fork smoke child exit code=37 (#745 review r2 B1): $(marker_count "$FORK_SMOKE_PARENT_REAPED_CODE_LITERAL")"
    echo "  first CoW fault taken (#745 precheck C3(2)): $(marker_count "$FORK_SMOKE_COW_FAULT_FIRST_LITERAL")"
    echo "  fork smoke spawn failed (must be absent, #745): $(marker_count "$FORK_SMOKE_SPAWN_FAILED_PREFIX")"
    echo "  fork smoke fork failed (must be absent, #745): $(marker_count "$FORK_SMOKE_FORK_FAILED_PREFIX")"
    echo "  fork smoke child unexpected return (must be absent, #745): $(marker_count "$FORK_SMOKE_CHILD_UNEXPECTED_RETURN_LITERAL")"
    echo "  fork smoke reap failed (must be absent, #745): $(marker_count "$FORK_SMOKE_REAP_FAILED_PREFIX")"
    # init's full boot-script chain (bsh --init-shell -> /etc/init.js's
    # further spawns) is still not pinned here -- see INIT SURVIVAL
    # EVIDENCE above and #722.
    { grep -F -h -- "$TOMBSTONE_CENSUS_PREFIX" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true; }
    { grep -F -h -- "$ROOT_CUSTODY_PREFIX" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true; }
    { grep -F -h -- "$PREEMPT_CENSUS_PREFIX" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true; }
    { grep -F -h -- "$PROD_BRACKET_RELEASE_PREFIX" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true; }
    { grep -F -h -- "$TIMER_RESOLUTION_WINDOW_EXCEEDED_PREFIX" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true; }
    { grep -F -h -- "$INIT_DESIGNATION_X86_PREFIX" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true; }
}

cd "$BREENIX_ROOT"

echo "Building the shipped x86_64 production kernel profile..."
# #673 review, B5: the ONLY two legal values are empty (the shipped profile)
# and the documented anti-vacuity knob below. "The absence of --features is
# the point" (see the header) is not just prose about the default -- any
# other value here would silently build and measure a DIFFERENT kernel than
# the one this gate exists to prove, with no assertion below able to catch
# it, and the trap above is already armed to make this loud (#668).
test -z "$X86_PROD_PROFILE_EXTRA_FEATURES" -o "$X86_PROD_PROFILE_EXTRA_FEATURES" = "disable_x86_prod_init"
FEATURE_ARGS=()
if [ -n "$X86_PROD_PROFILE_EXTRA_FEATURES" ]; then
    FEATURE_ARGS=(--features "$X86_PROD_PROFILE_EXTRA_FEATURES")
    echo "  (#673 anti-vacuity leg: extra features = $X86_PROD_PROFILE_EXTRA_FEATURES)"
fi
# No --features by default, and that omission is the assertion: silently
# adding one would make this gate measure a different kernel than the one
# the project ships. The one documented exception is the #673 anti-vacuity
# knob above, which is off by default and must be opted into explicitly.
# The existing image is removed first so a stale artifact from a differently
# featured build cannot be picked up by the newest-first selection below.
rm -f target/release/build/breenix-*/out/breenix-uefi.img
BUILD_LOG=/tmp/breenix_x86_prod_profile_build.log
cargo build --release ${FEATURE_ARGS[@]+"${FEATURE_ARGS[@]}"} --bin qemu-uefi 2>&1 | tee "$BUILD_LOG"
# Zero-warning law. grep exits 1 on the clean case, so the status is swallowed in
# the group and awk -- which always exits 0 -- produces the number.
test "$( { grep -c '^warning' "$BUILD_LOG" || true; } | awk '{ print $1 + 0 }')" -eq 0
BREENIX_PRINT_UEFI_IMAGE=1 cargo run --release ${FEATURE_ARGS[@]+"${FEATURE_ARGS[@]}"} --bin qemu-uefi >/dev/null
UEFI_IMG=$(ls -t target/release/build/breenix-*/out/breenix-uefi.img | head -1)
test -n "$UEFI_IMG"

# #721 K9: create_ext2_disk.sh installs *.elf by glob with no manifest check (its
# only missing-binary handling is the busybox warning) and this script does not
# rebuild userspace itself -- so on a stale beast checkout where
# `userspace/programs/build.sh --arch x86_64` has not been re-run since the exec
# smoke binaries were added, /bin/exec_smoke would simply be absent from the
# image and the EXEC_SMOKE assertions below would redden as an *exec defect*
# rather than the *missing-artifact* build error it actually is. Fail loudly and
# distinctly here, before that ambiguity can happen.
for exec_smoke_bin in exec_smoke exec_smoke_target; do
    elf_path="userspace/programs/${exec_smoke_bin}.elf"
    if [ ! -f "$elf_path" ]; then
        echo "userspace artifact missing: ${elf_path} -- run userspace/programs/build.sh --arch x86_64" >&2
        false
    fi
    # #721 m3: presence alone does not rule out a stale wrong-arch artifact --
    # a checkout that last ran `build.sh --arch aarch64` leaves an aarch64 .elf
    # in place, which would pass the existence check above and then redden the
    # EXEC_SMOKE assertions below as an *exec defect* instead of the *stale
    # build* it actually is. Read the ELF e_machine field directly (offset
    # 0x12, little-endian u16) rather than trusting the file's mere presence.
    exec_smoke_e_machine=$(od -An -tu2 -j 18 -N 2 "$elf_path" | tr -d ' ')
    if [ "$exec_smoke_e_machine" != "62" ]; then
        echo "userspace artifact wrong architecture: ${elf_path} has e_machine=${exec_smoke_e_machine} (expected 62/EM_X86_64) -- run userspace/programs/build.sh --arch x86_64" >&2
        false
    fi
done

# The ext2 image carries the userspace binaries, so rebuild it every run: a
# cached image silently boots an old root filesystem.
rm -f target/ext2.img
./scripts/create_ext2_disk.sh
test -f target/ext2.img

rm -rf "$OUTPUT_DIR"
mkdir -p "$OUTPUT_DIR"
cp target/ovmf/x64/code.fd "$OUTPUT_DIR/OVMF_CODE.fd"
cp target/ovmf/x64/vars.fd "$OUTPUT_DIR/OVMF_VARS.fd"
# Zero-filled stand-in for the test-binaries disk production does not carry.
# Its only job is to occupy virtio-blk index 1 so the ext2 root lands on index 2,
# which is where init_root_fs() looks for it.
dd if=/dev/zero of="$OUTPUT_DIR/placeholder.img" bs=1M count=16 status=none

echo "Booting the x86_64 production profile..."
qemu-system-x86_64 \
    -pflash "$OUTPUT_DIR/OVMF_CODE.fd" \
    -pflash "$OUTPUT_DIR/OVMF_VARS.fd" \
    -drive "if=none,id=hd,format=raw,readonly=on,file=$BREENIX_ROOT/$UEFI_IMG" \
    -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
    -drive "if=none,id=placeholder,format=raw,readonly=on,file=$OUTPUT_DIR/placeholder.img" \
    -device virtio-blk-pci,drive=placeholder,disable-modern=on,disable-legacy=off \
    -drive "if=none,id=ext2disk,format=raw,readonly=on,file=$BREENIX_ROOT/target/ext2.img" \
    -device virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off \
    -netdev user,id=net0 \
    -device e1000,netdev=net0,mac=52:54:00:12:34:56 \
    -machine pc,accel=tcg -cpu qemu64 -smp 1 -m 512 \
    -display none -no-reboot -no-shutdown \
    -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
    -chardev "socket,id=console,path=$OUTPUT_DIR/console.sock,server=on,wait=off,logfile=$OUTPUT_DIR/serial_user.txt" \
    -serial chardev:console \
    -serial "file:$OUTPUT_DIR/serial_kernel.txt" \
    >"$OUTPUT_DIR/qemu.log" 2>&1 &
QEMU_PID=$!

reached=false
for _ in $(seq 1 "$POLL_BOUND_SECONDS"); do
    if grep -qF -- "$STEADY_STATE_LITERAL" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null; then
        reached=true
        break
    fi
    if grep -qE -- "$CRASH_MARKERS_PATTERN" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null; then
        break
    fi
    if ! kill -0 "$QEMU_PID" 2>/dev/null; then
        break
    fi
    sleep 1
done

# Explicit assertion, not a bare boolean variable: a bare boolean executed as a
# command is silent under set -e and leaves a red with no verdict text.
test "$reached" = true

# Liveness. Both samples are taken with QEMU still running, after steady state,
# with the stimulus written between them: a kernel that has wedged in its halt
# loop never services the UART interrupt and never echoes, a live one answers
# with a fresh `breenix> ` prompt. See the LIVENESS_STIMULUS_BYTE block above
# for why a bare byte-growth check is not available on a kernel with #672 and
# #673 both fixed, and why counting THIS specific marker's growth is.
PROMPT_BEFORE=$(marker_count "$CONSOLE_PROMPT_LITERAL")
python3 - "$OUTPUT_DIR/console.sock" "$LIVENESS_STIMULUS_BYTE" <<'STIMULUS'
import socket
import sys

console = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
console.connect(sys.argv[1])
console.sendall(sys.argv[2].encode())
console.close()
STIMULUS
sleep "$LIVENESS_WINDOW_SECONDS"
PROMPT_AFTER=$(marker_count "$CONSOLE_PROMPT_LITERAL")

kill "$QEMU_PID" 2>/dev/null || true
wait "$QEMU_PID" 2>/dev/null || true
QEMU_PID=""

test "$PROMPT_AFTER" -gt "$PROMPT_BEFORE"

# Production milestones.
test "$(marker_count "$EXT2_ROOT_LITERAL")" -eq 1
test "$(marker_count "$KERNEL_INIT_LITERAL")" -eq 1
test "$(marker_count "$EXECUTOR_LITERAL")" -eq 1
test "$(marker_count "$STEADY_STATE_LITERAL")" -eq 1
# Strengthened from a bare presence pin (#673 review, B2) into a before/after
# delta: PROMPT_BEFORE is the steady-state prompt printed once at start,
# PROMPT_AFTER is that plus exactly the one the liveness stimulus earned.
test "$PROMPT_BEFORE" -eq 1
test "$PROMPT_AFTER" -eq 2

# Teardown census, at rest, in the shipped profile.
test "$(marker_count "$TOMBSTONE_CENSUS_PREFIX")" -eq 1
test "$(marker_count "$TOMBSTONE_CENSUS_PROD_LITERAL")" -eq 1
test "$(marker_count "$ROOT_CUSTODY_PREFIX")" -eq 1
test "$(marker_count "$ROOT_CUSTODY_PROD_LITERAL")" -eq 1

# Profile fidelity: nothing a test kernel emits may appear.
for marker in "${TEST_ONLY_MARKERS[@]}"; do
    test "$(marker_count "$marker")" -eq 0
done

# No production fault report, no crash.
for marker in "${FAULT_MARKERS[@]}"; do
    test "$(marker_count "$marker")" -eq 0
done
test "$(crash_count)" -eq 0

# #673, fixed: the shipped kernel now launches init, so PRECONDITION 5 must
# now pass and must still be reported. See the block above.
test "$(marker_count "$PRECOND_RUNNABLE_FAIL_LITERAL")" -eq 0
test "$(marker_count "$PRECOND_RUNNABLE_PASS_LITERAL")" -eq 1

# #672, fixed: the preempt bracket is symmetric in the shipped profile, so
# PRECONDITION 7 must now pass and must still be reported.
test "$(marker_count "$PRECOND_PREEMPT_FAIL_LITERAL")" -eq 0
test "$(marker_count "$PRECOND_PREEMPT_PASS_LITERAL")" -eq 1

# Preempt-bracket census, at rest, in the shipped profile.
test "$(marker_count "$PREEMPT_CENSUS_PREFIX")" -eq 1
test "$(marker_count "$PREEMPT_CENSUS_PROD_LITERAL")" -eq 1

# #673 review, mi5: the production bracket's own release, censused separately
# from the shared census above (see its declaration comment).
test "$(marker_count "$PROD_BRACKET_RELEASE_PREFIX")" -eq 1
test "$(marker_count "$PROD_BRACKET_RELEASE_PROD_LITERAL")" -eq 1

# #673 review, MA6/R3-m4: pin the demoted timer-resolution window overrun at
# zero -- see its declaration comment above.
test "$(marker_count "$TIMER_RESOLUTION_WINDOW_EXCEEDED_PREFIX")" -eq 0

# #673: init designation and syscall-execution evidence -- construction,
# dispatch, and execution, each proven independently. See the header.
test "$(marker_count "$INIT_DESIGNATION_X86_PREFIX")" -eq 1
test "$(marker_count "$RING3_SYSCALL_LITERAL")" -eq 1

# #713, fixed: init survival/execution evidence. init reached its own
# first line, bsshd started AND reached listening (no longer just a
# graceful-failure warning -- SPAWN works on x86 now), and a dedicated
# spawn-smoke child (/bin/spawn_smoke_target) was observed exiting 0,
# reaped DIRECTLY by run_spawn_smoke() before start_bsshd() runs -- not
# through init's ordinary end-of-main() reap loop, which does not run
# until after run_boot_script()'s much slower chain completes (#722) --
# proving the full create+exec+run+exit+reap path end to end. The reap
# failure literal must also stay absent: a waitpid() error on the smoke
# child (e.g. it was silently never registered as init's child) now prints
# a distinct literal instead of a fabricated "exited (code 0)", so this
# pin is a genuine reap, not merely "spawn returned Ok". init's full
# boot-script chain (bsh --init-shell -> /etc/init.js) is still NOT pinned
# here -- see "INIT SURVIVAL EVIDENCE" above and #722.
test "$(marker_count "$INIT_FIRST_LINE_LITERAL")" -eq 1
test "$(marker_count "$INIT_BSSHD_WARNING_LITERAL")" -eq 0
test "$(marker_count "$BSSHD_STARTED_LITERAL")" -eq 1
test "$(marker_count "$BSSHD_LISTENING_LITERAL")" -eq 1
test "$(marker_count "$INIT_SPAWN_SMOKE_REAP_LITERAL")" -eq 1
test "$(marker_count "$INIT_SPAWN_SMOKE_REAP_FAILED_LITERAL")" -eq 0

# TTY-x86 port: light canary that the leg ran and reported no failure. The
# full 13-arm proof lives in the dedicated gate (see the literal comment
# above). Pinned on init's single-emit post-wait record rather than the
# oracle's own double-emitted COMPLETE line -- see that comment for why.
test "$(marker_count "$INIT_TTY_ORACLE_EXIT_LITERAL")" -eq 1
test "$(marker_count "$INIT_TTY_ORACLE_REAP_FAILED_LITERAL")" -eq 0
test "$(marker_count "$TTY_ORACLE_FAIL_LITERAL")" -eq 0

# #721, fixed: production exec. Four positive userspace markers proving a completed,
# argv-correct exec (not just "spawn returned Ok" -- exec_smoke_target's success marker is
# reachable only after the argv check and the deliberate yield loop both pass); three
# negative markers (anti-vacuity, #721 spec section 4) proving exec did not fail or corrupt
# argv; four kernel-side receipt oracles (K7) proving the scheduler-side commit that makes
# the post-exec preemption safe actually ran, with zero lock-order violations.
test "$(marker_count "$EXEC_SMOKE_LAUNCH_LITERAL")" -eq 1
test "$(marker_count "$EXEC_SMOKE_TARGET_ENTER_LITERAL")" -eq 1
test "$(marker_count "$EXEC_SMOKE_TARGET_OK_LITERAL")" -eq 1
test "$(marker_count "$EXEC_SMOKE_LAUNCHER_EXIT_LITERAL")" -eq 1
test "$(marker_count "$EXEC_SMOKE_SPAWN_FAILED_PREFIX")" -eq 0
test "$(marker_count "$EXEC_SMOKE_EXEC_FAILED_LITERAL")" -eq 0
test "$(marker_count "$EXEC_SMOKE_TARGET_ARGV_FAIL_PREFIX")" -eq 0
test "$(marker_count "$EXEC_LOCK_ORDER_FIRST_COMMIT_LITERAL")" -eq 1
test "$(marker_count "$EXEC_LOCK_ORDER_PM_HELD_LITERAL")" -eq 0
test "$(marker_count "$EXEC_LOCK_ORDER_UNPINNED_LITERAL")" -eq 0
test "$(marker_count "$EXEC_LOCK_ORDER_NO_SCHED_THREAD_LITERAL")" -eq 0

# #745, fixed: production fork. Positive markers proving a completed fork, the
# child's forced reschedule round trip, a genuine reap, and -- separately from
# the reap -- that the child exited CLEANLY with its own distinguishing code
# (review round 2, B1: a killed child is still reaped, and the userspace-fault
# kill path is silent in CRASH_MARKERS_PATTERN and FAULT_MARKERS). Then the two
# halves of precheck C3: `[COW FAULT #0] addr=` proving at least one CoW fault
# actually OCCURRED, and the isolation receipt proving the parent's own private
# copy survived the child's independent write (a broken refcount/isolation
# check corrupts shared memory silently rather than crashing, so that half has
# to be functional, not just "some fault line appeared"). Negative markers
# (anti-vacuity) prove fork did not fail, corrupt memory, or resume twice into
# the same branch. Both new pins were reddened by a mutation before being
# believed: docs/planning/745-x86-fork/serials/review-round-2/b1-mutation-child-exit-38-gate-FAIL.txt
# and docs/planning/745-x86-fork/serials/review-round-2/m2-mutation-cow-isolation-broken-gate-FAIL.txt
# claim-lint:ok: the two mutation runs named on the previous two lines.
test "$(marker_count "$FORK_SMOKE_LAUNCH_LITERAL")" -eq 1
test "$(marker_count "$FORK_SMOKE_CHILD_PREFIX")" -eq 1
test "$(marker_count "$FORK_SMOKE_COW_ISOLATION_OK_PREFIX")" -eq 1
test "$(marker_count "$FORK_SMOKE_COW_ISOLATION_CORRUPTED_PREFIX")" -eq 0
test "$(marker_count "$FORK_SMOKE_PARENT_REAPED_PREFIX")" -eq 1
test "$(marker_count "$FORK_SMOKE_PARENT_REAPED_CODE_LITERAL")" -eq 1
test "$(marker_count "$FORK_SMOKE_COW_FAULT_FIRST_LITERAL")" -eq 1
test "$(marker_count "$FORK_SMOKE_LAUNCHER_EXIT_LITERAL")" -eq 1
test "$(marker_count "$FORK_SMOKE_SPAWN_FAILED_PREFIX")" -eq 0
test "$(marker_count "$FORK_SMOKE_FORK_FAILED_PREFIX")" -eq 0
test "$(marker_count "$FORK_SMOKE_CHILD_UNEXPECTED_RETURN_LITERAL")" -eq 0
test "$(marker_count "$FORK_SMOKE_REAP_FAILED_PREFIX")" -eq 0

trap - ERR
# #673 review, B5: an anti-vacuity leg must never print a bare production PASS
# -- the measured arm has to be the shipping arm, or the verdict has to say so.
if [ -n "$X86_PROD_PROFILE_EXTRA_FEATURES" ]; then
    echo "PASS (feature-mutated build, features=$X86_PROD_PROFILE_EXTRA_FEATURES; NOT the shipped profile)"
else
    echo "PASS: x86 production profile reached steady state with the teardown census at rest"
fi
print_observed_values
echo "  console prompt count over ${LIVENESS_WINDOW_SECONDS}s: $PROMPT_BEFORE -> $PROMPT_AFTER"
echo "  (informational) total serial bytes at exit: $(serial_bytes)"
