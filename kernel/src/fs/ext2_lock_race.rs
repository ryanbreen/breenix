//! #728 gate-observable repro oracle — ext2 lock discipline.
//!
//! Test profile only: this module and its single call site (one per
//! architecture main) are behind the `ext2_lock_race` feature.
//!
//! # What this constructs
//!
//! The #728 pre-check's evidence (the preserved repro serial log) showed a
//! READER holding `root_fs_read()` parked for real block-device I/O while a
//! WRITER spun forever inside `root_fs_write()`'s acquisition, on x86 `-smp 1`.
//! On aarch64's multi-CPU profile the same shape needs enough concurrent
//! spinning contenders to occupy every CPU, because a parked holder's own CPU
//! is otherwise free for the scheduler to dispatch its eventual wakeup.
//!
//! This leg constructs that shape directly, in-kernel, deterministically:
//!
//!  1. A "holder" kthread acquires `root_fs_read()` (or `home_fs_read()`)
//!     under an explicit `preempt_disable()` bracket — mirroring exactly what
//!     a real syscall dispatch does around a filesystem call — and then
//!     deliberately parks *while still holding the guard* via
//!     `Completion::wait_timeout_uninterruptible()` on a scratch `Completion`
//!     that is never completed, so the wait always runs its full deadline.
//!     This reproduces "guard held across a park" using the exact primitive
//!     the real bug's evidence cites (`task/completion.rs`), without needing
//!     any real device I/O or fault injection.
//!  2. After a short head start (long enough for the holder to have acquired
//!     its guard — a handful of instructions — not a race-sensitive value:
//!     the hold window below is three seconds), `CONTENDER_COUNT` "contender"
//!     kthreads each attempt `root_fs_write()`/`home_fs_write()` under the
//!     same preempt-disable bracket and, on success, create a small
//!     directory (the write-family's own #728 shape — `sys_mkdir` was the
//!     observed repro's own call site). `CONTENDER_COUNT` matches each gate
//!     profile's known `-smp` value (1 on x86, 4 on aarch64) rather than
//!     auto-detecting it, keeping the oracle deterministic instead of
//!     environment-sensitive. On aarch64 each contender is pinned to a
//!     distinct CPU via `kthread_run_on_cpu_for_test` so the construction
//!     does not depend on the default scheduler's placement
//!     choices to occupy every CPU.
//!
//! # Why the observer is the spinner, not a userspace watchdog
//!
//! On unfixed lock code, the pathological case this leg constructs is a true
//! livelock: every CPU ends up occupied by a non-yielding contender, so
//! nothing else — including this leg's own driver thread, which is blocked
//! in `kthread_join()` waiting for the holder — can run again. There is
//! nothing to reach a "the test hung" verdict except the spin itself, which
//! is why the observability lives in `ext2::ext2_lock_spin_stalls()` (see
//! that module): it is read here as a before/after counter around the race,
//! not printed by a watchdog that would never get a CPU to run on.
//!
//! # Anti-vacuity
//!
//! Run against the observer-only commit (spin instrumented, no park path),
//! this leg's own `EXT2_LOCK_SPIN_STALLS` counter reliably increments and
//! `[LOCKRACE:...:race:verdict=FAIL...]` prints — proving the harness
//! actually forces the collision rather than relying on incidental boot
//! concurrency. Run against the fix, the same construction resolves via the
//! park path and reports PASS. Both runs exercise the identical harness; only
//! the lock code underneath differs. See `docker/qemu/run-ext2-lock-race-gate.sh`.

use crate::task::completion::Completion;
use crate::task::kthread::{kthread_join, kthread_run, KthreadHandle};

#[cfg(target_arch = "aarch64")]
use crate::task::kthread::kthread_run_on_cpu_for_test;

/// How long the holder keeps the guard held while parked. Long relative to
/// ordinary lock hold times so the head start below never has to be
/// race-sensitive: any contender that starts within three seconds of the
/// holder acquiring its guard is guaranteed to observe it held.
const HOLD_TIMEOUT_NS: u64 = 3_000_000_000;

/// Contenders spun up per filesystem, matching each gate profile's known
/// `-smp` value (not auto-detected — see module docs).
#[cfg(target_arch = "aarch64")]
const CONTENDER_COUNT: usize = 4;
#[cfg(not(target_arch = "aarch64"))]
const CONTENDER_COUNT: usize = 1;

/// Head start given to the holder before contenders are spawned, so
/// contenders reliably observe the guard held rather than racing the
/// holder's own (sub-millisecond) acquisition. Not race-sensitive: see
/// `HOLD_TIMEOUT_NS` above.
const HEAD_START_MS: u64 = 100;

static HOLDER_SCRATCH: Completion = Completion::new();

#[inline]
fn preempt_disable() {
    #[cfg(target_arch = "aarch64")]
    crate::per_cpu_aarch64::preempt_disable();
    #[cfg(not(target_arch = "aarch64"))]
    crate::per_cpu::preempt_disable();
}

#[inline]
fn preempt_enable() {
    #[cfg(target_arch = "aarch64")]
    crate::per_cpu_aarch64::preempt_enable();
    #[cfg(not(target_arch = "aarch64"))]
    crate::per_cpu::preempt_enable();
}

fn now_ns() -> u64 {
    let (secs, nanos) = crate::time::get_monotonic_time_ns();
    secs as u64 * 1_000_000_000 + nanos as u64
}

/// Busy-wait (yielding the CPU via halt between checks — this driver thread
/// is not itself part of the race) for `ms` milliseconds.
fn sleep_ms(ms: u64) {
    let deadline = now_ns() + ms * 1_000_000;
    while now_ns() < deadline {
        crate::arch_halt();
    }
}

fn holder_fn(is_home: bool) {
    preempt_disable();
    if is_home {
        let guard = crate::fs::ext2::home_fs_read();
        let _ = HOLDER_SCRATCH.wait_timeout_uninterruptible(1, HOLD_TIMEOUT_NS);
        drop(guard);
    } else {
        let guard = crate::fs::ext2::root_fs_read();
        let _ = HOLDER_SCRATCH.wait_timeout_uninterruptible(1, HOLD_TIMEOUT_NS);
        drop(guard);
    }
    preempt_enable();
}

fn contender_fn(is_home: bool, index: usize) {
    preempt_disable();
    let prefix = if is_home { "home" } else { "root" };
    let path = alloc::format!("/lockrace_{}_{}", prefix, index);
    if is_home {
        let mut guard = crate::fs::ext2::home_fs_write();
        if let Some(fs) = guard.as_mut() {
            let _ = fs.create_directory(&path, 0o755);
        }
    } else {
        let mut guard = crate::fs::ext2::root_fs_write();
        if let Some(fs) = guard.as_mut() {
            let _ = fs.create_directory(&path, 0o755);
        }
    }
    preempt_enable();
}

#[cfg(target_arch = "aarch64")]
fn spawn_pinned<F>(func: F, name: &str, cpu: usize) -> Option<KthreadHandle>
where
    F: FnOnce() + Send + 'static,
{
    match kthread_run_on_cpu_for_test(func, name, cpu) {
        Ok(h) => Some(h),
        Err(e) => {
            crate::serial_println!("[LOCKRACE:spawn:verdict=FAIL:detail={:?}]", e);
            None
        }
    }
}

fn spawn_plain<F>(func: F, name: &str) -> Option<KthreadHandle>
where
    F: FnOnce() + Send + 'static,
{
    match kthread_run(func, name) {
        Ok(h) => Some(h),
        Err(e) => {
            crate::serial_println!("[LOCKRACE:spawn:verdict=FAIL:detail={:?}]", e);
            None
        }
    }
}

/// Outcome of a single filesystem's race construction. `Fail` covers every
/// non-clean outcome: a spin stall, a setup failure, and — see `Fail`'s
/// `no-park-observed` detail below — a construction that resolved without
/// ever entering the fix's own park path, which review finding B4 flagged
/// as indistinguishable from "got lucky" if left unchecked.
enum RaceOutcome {
    Pass,
    Fail,
}

/// Run the race construction once against one filesystem (root or home).
/// Always prints exactly one `[LOCKRACE:{label}:race:verdict=...]` line
/// before returning (review finding B4: "require a per-race verdict line
/// to exist") and always reflects a non-clean run — including a spawn
/// failure that never got as far as constructing a race at all — as `Fail`,
/// never as a silent pass.
fn run_one(is_home: bool) -> RaceOutcome {
    let label = if is_home { "HOME" } else { "ROOT" };
    let stalls_before = crate::fs::ext2::ext2_lock_spin_stalls();
    let parks_before = crate::fs::ext2::ext2_lock_parks();
    // #748: re-arm the one-shot EXT2_LOCK_PARK_FIRST marker so a park
    // during THIS race attempt prints again, even if some earlier
    // boot_tests activity already consumed the boot's very first one (see
    // `ext2_reset_lock_park_first_marker()`'s doc comment) -- this is what
    // lets a gate script observe "this specific holder/contender
    // construction parked" without waiting for run_one() itself to return.
    crate::fs::ext2::ext2_reset_lock_park_first_marker();

    HOLDER_SCRATCH.reset();

    // Deliberately unpinned on both arches, including aarch64:
    // `kthread_run_on_cpu_for_test`'s placement is a single-slot-per-CPU
    // registration (`BOOT_TEST_CPU_AFFINITY`), so pinning the holder to the
    // same CPU as one of the contenders below would clobber that
    // contender's own placement. The holder's initial CPU doesn't need to
    // be deterministic for the construction to work — it parks almost
    // immediately, freeing whatever CPU it started on.
    let holder = spawn_plain(move || holder_fn(is_home), "lockrace_holder");

    let Some(holder) = holder else {
        crate::serial_println!("[LOCKRACE:{}:setup:verdict=FAIL:detail=holder-spawn-failed]", label);
        crate::serial_println!("[LOCKRACE:{}:race:verdict=FAIL:detail=setup-failed]", label);
        return RaceOutcome::Fail;
    };

    sleep_ms(HEAD_START_MS);

    let mut contenders = alloc::vec::Vec::with_capacity(CONTENDER_COUNT);
    let mut setup_failed = false;
    for i in 0..CONTENDER_COUNT {
        // CPU 0 is excluded from pinning: it hosts this driver's own
        // kthread_join polling loop below, and empirically
        // kthread_run_on_cpu_for_test's single-slot-per-CPU placement
        // (BOOT_TEST_CPU_AFFINITY) never dispatches a second thread pinned
        // there while this driver occupies it — observed directly (the
        // contender pinned to CPU 0 never printed even its own start line).
        // Contender 0 is spawned unpinned instead, landing wherever the
        // default scheduler puts it; contenders 1..CONTENDER_COUNT are
        // pinned to CPUs 1..CONTENDER_COUNT so aarch64's -smp 4 profile
        // still gets deterministic coverage of CPUs 1-3 (empirically
        // already sufficient to reproduce the #728 shape on unfixed code —
        // see the anti-vacuity notes in the gate script).
        #[cfg(target_arch = "aarch64")]
        let handle = if i == 0 {
            spawn_plain(move || contender_fn(is_home, i), "lockrace_contender")
        } else {
            spawn_pinned(move || contender_fn(is_home, i), "lockrace_contender", i)
        };
        #[cfg(not(target_arch = "aarch64"))]
        let handle = spawn_plain(move || contender_fn(is_home, i), "lockrace_contender");

        match handle {
            Some(h) => contenders.push(h),
            None => {
                crate::serial_println!(
                    "[LOCKRACE:{}:setup:verdict=FAIL:detail=contender-spawn-failed:index={}]",
                    label,
                    i
                );
                setup_failed = true;
                break;
            }
        }
    }

    // Join whatever was actually spawned either way, so a partial-setup
    // failure still releases the holder's guard (it parks for at most
    // HOLD_TIMEOUT_NS regardless) rather than leaking a parked kthread past
    // this leg's own return — review finding M6 flagged exactly this shape
    // of teardown hazard for the general case; this driver must not create
    // an instance of it on its own setup-failure path.
    for c in &contenders {
        let _ = kthread_join(c);
    }
    let _ = kthread_join(&holder);

    if setup_failed {
        crate::serial_println!("[LOCKRACE:{}:race:verdict=FAIL:detail=setup-failed]", label);
        return RaceOutcome::Fail;
    }

    let stalls_after = crate::fs::ext2::ext2_lock_spin_stalls();
    let parks_after = crate::fs::ext2::ext2_lock_parks();
    let parks = parks_after - parks_before;

    if stalls_after > stalls_before {
        crate::serial_println!(
            "[LOCKRACE:{}:race:verdict=FAIL:detail=spin-stall-observed:stalls={}:parks={}]",
            label,
            stalls_after - stalls_before,
            parks
        );
        RaceOutcome::Fail
    } else if parks == 0 {
        // The holder's 3s hold plus the 100ms head start guarantees every
        // contender's write-side upgrade attempt must wait for the reader
        // to drain, so a genuine, fixed-code run always parks at least
        // once. Zero parks means the fix's new code path was never
        // actually entered — "no stall" alone does not distinguish that
        // from a real pass (review finding B4).
        crate::serial_println!(
            "[LOCKRACE:{}:race:verdict=FAIL:detail=no-park-observed:parks=0]",
            label
        );
        RaceOutcome::Fail
    } else {
        crate::serial_println!(
            "[LOCKRACE:{}:race:verdict=PASS:detail=no-spin-stall:parks={}]",
            label,
            parks
        );
        RaceOutcome::Pass
    }
}

/// Entry point, called once per boot from each architecture's main after
/// both filesystems are mounted and the scheduler/timer/SMP are all up
/// (kthreads and real parking both require a running scheduler).
pub fn run_ext2_lock_race_leg() {
    if !crate::fs::ext2::is_mounted() {
        crate::serial_println!("[LOCKRACE:ROOT:setup:verdict=FAIL:detail=ext2-not-mounted]");
        crate::serial_println!("[LOCKRACE:ROOT:race:verdict=FAIL:detail=ext2-not-mounted]");
        crate::serial_println!("[LOCKRACE:COMPLETE:pass=0:fail=1]");
        return;
    }

    let mut pass = 0u32;
    let mut fail = 0u32;

    // Every leg is scored solely from run_one()'s own returned outcome —
    // no separate before/after check is re-derived here, so there is
    // exactly one place that decides pass/fail and it is the same place
    // that always prints the matching `:race:verdict=` line (review
    // finding B4a: a setup failure must count as `fail`, not a silent
    // pass by virtue of never having reached a stall check).
    match run_one(false) {
        RaceOutcome::Pass => pass += 1,
        RaceOutcome::Fail => fail += 1,
    }

    if crate::fs::ext2::is_home_mounted() {
        match run_one(true) {
            RaceOutcome::Pass => pass += 1,
            RaceOutcome::Fail => fail += 1,
        }
    } else {
        crate::serial_println!("[LOCKRACE:HOME:setup:verdict=SKIP:detail=home-not-mounted]");
    }

    crate::serial_println!("[LOCKRACE:COMPLETE:pass={}:fail={}]", pass, fail);
}
