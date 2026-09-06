//! #766: how long a thread waits between its sleep deadline passing and the
//! CPU actually being handed to it.
//!
//! # What this measures
//!
//! One kthread sleeps for `SLEEP_MS` against an absolute monotonic deadline it
//! computes itself, while `PEERS` CPU-bound kthreads are runnable. The number
//! reported is `wake_instant - deadline`, both read with
//! `crate::time::get_monotonic_time_ns()` (millisecond units here are #767's,
//! i.e. real milliseconds, not raw ticks), and `deadline` is the SAME value
//! handed to `Scheduler::block_current_for_timer`, so the interval is anchored
//! to the kernel's own deadline rather than to a second, later clock read.
//!
//! # The barrier, and why it is not optional
//!
//! The threads are created before any of them may spin. Creating a kernel
//! thread is not cheap -- it allocates and maps a kernel stack -- and the boot
//! thread doing the creating is itself preemptible, so a peer that is runnable
//! while the later peers are being created takes a full quantum per round away
//! from the creation. Two earlier versions of this file measured that cost on
//! the x86 boot-test gate and are recorded in the round doc,
//! docs/planning/green-program/timekeeping/766-TIMER-WAKE-DISPATCH-2026-09-06.md
//! section "the barrier": spinning peers cost 16 s of guest time across the 8
//! creations, and peers parked on a `halt` loop -- which does NOT leave the
//! ready queue, so they still hold a quantum each -- cost 362 s.
//!
//! So a peer parks with `kthread_park()`, which blocks it and takes it out of
//! the ready queue, until the boot thread has created the 9 threads and calls
//! `kthread_unpark`. The sleeper then waits for `PEERS_SPINNING` to reach
//! `PEERS` before starting its sleep, so the wait it measures is a wait behind
//! peers that are actually running. `setup_ms` and `window_ms` on the marker are
//! those two phases, and `peers_spinning` is the fact the verdict depends on.
//!
//! # Why the peers do not yield
//!
//! The quantity #766 measures is the woken thread's wait for a dispatch turn
//! behind threads that each hold the CPU for a full quantum. A peer that
//! yielded would give the CPU back before its quantum expired, and the round
//! this oracle has to reproduce would not happen. So the peers spin without
//! yielding, exactly like any compute-bound kernel thread, and are preempted by
//! the timer on the same quantum policy as anything else.
//!
//! # The bound
//!
//! With the fix in `Scheduler::wake_expired_timers` (a late wake is enqueued at
//! the HEAD of its target ready queue), the wait is bounded by the running
//! thread's REMAINING quantum plus one tick of granularity, because the pass
//! that detects the expiry is `schedule()`'s own and its selection loop pops the
//! front of that queue a few lines later. That is
//! `QUANTUM_TICKS * MS_PER_TICK + MS_PER_TICK`: 55 ms on x86 (10 ticks of 5 ms,
//! plus one 5 ms tick) and 11 ms on aarch64 (10 ticks of 1 ms, plus one 1 ms
//! tick). `BOUND_MS` is deliberately larger than either, because both gates run
//! the guest under emulation where an emulated periodic timer can be delivered
//! late; the extra is a jitter allowance and is NOT part of the mechanism
//! claim. The marker prints `quantum_ms` and `round_ms` so a reader can redo the
//! arithmetic from the line itself.
//!
//! Without the fix the same leg waits a full round of the peers' quanta:
//! `round_ms` is that figure, and it is printed whether the run passes or fails.
//!
//! # Not claimed
//!
//! * This oracle does not measure a userspace `nanosleep`. It measures the
//!   kernel-side primitive that path blocks on.
//! * It does not separate the detection term (deadline to next `schedule()`)
//!   from the queue-position term. It bounds their SUM, which is the quantity a
//!   sleeper experiences.
//! * `round_ms` is arithmetic (`PEERS * QUANTUM_MS`), not a measurement. The
//!   real round also carries whatever else is runnable in that boot window.
//! * On aarch64 the leg runs against `MAX_CPUS` ready queues and a shorter
//!   quantum, so a green reading there is a regression guard, not evidence
//!   about the x86 mechanism.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::serial_println;
use crate::task::kthread::{kthread_join, kthread_park, kthread_run, kthread_unpark};
use crate::task::scheduler;
use crate::task::thread::ThreadState;
use crate::{arch_disable_interrupts, arch_enable_interrupts, arch_halt_with_interrupts};

/// Architecture tag on the marker, so an x86 gate's assertion cannot be
/// satisfied by an aarch64 emission.
const ARCH_TAG: &str = if cfg!(target_arch = "x86_64") {
    "x86"
} else {
    "aarch64"
};

/// Runnable CPU-bound threads the sleeper has to get past.
const PEERS: usize = 8;

/// The sleep the oracle measures.
const SLEEP_MS: u64 = 10;

/// `TIME_QUANTUM` as both timer interrupt handlers declare it, in ticks.
///
/// Duplicated here rather than imported: the two declarations live in
/// `kernel/src/interrupts/timer.rs` and
/// `kernel/src/arch_impl/aarch64/timer_interrupt.rs`, both private, and the
/// x86 one is a Tier-1 file that this change has no reason to edit. The
/// duplication is pinned by `tests/timer_wake_dispatch_structure.rs`, which
/// reads both files and fails if either literal stops matching this one.
const QUANTUM_TICKS: u64 = 10;

/// Milliseconds of a full scheduling quantum on this build.
const QUANTUM_MS: u64 = QUANTUM_TICKS * crate::time::timer::MS_PER_TICK;

/// The wait a tail enqueue costs: the peers ahead of the woken thread each run
/// a quantum before it is selected. Printed for the reader, not asserted.
const ROUND_MS: u64 = PEERS as u64 * QUANTUM_MS;

/// Gate bound. Mechanism bound is `QUANTUM_MS + MS_PER_TICK` (55 ms on x86,
/// 11 ms on aarch64); the remainder is an emulated-timer jitter allowance.
const BOUND_MS: u64 = 100;

/// Backstops. No wait in this oracle is unbounded: each one carries a
/// monotonic deadline, and whether a backstop fired is reported on the marker
/// rather than swallowed. These are "the boot is broken" budgets, not
/// measurement participants -- the barrier is what keeps the measured window
/// short.
const START_WAIT_BACKSTOP_MS: u64 = 120_000;
const SPINNING_WAIT_BACKSTOP_MS: u64 = 30_000;
const PEER_SPIN_BACKSTOP_MS: u64 = 30_000;
const SLEEP_BACKSTOP_MS: u64 = 20_000;
const CLEANUP_BACKSTOP_MS: u64 = 30_000;

static MEASURE_OPEN: AtomicBool = AtomicBool::new(false);
static PEERS_RUN: AtomicBool = AtomicBool::new(false);
static PEERS_STARTED: AtomicU32 = AtomicU32::new(0);
static PEERS_SPINNING: AtomicU32 = AtomicU32::new(0);
static PEERS_EXITED: AtomicU32 = AtomicU32::new(0);
static BACKSTOPS: AtomicU32 = AtomicU32::new(0);
static OVERRUN_MS: AtomicU64 = AtomicU64::new(u64::MAX);
static WAKE_ENQUEUES: AtomicU64 = AtomicU64::new(0);
static SETUP_MS: AtomicU64 = AtomicU64::new(0);
static WINDOW_MS: AtomicU64 = AtomicU64::new(0);
static OPEN_NS: AtomicU64 = AtomicU64::new(0);
static MEASURED: AtomicBool = AtomicBool::new(false);

fn now_ns() -> u64 {
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    seconds.saturating_mul(1_000_000_000).saturating_add(nanos)
}

/// Wait with `halt` until `ready()` or the deadline passes. Returns false if
/// the deadline ended the wait, and counts that as a backstop.
///
/// A `halt` does NOT take the waiting thread out of the ready queue -- it still
/// holds its quantum, waking on each tick -- so this is used only for the two
/// short waits inside the measured window, not across thread creation.
fn wait_until<F: Fn() -> bool>(ready: F, budget_ms: u64) -> bool {
    let deadline = now_ns().saturating_add(budget_ms.saturating_mul(1_000_000));
    loop {
        if ready() {
            return true;
        }
        if now_ns() >= deadline {
            BACKSTOPS.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        arch_halt_with_interrupts();
    }
}

/// A CPU-bound peer: parked until the barrier opens, then runnable for the
/// whole measurement window, does not yield, and leaves when the sleeper says
/// so or its own backstop expires.
fn peer_body() {
    // Kernel threads start with interrupts disabled; a peer that cannot be
    // preempted would hold the CPU forever instead of taking one quantum.
    unsafe { arch_enable_interrupts() };
    PEERS_STARTED.fetch_add(1, Ordering::AcqRel);
    // Blocks and leaves the ready queue, so the creations still to come run at
    // full speed. Re-parks on a spurious unpark; the boot thread sets
    // MEASURE_OPEN before it unparks anyone.
    while !MEASURE_OPEN.load(Ordering::Acquire) {
        kthread_park();
    }
    PEERS_SPINNING.fetch_add(1, Ordering::AcqRel);
    let backstop = now_ns().saturating_add(PEER_SPIN_BACKSTOP_MS * 1_000_000);
    while PEERS_RUN.load(Ordering::Acquire) {
        if now_ns() >= backstop {
            BACKSTOPS.fetch_add(1, Ordering::Relaxed);
            break;
        }
        for _ in 0..256 {
            core::hint::spin_loop();
        }
    }
    PEERS_EXITED.fetch_add(1, Ordering::AcqRel);
}

/// The measured thread. Created last, so it does not park: by the time it first
/// runs, the creations are done.
fn sleeper_body() {
    unsafe { arch_enable_interrupts() };
    wait_until(
        || MEASURE_OPEN.load(Ordering::Acquire),
        START_WAIT_BACKSTOP_MS,
    );
    // Do not start measuring until `PEERS_SPINNING` reaches `PEERS`; a peer
    // still parked is not competition and would understate the wait this
    // oracle exists to bound.
    wait_until(
        || PEERS_SPINNING.load(Ordering::Acquire) as usize >= PEERS,
        SPINNING_WAIT_BACKSTOP_MS,
    );

    let Some(tid) = scheduler::current_thread_id() else {
        PEERS_RUN.store(false, Ordering::Release);
        return;
    };

    let enqueues_before = scheduler::ENQUEUE_TIMER_WAKE.load(Ordering::Relaxed);
    let deadline = now_ns().saturating_add(SLEEP_MS * 1_000_000);
    let give_up = deadline.saturating_add(SLEEP_BACKSTOP_MS * 1_000_000);

    scheduler::with_scheduler(|sched| sched.block_current_for_timer(deadline));
    scheduler::yield_current();

    loop {
        let still_blocked = scheduler::with_scheduler(|sched| {
            sched
                .get_thread(tid)
                .is_some_and(|thread| thread.state == ThreadState::BlockedOnTimer)
        })
        .unwrap_or(false);
        if !still_blocked {
            break;
        }
        if now_ns() >= give_up {
            BACKSTOPS.fetch_add(1, Ordering::Relaxed);
            break;
        }
        arch_halt_with_interrupts();
    }

    let woke = now_ns();
    WAKE_ENQUEUES.store(
        scheduler::ENQUEUE_TIMER_WAKE
            .load(Ordering::Relaxed)
            .wrapping_sub(enqueues_before),
        Ordering::Release,
    );
    OVERRUN_MS.store(woke.saturating_sub(deadline) / 1_000_000, Ordering::Release);
    WINDOW_MS.store(
        woke.saturating_sub(OPEN_NS.load(Ordering::Acquire)) / 1_000_000,
        Ordering::Release,
    );
    MEASURED.store(true, Ordering::Release);
    PEERS_RUN.store(false, Ordering::Release);
}

/// Emit the verdict line. `reason` is empty on a run that measured something.
fn emit(overrun_ms: u64, spawned: usize, reason: &str) {
    let measured = MEASURED.load(Ordering::Acquire);
    let backstops = BACKSTOPS.load(Ordering::Relaxed);
    let wake_enqueues = WAKE_ENQUEUES.load(Ordering::Acquire);
    let started = PEERS_STARTED.load(Ordering::Acquire);
    let spinning = PEERS_SPINNING.load(Ordering::Acquire);
    // A run passes only if it MEASURED a wake (so a boot that did not reach
    // the wake path cannot report a small number), if the wake went through
    // the timer-wake enqueue site, if `PEERS_SPINNING` reached `PEERS` when the
    // sleep started, if the backstop count is 0, and if the overrun is inside
    // the bound.
    let pass = measured
        && reason.is_empty()
        && spawned == PEERS
        && started as usize == PEERS
        && spinning as usize == PEERS
        && wake_enqueues >= 1
        && backstops == 0
        && overrun_ms <= BOUND_MS;
    serial_println!(
        "[TIMER_WAKE_LATENCY_ORACLE:{}:sleep_ms={}:peers={}:overrun_ms={}:bound_ms={}:quantum_ms={}:round_ms={}:wake_enqueues={}:peers_started={}:peers_spinning={}:backstops={}:setup_ms={}:window_ms={}:measured={}:{}{}]",
        ARCH_TAG,
        SLEEP_MS,
        PEERS,
        overrun_ms,
        BOUND_MS,
        QUANTUM_MS,
        ROUND_MS,
        wake_enqueues,
        started,
        spinning,
        backstops,
        SETUP_MS.load(Ordering::Acquire),
        WINDOW_MS.load(Ordering::Acquire),
        u8::from(measured),
        if pass { "PASS" } else { "FAIL" },
        reason,
    );
}

/// Run the leg once and print `[TIMER_WAKE_LATENCY_ORACLE:...]`.
///
/// Restores the interrupt flag it found, so the caller's boot sequence keeps
/// the state it had; the peers and the sleeper need interrupts on to be
/// preempted at all.
pub fn run() {
    MEASURE_OPEN.store(false, Ordering::Release);
    PEERS_RUN.store(true, Ordering::Release);
    PEERS_STARTED.store(0, Ordering::Release);
    PEERS_SPINNING.store(0, Ordering::Release);
    PEERS_EXITED.store(0, Ordering::Release);
    BACKSTOPS.store(0, Ordering::Release);
    OVERRUN_MS.store(u64::MAX, Ordering::Release);
    WAKE_ENQUEUES.store(0, Ordering::Release);
    SETUP_MS.store(0, Ordering::Release);
    WINDOW_MS.store(0, Ordering::Release);
    MEASURED.store(false, Ordering::Release);

    let interrupts_were_enabled = crate::arch_interrupts_enabled();
    if !interrupts_were_enabled {
        unsafe { arch_enable_interrupts() };
    }
    let started_ns = now_ns();

    let mut peers = Vec::with_capacity(PEERS);
    for _ in 0..PEERS {
        match kthread_run(peer_body, "t766_peer") {
            Ok(handle) => peers.push(handle),
            Err(_) => break,
        }
    }
    let spawned = peers.len();

    let sleeper = if spawned == PEERS {
        kthread_run(sleeper_body, "t766_sleeper").ok()
    } else {
        None
    };

    if sleeper.is_some() {
        // The peers have to have reached the barrier before it opens, so their
        // spin windows start at the same point.
        wait_until(
            || PEERS_STARTED.load(Ordering::Acquire) as usize >= PEERS,
            START_WAIT_BACKSTOP_MS,
        );
        let open = now_ns();
        OPEN_NS.store(open, Ordering::Release);
        SETUP_MS.store(open.saturating_sub(started_ns) / 1_000_000, Ordering::Release);
        MEASURE_OPEN.store(true, Ordering::Release);
        // Unpark in a retry loop: a peer that entered `kthread_park()` after a
        // single unpark would block with nobody left to wake it. Repeating the
        // unpark until the peer reports itself spinning closes that window
        // without needing the park and the unpark to be ordered.
        let deadline = now_ns().saturating_add(SPINNING_WAIT_BACKSTOP_MS * 1_000_000);
        loop {
            if PEERS_SPINNING.load(Ordering::Acquire) as usize >= PEERS {
                break;
            }
            for handle in peers.iter() {
                kthread_unpark(handle);
            }
            if now_ns() >= deadline {
                BACKSTOPS.fetch_add(1, Ordering::Relaxed);
                break;
            }
            arch_halt_with_interrupts();
        }
    }

    if let Some(handle) = sleeper.as_ref() {
        let _ = kthread_join(handle);
    }
    // Whatever happened above, the peers are told to stop and unparked before
    // they are joined, or a failed sleeper leaves eight threads behind and the
    // join below waits on a thread with no remaining waker.
    MEASURE_OPEN.store(true, Ordering::Release);
    PEERS_RUN.store(false, Ordering::Release);
    let cleanup_deadline = now_ns().saturating_add(CLEANUP_BACKSTOP_MS * 1_000_000);
    loop {
        if PEERS_EXITED.load(Ordering::Acquire) as usize >= spawned {
            break;
        }
        for handle in peers.iter() {
            kthread_unpark(handle);
        }
        if now_ns() >= cleanup_deadline {
            BACKSTOPS.fetch_add(1, Ordering::Relaxed);
            break;
        }
        arch_halt_with_interrupts();
    }
    for handle in peers.iter() {
        let _ = kthread_join(handle);
    }

    if !interrupts_were_enabled {
        unsafe { arch_disable_interrupts() };
    }

    let reason = if spawned != PEERS {
        ":reason=peer_spawn_failed"
    } else if sleeper.is_none() {
        ":reason=sleeper_spawn_failed"
    } else {
        ""
    };
    let overrun_ms = OVERRUN_MS.load(Ordering::Acquire);
    emit(
        if overrun_ms == u64::MAX { 0 } else { overrun_ms },
        spawned,
        reason,
    );
}
