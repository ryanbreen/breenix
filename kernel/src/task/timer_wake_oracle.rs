//! #766: how long a thread waits between its sleep deadline passing and the
//! CPU actually being handed to it, and what that costs the CPU-bound threads
//! it is dispatched ahead of.
//!
//! # The two quantities
//!
//! `PEERS` CPU-bound kthreads are runnable for the whole measured window while
//! `REARMERS` kthreads each sleep `REARMS` times in a row against absolute
//! monotonic deadlines they compute themselves. The marker reports:
//!
//! * `overrun_ms` -- the WORST `wake_instant - deadline` of the
//!   `REARMERS * REARMS` sleeps the leg performs. Both instants are read with
//!   `crate::time::get_monotonic_time_ns()` (millisecond units here are #767's,
//!   i.e. real milliseconds, not raw ticks), and `deadline` is the SAME value
//!   handed to `Scheduler::block_current_for_timer`, so the interval is
//!   anchored to the kernel's own deadline rather than to a second, later clock
//!   read. This is the quantity #766 is about.
//! * `peer_max_gap_ms` -- the WORST interval any CPU-bound peer spent off the
//!   CPU during that same window, measured by each peer from consecutive clock
//!   reads in its own spin loop. This is the quantity the head enqueue could
//!   cost, and it is why the re-armers re-arm rather than sleeping once: a
//!   thread that sleeps, wakes, and immediately sleeps again is promoted to the
//!   head of the ready queue on each of its wakes, which is the repeated
//!   displacement a single sleep cannot exercise.
//!
//! # The barrier, and why it is not optional
//!
//! The threads are created before any of them may run. Creating a kernel
//! thread is not cheap -- it allocates and maps a kernel stack -- and the boot
//! thread doing the creating is itself preemptible, so a thread that is
//! runnable while the later ones are being created takes a full quantum per
//! round away from the creation. Two earlier versions of this file measured
//! that cost on the x86 boot-test gate and are recorded in the round doc,
//! docs/planning/green-program/timekeeping/766-TIMER-WAKE-DISPATCH-2026-09-06.md
//! section "the barrier": spinning peers cost 16 s of guest time across the 8
//! creations, and peers parked on a `halt` loop -- which does NOT leave the
//! ready queue, so they still hold a quantum each -- cost 362 s.
//!
//! So each PEER parks with `kthread_park()`, which blocks it and takes it out
//! of the ready queue, until the boot thread has created the 12 threads and
//! calls `kthread_unpark`. The re-armers are created last and do not park:
//! at most three of them are runnable during the last three creations, which
//! is cheap, and it keeps them out of a park/unpark handshake that the join
//! waiting on them could not time out on. Each re-armer waits for
//! `PEERS_SPINNING` to reach `PEERS` before its first sleep, so the wait it
//! measures is a wait behind peers that are actually running. `setup_ms` and
//! `window_ms` on the marker are those two phases, and `peers_spinning` is a
//! fact the verdict depends on.
//!
//! # Each wait in the leg carries a deadline
//!
//! `kthread_join` halts until the thread it names exits, with no deadline of
//! its own, so a leg that joined a thread which stopped being dispatched would
//! hang the boot with no marker printed -- which is how the first round-2
//! mutation run of this leg ended, 1 run of 1. The leg therefore waits on the counters the two classes
//! publish (`REARMERS_DONE`, `PEERS_EXITED`), each with a budget, and joins a
//! class only once its counter says the class is finished. `peers_exited` and
//! `rearmers` are on the marker for the same reason: a class that stalls is
//! then named by the verdict line rather than swallowed by a halt.
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
//! # The two bounds
//!
//! With the fix in `Scheduler::wake_expired_timers` (a late wake is enqueued at
//! the HEAD of its target ready queue), a wake's wait is bounded by the running
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
//! Without the fix the same sleeps wait a full round of the peers' quanta:
//! `round_ms` is that figure, and it is printed whether the run passes or fails.
//!
//! `PEER_GAP_BOUND_MS` is a STARVATION ceiling, not a latency certification. It
//! is `(PEERS + REARMERS + 2) * QUANTUM_MS * 4`, i.e. four times the round a
//! peer would wait if each thread in the leg held a full quantum; the factor of
//! four is a deliberate allowance and no claim is made that a reading near the
//! ceiling would be acceptable. What the field is here to catch is a peer that
//! stops being dispatched at all while wakes keep being promoted ahead of it.
//! Section "cross-class fairness" of the RCA derives what the promotion policy
//! does and does not bound.
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
//! * `peer_max_gap_ms` is read at one point of the design space:
//!   `REARMERS` re-armers at a `SLEEP_MS` period against `PEERS` CPU-bound
//!   peers. It is not a sweep over re-armer counts or periods, and it does not
//!   measure the inflation factor the RCA derives.
//! * `peer_max_gap_ms` is a maximum over samples a peer takes between spin
//!   batches, so an interval that starts or ends outside the window is counted
//!   only in part. It is a lower bound on the true worst gap.
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

/// Runnable CPU-bound threads a wake has to get past.
///
/// `PEERS + REARMERS + 1` is 10, one more than the boot thread plus the 9 the
/// pre-re-arm version of this leg created. The aarch64 strict gate scores a
/// boot against a 20 s ceiling and has been recorded within 700 ms of it, so
/// the leg's thread count and its window are held at what that gate was
/// measured green on rather than at whatever the measurement would prefer.
const PEERS: usize = 6;

/// Threads that sleep and immediately sleep again. More than one, because the
/// question the peer gap answers is what a POPULATION of re-arming timer
/// threads costs the CPU-bound class, not what one does.
const REARMERS: usize = 3;

/// Sleeps per re-armer.
const REARMS: u64 = 4;

/// Sleeps the leg performs in total. Pinned on the marker, so a re-armer that
/// gave up early cannot be read as a completed run.
const TOTAL_REARMS: u64 = REARMERS as u64 * REARMS;

/// The sleep each re-arm measures.
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

/// Gate bound on a wake's overrun. Mechanism bound is
/// `QUANTUM_MS + MS_PER_TICK` (55 ms on x86, 11 ms on aarch64); the remainder
/// is an emulated-timer jitter allowance.
const BOUND_MS: u64 = 100;

/// Starvation ceiling on a CPU-bound peer's worst off-CPU interval: four times
/// the round it would wait if each thread in this leg held a full quantum.
/// x86: `(6 + 3 + 2) * 50 * 4` = 2200 ms. aarch64: `(6 + 3 + 2) * 10 * 4` =
/// 440 ms.
const PEER_GAP_BOUND_MS: u64 = (PEERS as u64 + REARMERS as u64 + 2) * QUANTUM_MS * 4;

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
/// Budget for the whole re-arm phase, measured from the barrier. A re-armer
/// that reaches it abandons its remaining sleeps and counts a backstop, so a
/// build whose wakes are slow reports a FAIL with numbers on it instead of
/// running until the gate's own boot timeout.
const REARM_PHASE_BACKSTOP_MS: u64 = 60_000;
/// Budget for waiting on a class of threads to report itself finished before
/// joining it. `kthread_join` halts with no deadline of its own, so a thread
/// that stopped being dispatched would hang the boot inside the join and the
/// marker would not be printed. The leg waits on its own published counters
/// instead, and skips the join if the class did not finish.
const JOIN_WAIT_BACKSTOP_MS: u64 = 60_000;

static MEASURE_OPEN: AtomicBool = AtomicBool::new(false);
static PEERS_RUN: AtomicBool = AtomicBool::new(false);
static PEERS_STARTED: AtomicU32 = AtomicU32::new(0);
static PEERS_SPINNING: AtomicU32 = AtomicU32::new(0);
static PEERS_EXITED: AtomicU32 = AtomicU32::new(0);
static REARMERS_STARTED: AtomicU32 = AtomicU32::new(0);
static REARMERS_RUNNING: AtomicU32 = AtomicU32::new(0);
static REARMERS_DONE: AtomicU32 = AtomicU32::new(0);
static REARMS_DONE: AtomicU64 = AtomicU64::new(0);
static BACKSTOPS: AtomicU32 = AtomicU32::new(0);
static OVERRUN_MS: AtomicU64 = AtomicU64::new(0);
static PEER_MAX_GAP_NS: AtomicU64 = AtomicU64::new(0);
static WAKE_ENQUEUES: AtomicU64 = AtomicU64::new(0);
static ENQUEUES_AT_OPEN: AtomicU64 = AtomicU64::new(0);
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
/// holds its quantum, waking on each tick -- so this is used only for the short
/// waits inside the measured window, not across thread creation.
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

/// Park until the boot thread opens the barrier. Used by the peers only.
/// Re-parks on a spurious unpark; the boot thread sets `MEASURE_OPEN` before it
/// unparks anyone.
fn park_until_open() {
    while !MEASURE_OPEN.load(Ordering::Acquire) {
        kthread_park();
    }
}

/// A CPU-bound peer: parked until the barrier opens, then runnable for the
/// whole measurement window, does not yield, and leaves when the re-armers are
/// done or its own backstop expires.
///
/// While it spins it records the largest interval between two consecutive
/// readings of the monotonic clock. The peer is runnable throughout, so an
/// interval longer than a spin batch is time it spent off the CPU: that is the
/// cost the head enqueue could impose on the CPU-bound class, measured from
/// inside the class.
fn peer_body() {
    // Kernel threads start with interrupts disabled; a peer that cannot be
    // preempted would hold the CPU forever instead of taking one quantum.
    unsafe { arch_enable_interrupts() };
    PEERS_STARTED.fetch_add(1, Ordering::AcqRel);
    // Blocks and leaves the ready queue, so the creations still to come run at
    // full speed.
    park_until_open();
    PEERS_SPINNING.fetch_add(1, Ordering::AcqRel);
    let backstop = now_ns().saturating_add(PEER_SPIN_BACKSTOP_MS * 1_000_000);
    let mut previous = now_ns();
    let mut max_gap_ns: u64 = 0;
    while PEERS_RUN.load(Ordering::Acquire) {
        let sample = now_ns();
        let gap = sample.saturating_sub(previous);
        if gap > max_gap_ns {
            max_gap_ns = gap;
        }
        previous = sample;
        if sample >= backstop {
            BACKSTOPS.fetch_add(1, Ordering::Relaxed);
            break;
        }
        for _ in 0..256 {
            core::hint::spin_loop();
        }
    }
    PEER_MAX_GAP_NS.fetch_max(max_gap_ns, Ordering::AcqRel);
    PEERS_EXITED.fetch_add(1, Ordering::AcqRel);
}

/// Publish the window's readings once the last re-armer is finished, and
/// release the peers.
fn close_window() {
    if REARMERS_DONE.fetch_add(1, Ordering::AcqRel) as usize + 1 < REARMERS {
        return;
    }
    WAKE_ENQUEUES.store(
        scheduler::ENQUEUE_TIMER_WAKE
            .load(Ordering::Relaxed)
            .wrapping_sub(ENQUEUES_AT_OPEN.load(Ordering::Acquire)),
        Ordering::Release,
    );
    WINDOW_MS.store(
        now_ns().saturating_sub(OPEN_NS.load(Ordering::Acquire)) / 1_000_000,
        Ordering::Release,
    );
    MEASURED.store(true, Ordering::Release);
    PEERS_RUN.store(false, Ordering::Release);
}

/// A measured thread: sleeps `REARMS` times in a row, each against its own
/// absolute deadline, and folds the worst overrun into `OVERRUN_MS`.
fn rearmer_body() {
    unsafe { arch_enable_interrupts() };
    REARMERS_STARTED.fetch_add(1, Ordering::AcqRel);
    // A re-armer does NOT park. Parking is what lets the eight peers leave the
    // ready queue while the remaining threads are created, and it costs a
    // `kthread_unpark` handshake per thread; a re-armer is created after the
    // peers, so at most three of them hold a quantum during the last three
    // creations, so the handshake buys little. It also keeps the re-armers out
    // of the park/unpark path, which matters because the join that waits for
    // them cannot time out on its own.
    wait_until(
        || MEASURE_OPEN.load(Ordering::Acquire),
        START_WAIT_BACKSTOP_MS,
    );
    REARMERS_RUNNING.fetch_add(1, Ordering::AcqRel);
    // Do not start measuring until `PEERS_SPINNING` reaches `PEERS`; a peer
    // still parked is not competition and would understate the wait this
    // oracle exists to bound.
    wait_until(
        || PEERS_SPINNING.load(Ordering::Acquire) as usize >= PEERS,
        SPINNING_WAIT_BACKSTOP_MS,
    );

    let Some(tid) = scheduler::current_thread_id() else {
        close_window();
        return;
    };

    let phase_deadline = OPEN_NS
        .load(Ordering::Acquire)
        .saturating_add(REARM_PHASE_BACKSTOP_MS * 1_000_000);
    for _ in 0..REARMS {
        if now_ns() >= phase_deadline {
            BACKSTOPS.fetch_add(1, Ordering::Relaxed);
            break;
        }
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
        OVERRUN_MS.fetch_max(
            woke.saturating_sub(deadline) / 1_000_000,
            Ordering::AcqRel,
        );
        REARMS_DONE.fetch_add(1, Ordering::AcqRel);
    }

    close_window();
}

/// Emit the verdict line. `reason` is empty on a run that measured something.
fn emit(spawned_peers: usize, spawned_rearmers: usize, reason: &str) {
    let measured = MEASURED.load(Ordering::Acquire);
    let backstops = BACKSTOPS.load(Ordering::Relaxed);
    let wake_enqueues = WAKE_ENQUEUES.load(Ordering::Acquire);
    let started = PEERS_STARTED.load(Ordering::Acquire);
    let spinning = PEERS_SPINNING.load(Ordering::Acquire);
    let exited = PEERS_EXITED.load(Ordering::Acquire);
    let rearmers_done = REARMERS_DONE.load(Ordering::Acquire);
    let rearms_done = REARMS_DONE.load(Ordering::Acquire);
    let overrun_ms = OVERRUN_MS.load(Ordering::Acquire);
    let peer_max_gap_ms = PEER_MAX_GAP_NS.load(Ordering::Acquire) / 1_000_000;
    // A run passes only if it MEASURED the window (so a boot that did not reach
    // the wake path cannot report a small number), if the completed re-arm
    // count reached `TOTAL_REARMS`, if the wakes went through the timer-wake
    // enqueue site, if `PEERS_SPINNING` reached `PEERS` when the sleeps started,
    // if the backstop count is 0, if the worst overrun is inside the wake
    // bound, and if the worst peer gap is inside the starvation ceiling.
    let pass = measured
        && reason.is_empty()
        && spawned_peers == PEERS
        && spawned_rearmers == REARMERS
        && started as usize == PEERS
        && spinning as usize == PEERS
        && exited as usize == PEERS
        && rearmers_done as usize == REARMERS
        && rearms_done == TOTAL_REARMS
        && wake_enqueues >= REARMERS as u64
        && backstops == 0
        && overrun_ms <= BOUND_MS
        && peer_max_gap_ms <= PEER_GAP_BOUND_MS;
    serial_println!(
        "[TIMER_WAKE_LATENCY_ORACLE:{}:sleep_ms={}:peers={}:rearmers={}:rearms={}:overrun_ms={}:bound_ms={}:quantum_ms={}:round_ms={}:peer_max_gap_ms={}:peer_gap_bound_ms={}:wake_enqueues={}:peers_started={}:peers_spinning={}:peers_exited={}:backstops={}:setup_ms={}:window_ms={}:measured={}:{}{}]",
        ARCH_TAG,
        SLEEP_MS,
        PEERS,
        rearmers_done,
        rearms_done,
        overrun_ms,
        BOUND_MS,
        QUANTUM_MS,
        ROUND_MS,
        peer_max_gap_ms,
        PEER_GAP_BOUND_MS,
        wake_enqueues,
        started,
        spinning,
        exited,
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
/// the state it had; the peers and the re-armers need interrupts on to be
/// preempted at all.
pub fn run() {
    MEASURE_OPEN.store(false, Ordering::Release);
    PEERS_RUN.store(true, Ordering::Release);
    PEERS_STARTED.store(0, Ordering::Release);
    PEERS_SPINNING.store(0, Ordering::Release);
    PEERS_EXITED.store(0, Ordering::Release);
    REARMERS_STARTED.store(0, Ordering::Release);
    REARMERS_RUNNING.store(0, Ordering::Release);
    REARMERS_DONE.store(0, Ordering::Release);
    REARMS_DONE.store(0, Ordering::Release);
    BACKSTOPS.store(0, Ordering::Release);
    OVERRUN_MS.store(0, Ordering::Release);
    PEER_MAX_GAP_NS.store(0, Ordering::Release);
    WAKE_ENQUEUES.store(0, Ordering::Release);
    ENQUEUES_AT_OPEN.store(0, Ordering::Release);
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
    let spawned_peers = peers.len();

    let mut rearmers = Vec::with_capacity(REARMERS);
    if spawned_peers == PEERS {
        for _ in 0..REARMERS {
            match kthread_run(rearmer_body, "t766_rearmer") {
                Ok(handle) => rearmers.push(handle),
                Err(_) => break,
            }
        }
    }
    let spawned_rearmers = rearmers.len();

    if spawned_peers == PEERS && spawned_rearmers == REARMERS {
        // Everyone has to have reached the barrier before it opens, so the
        // peers' spin windows and the re-armers' sleeps start at the same
        // point.
        wait_until(
            || {
                PEERS_STARTED.load(Ordering::Acquire) as usize >= PEERS
                    && REARMERS_STARTED.load(Ordering::Acquire) as usize >= REARMERS
            },
            START_WAIT_BACKSTOP_MS,
        );
        let open = now_ns();
        OPEN_NS.store(open, Ordering::Release);
        ENQUEUES_AT_OPEN.store(
            scheduler::ENQUEUE_TIMER_WAKE.load(Ordering::Relaxed),
            Ordering::Release,
        );
        SETUP_MS.store(open.saturating_sub(started_ns) / 1_000_000, Ordering::Release);
        MEASURE_OPEN.store(true, Ordering::Release);
        // Unpark in a retry loop: a thread that entered `kthread_park()` after
        // a single unpark would block with nobody left to wake it. Repeating
        // the unpark until each thread reports itself running closes that
        // window without needing the park and the unpark to be ordered.
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

    // Wait on the counter the re-armers publish, not on the join: a join that
    // does not return prints no marker, which is how the first round-2 mutation
    // run ended (section 7 of the round record).
    let rearmers_finished = wait_until(
        || REARMERS_DONE.load(Ordering::Acquire) as usize >= spawned_rearmers,
        JOIN_WAIT_BACKSTOP_MS,
    );
    if rearmers_finished {
        for handle in rearmers.iter() {
            let _ = kthread_join(handle);
        }
    }
    // Whatever happened above, the peers are told to stop and unparked before
    // they are joined, or a failed re-armer leaves eight threads behind and the
    // join below waits on a thread with no remaining waker.
    MEASURE_OPEN.store(true, Ordering::Release);
    PEERS_RUN.store(false, Ordering::Release);
    let cleanup_deadline = now_ns().saturating_add(CLEANUP_BACKSTOP_MS * 1_000_000);
    loop {
        if PEERS_EXITED.load(Ordering::Acquire) as usize >= spawned_peers {
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
    if PEERS_EXITED.load(Ordering::Acquire) as usize >= spawned_peers {
        for handle in peers.iter() {
            let _ = kthread_join(handle);
        }
    }

    if !interrupts_were_enabled {
        unsafe { arch_disable_interrupts() };
    }

    let reason = if spawned_peers != PEERS {
        ":reason=peer_spawn_failed"
    } else if spawned_rearmers != REARMERS {
        ":reason=rearmer_spawn_failed"
    } else {
        ""
    };
    emit(spawned_peers, spawned_rearmers, reason);
}
