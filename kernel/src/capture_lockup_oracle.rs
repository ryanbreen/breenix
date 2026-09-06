//! The deterministic `edge=LOCKUP` oracle: a real peer-held scheduler mutex.
//!
//! Behind `--features capture_lockup_oracle`, which no gate builds with, so
//! this module is not compiled into a shipped or gated kernel. Its purpose is
//! to make PR-7's claim -- "a soft lockup whose scheduler lock is held by a
//! peer still emits a complete `BXCAP` bracket" -- testable on demand instead
//! of waiting for a real wedge.
//!
//! # The shape it constructs
//!
//! CPU1 runs a pinned kernel thread that takes the REAL global scheduler guard
//! through `task::scheduler::with_lockup_oracle_hold`, with local IRQ/FIQ
//! masked for the whole guard lifetime, and holds it past the detector's own
//! `LOCKUP_THRESHOLD_TICKS`. CPU0 stays in this coordinator with interrupts
//! ENABLED and boot preemption disabled, so its hardware timer keeps firing and
//! reaches `check_soft_lockup`. Nothing on either CPU makes a context switch, a
//! syscall or an exit-kick heartbeat move while the hold lasts, which is what
//! lets the detector's stall counter reach its threshold.
//! claim-lint:ok: measured, not asserted -- 2 of 2 episodes recorded
//! rcpt_progress_moved_during_hold=0 and equal ctx/syscall/heartbeat counters at
//! acquire and release in
//! docs/planning/green-program/failure-capture/serials/pr7/receipts.txt
//!
//! The refusal the capture then reports is therefore a genuine `try_lock`
//! failure against a genuine holder, not a forced return value. That is the
//! whole point: a fixture that injected a refusal would prove nothing about the
//! arm this PR repairs.
//! claim-lint:ok: the hold is measured -- rcpt_acquired=1 and rcpt_cpu=1 on 2 of
//! 2 episodes in docs/planning/green-program/failure-capture/serials/pr7/receipts.txt
//!
//! # What it deliberately does NOT do
//!
//! It does not call `dump_lockup_state`, `check_soft_lockup` or
//! `capture::emit`. It does not write a tick counter, the watchdog baseline or
//! `WATCHDOG_REPORTED`. It does not shorten the threshold -- the hold length is
//! read from `timer_interrupt::lockup_threshold_ticks()`, the constant the
//! detector itself enforces. Everything the run is scored on comes from the
//! kernel's own behaviour.
//!
//! It also does not represent every deadlock: a CPU0 hard wedge with interrupts
//! masked would stop the detector itself, and this construction cannot model
//! that. It models a peer-held scheduler stall with a live reporting CPU.
//! claim-lint:ok: this states a narrowing rather than a measurement; section 6 of
//! docs/planning/green-program/failure-capture/PR-7-2026-09-06.md carries it too
//!
//! # Receipts
//!
//! Every fact the host scorer needs is published in a fixed atomic slot, two
//! slots deep -- one per episode -- and read by GDB outside the held interval.
//! No progress log is printed from the IRQ or held window.
//! claim-lint:ok: 15 of 15 receipt slots plus the arming samples are listed in
//! docs/planning/green-program/failure-capture/serials/pr7/receipts.txt

use core::sync::atomic::{AtomicU64, Ordering};

/// Episodes this oracle runs in one boot. Two, because the second one is what
/// shows `WATCHDOG_REPORTED` recovering on observed progress and `IN_CAPTURE`
/// clearing normally: a single episode cannot distinguish "the latch works"
/// from "the latch was never re-entered".
/// claim-lint:ok: 2 of 2 episodes decode with seq advancing --
/// the_lockup_edge_emits_one_capture_per_episode_in_one_boot in
/// tests/capture_bxcap_schema_structure.rs
pub const ORACLE_EPISODES: usize = 2;

/// Extra CPU0 ticks held past the detector's threshold. The detector reports
/// once per uninterrupted episode, so the tail exercises the latch AFTER the
/// crossing rather than racing it.
const HOLD_TAIL_TICKS: u64 = 128;

/// Hardware-clock ceiling on establishing the hold, in seconds.
const SETUP_CEILING_SECS: u64 = 10;

/// Hardware-clock fail-safe on the hold itself, in seconds. Above the
/// threshold-plus-tail this oracle actually waits for (5.128 s at 1 kHz), so a
/// hold that reaches it is a failure receipt rather than a normal release.
const HOLD_FAILSAFE_SECS: u64 = 15;

// --- phases -----------------------------------------------------------------
// The host reads ORACLE_PHASE to know where the coordinator is, and writes
// ORACLE_HOST_ACK to let it past a checkpoint.

pub const PHASE_INIT: u64 = 0;
/// At an arming checkpoint: the host may sample DEFERRED_REQUEUE[0] and the
/// exit-kick heartbeat here, then ack to start the episode.
pub const PHASE_PRE_ARM: u64 = 1;
pub const PHASE_SPAWNED: u64 = 2;
pub const PHASE_HOLDING: u64 = 3;
pub const PHASE_RELEASED: u64 = 4;
pub const PHASE_DONE: u64 = 5;
pub const PHASE_FAILED: u64 = 9;

// --- setup failure codes ----------------------------------------------------

pub const FAIL_NONE: u64 = 0;
pub const FAIL_SPAWN: u64 = 1;
pub const FAIL_SETUP_TIMEOUT: u64 = 2;
pub const FAIL_HOLD_REFUSED: u64 = 3;
pub const FAIL_WRONG_CPU: u64 = 4;
pub const FAIL_HOLD_EXPIRED: u64 = 5;
pub const FAIL_NO_PROGRESS_AFTER_RELEASE: u64 = 6;
pub const FAIL_TIMESTAMP_FREQ: u64 = 7;

// --- coordinator/host handshake --------------------------------------------

pub static ORACLE_PHASE: AtomicU64 = AtomicU64::new(PHASE_INIT);
pub static ORACLE_EPISODE: AtomicU64 = AtomicU64::new(0);
pub static ORACLE_HOST_ACK: AtomicU64 = AtomicU64::new(0);
pub static ORACLE_SETUP_FAILURE: AtomicU64 = AtomicU64::new(FAIL_NONE);
pub static ORACLE_THRESHOLD_TICKS: AtomicU64 = AtomicU64::new(0);
pub static ORACLE_TSFREQ_HZ: AtomicU64 = AtomicU64::new(0);

// --- holder to coordinator --------------------------------------------------

/// Set by the holder with Release ordering only AFTER the real guard is in
/// hand. The coordinator's Acquire load of this is what starts the hold clock.
static HOLD_ESTABLISHED: AtomicU64 = AtomicU64::new(0);
/// Set by the coordinator to release the holder; read by the holder's spin.
static HOLD_RELEASE: AtomicU64 = AtomicU64::new(0);
/// Set by the holder as its last act, after the guard is dropped and its
/// interrupts are restored.
static HOLDER_FINISHED: AtomicU64 = AtomicU64::new(0);
/// Set by the holder when its own hardware-clock fail-safe, not the
/// coordinator, ended the hold.
static HOLD_EXPIRED: AtomicU64 = AtomicU64::new(0);
/// The CPU the holder actually ran on, as the hardware reports it.
static HOLD_OBSERVED_CPU: AtomicU64 = AtomicU64::new(u64::MAX);
/// Whether the masked try-lock helper returned success.
static HOLD_ACQUIRED: AtomicU64 = AtomicU64::new(0);

// --- per-episode receipts ---------------------------------------------------
// Read by GDB at the PHASE_RELEASED and PHASE_DONE checkpoints, outside
// the held interval.

macro_rules! receipt_slots {
    ($($name:ident),* $(,)?) => {
        $(pub static $name: [AtomicU64; ORACLE_EPISODES] =
            [const { AtomicU64::new(0) }; ORACLE_EPISODES];)*
    };
}

receipt_slots!(
    RCPT_ACQUIRED,
    RCPT_CPU,
    RCPT_TICK_AT_ACQUIRE,
    RCPT_TICK_AT_RELEASE,
    RCPT_HELD_TICKS,
    RCPT_CTX_AT_ACQUIRE,
    RCPT_CTX_AT_RELEASE,
    RCPT_SYSCALL_AT_ACQUIRE,
    RCPT_SYSCALL_AT_RELEASE,
    RCPT_HEARTBEAT_AT_ACQUIRE,
    RCPT_HEARTBEAT_AT_RELEASE,
    RCPT_PROGRESS_MOVED_DURING_HOLD,
    RCPT_EXPIRED,
    RCPT_TS_AT_ACQUIRE,
    RCPT_TS_AT_RELEASE,
);

#[inline(always)]
fn cpu_id() -> u64 {
    crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as u64
}

#[inline(always)]
fn cpu0_ticks() -> u64 {
    crate::arch_impl::aarch64::timer_interrupt::TIMER_TICK_COUNT[0].load(Ordering::Relaxed)
}

#[inline(always)]
fn hw_now() -> u64 {
    crate::tracing::trace_timestamp()
}

#[inline(always)]
fn ctx_count() -> u64 {
    crate::task::scheduler::context_switch_count()
}

#[inline(always)]
fn syscall_count() -> u64 {
    crate::tracing::providers::counters::SYSCALL_TOTAL.aggregate()
}

#[inline(always)]
fn heartbeat() -> u64 {
    crate::arch_impl::aarch64::timer_interrupt::exit_kick_gate_watchdog_heartbeat()
}

fn fail(code: u64) {
    ORACLE_SETUP_FAILURE.store(code, Ordering::Release);
    ORACLE_PHASE.store(PHASE_FAILED, Ordering::Release);
}

/// Wait until the host has acknowledged checkpoint `n`, or the hardware clock
/// runs past `deadline`. Returns whether the ack arrived.
fn wait_for_ack(n: u64, deadline: u64) -> bool {
    while ORACLE_HOST_ACK.load(Ordering::Acquire) < n {
        if hw_now() >= deadline {
            return false;
        }
        core::hint::spin_loop();
    }
    true
}

/// The CPU1 holder. Runs as an ordinary pinned kernel thread; everything after
/// the guard is in hand is atomics and a spin hint.
fn holder_entry() {
    let observed = cpu_id();
    HOLD_OBSERVED_CPU.store(observed, Ordering::Release);
    if observed != 1 {
        HOLDER_FINISHED.store(1, Ordering::Release);
        return;
    }

    let tsfreq = crate::tracing::timestamp_frequency_hz();
    let acquired = crate::task::scheduler::with_lockup_oracle_hold(|| {
        // Guard held, interrupts masked. Atomics and a spin hint only.
        HOLD_ESTABLISHED.store(1, Ordering::Release);
        let failsafe = hw_now().wrapping_add(tsfreq.wrapping_mul(HOLD_FAILSAFE_SECS));
        while HOLD_RELEASE.load(Ordering::Acquire) == 0 {
            if hw_now() >= failsafe {
                HOLD_EXPIRED.store(1, Ordering::Release);
                break;
            }
            core::hint::spin_loop();
        }
    });

    HOLD_ACQUIRED.store(acquired as u64, Ordering::Release);
    HOLDER_FINISHED.store(1, Ordering::Release);
}

fn reset_episode_state() {
    HOLD_ESTABLISHED.store(0, Ordering::Release);
    HOLD_RELEASE.store(0, Ordering::Release);
    HOLDER_FINISHED.store(0, Ordering::Release);
    HOLD_EXPIRED.store(0, Ordering::Release);
    HOLD_OBSERVED_CPU.store(u64::MAX, Ordering::Release);
    HOLD_ACQUIRED.store(0, Ordering::Release);
}

/// Run one episode. Returns false and latches a failure code on any refusal,
/// wrong CPU, uninitialized scheduler, or expiry, rather than skipping silently.
fn run_episode(index: usize, threshold: u64, tsfreq: u64) -> bool {
    reset_episode_state();
    ORACLE_EPISODE.store(index as u64, Ordering::Release);
    ORACLE_PHASE.store(PHASE_PRE_ARM, Ordering::Release);

    // Arming checkpoint. The host samples CPU0's deferred-requeue slot and the
    // exit-kick heartbeat here -- outside any held interval -- and acks.
    let ack_deadline = hw_now().wrapping_add(tsfreq.wrapping_mul(SETUP_CEILING_SECS));
    if !wait_for_ack(index as u64 + 1, ack_deadline) {
        fail(FAIL_SETUP_TIMEOUT);
        return false;
    }

    // Creating the thread allocates; this is ordinary boot-thread context on
    // CPU0, not a timer hook, which is the only place that is allowed.
    if crate::task::kthread::kthread_run_on_cpu_for_test(holder_entry, "lockup-oracle-holder", 1)
        .is_err()
    {
        fail(FAIL_SPAWN);
        return false;
    }
    ORACLE_PHASE.store(PHASE_SPAWNED, Ordering::Release);

    let setup_deadline = hw_now().wrapping_add(tsfreq.wrapping_mul(SETUP_CEILING_SECS));
    while HOLD_ESTABLISHED.load(Ordering::Acquire) == 0 {
        if HOLDER_FINISHED.load(Ordering::Acquire) != 0 {
            // The holder returned without establishing a hold: either it landed
            // on the wrong CPU or the guard was refused.
            if HOLD_OBSERVED_CPU.load(Ordering::Acquire) != 1 {
                fail(FAIL_WRONG_CPU);
            } else {
                fail(FAIL_HOLD_REFUSED);
            }
            return false;
        }
        if hw_now() >= setup_deadline {
            fail(FAIL_SETUP_TIMEOUT);
            return false;
        }
        core::hint::spin_loop();
    }

    ORACLE_PHASE.store(PHASE_HOLDING, Ordering::Release);

    let tick_at_acquire = cpu0_ticks();
    let ctx_at_acquire = ctx_count();
    let syscall_at_acquire = syscall_count();
    let heartbeat_at_acquire = heartbeat();
    RCPT_TICK_AT_ACQUIRE[index].store(tick_at_acquire, Ordering::Relaxed);
    RCPT_CTX_AT_ACQUIRE[index].store(ctx_at_acquire, Ordering::Relaxed);
    RCPT_SYSCALL_AT_ACQUIRE[index].store(syscall_at_acquire, Ordering::Relaxed);
    RCPT_HEARTBEAT_AT_ACQUIRE[index].store(heartbeat_at_acquire, Ordering::Relaxed);
    RCPT_TS_AT_ACQUIRE[index].store(hw_now(), Ordering::Relaxed);
    RCPT_CPU[index].store(HOLD_OBSERVED_CPU.load(Ordering::Acquire), Ordering::Relaxed);

    // The wait. Atomic reads and a spin hint, and no more: no syscall, no
    // scheduler call, and deliberately no exit-kick heartbeat update -- that
    // last one is itself a liveness signal the detector honours, so touching it
    // here would suppress the report this oracle exists to observe.
    let want_ticks = threshold.wrapping_add(HOLD_TAIL_TICKS);
    let mut progress_moved = 0u64;
    loop {
        let now_ticks = cpu0_ticks();
        if ctx_count() != ctx_at_acquire
            || syscall_count() != syscall_at_acquire
            || heartbeat() != heartbeat_at_acquire
        {
            progress_moved = 1;
        }
        if now_ticks.wrapping_sub(tick_at_acquire) >= want_ticks {
            break;
        }
        if HOLD_EXPIRED.load(Ordering::Acquire) != 0 {
            break;
        }
        core::hint::spin_loop();
    }
    RCPT_PROGRESS_MOVED_DURING_HOLD[index].store(progress_moved, Ordering::Relaxed);

    let tick_at_release = cpu0_ticks();
    RCPT_TICK_AT_RELEASE[index].store(tick_at_release, Ordering::Relaxed);
    RCPT_HELD_TICKS[index]
        .store(tick_at_release.wrapping_sub(tick_at_acquire), Ordering::Relaxed);
    RCPT_CTX_AT_RELEASE[index].store(ctx_count(), Ordering::Relaxed);
    RCPT_SYSCALL_AT_RELEASE[index].store(syscall_count(), Ordering::Relaxed);
    RCPT_HEARTBEAT_AT_RELEASE[index].store(heartbeat(), Ordering::Relaxed);
    RCPT_TS_AT_RELEASE[index].store(hw_now(), Ordering::Relaxed);

    HOLD_RELEASE.store(1, Ordering::Release);

    let finish_deadline = hw_now().wrapping_add(tsfreq.wrapping_mul(SETUP_CEILING_SECS));
    while HOLDER_FINISHED.load(Ordering::Acquire) == 0 {
        if hw_now() >= finish_deadline {
            fail(FAIL_SETUP_TIMEOUT);
            return false;
        }
        core::hint::spin_loop();
    }

    RCPT_ACQUIRED[index].store(HOLD_ACQUIRED.load(Ordering::Acquire), Ordering::Relaxed);
    RCPT_EXPIRED[index].store(HOLD_EXPIRED.load(Ordering::Acquire), Ordering::Relaxed);

    if HOLD_ACQUIRED.load(Ordering::Acquire) == 0 {
        fail(FAIL_HOLD_REFUSED);
        return false;
    }
    if HOLD_EXPIRED.load(Ordering::Acquire) != 0 {
        fail(FAIL_HOLD_EXPIRED);
        return false;
    }

    // Real scheduling progress, then enough CPU0 ticks for the detector to have
    // observed it and cleared its own report latch. This coordinator does not make
    // the progress; the holder thread's own exit is what supplies it.
    let ctx_before = RCPT_CTX_AT_RELEASE[index].load(Ordering::Relaxed);
    let progress_deadline = hw_now().wrapping_add(tsfreq.wrapping_mul(SETUP_CEILING_SECS));
    while ctx_count() == ctx_before {
        if hw_now() >= progress_deadline {
            fail(FAIL_NO_PROGRESS_AFTER_RELEASE);
            return false;
        }
        core::hint::spin_loop();
    }
    let settle_from = cpu0_ticks();
    while cpu0_ticks().wrapping_sub(settle_from) < 64 {
        core::hint::spin_loop();
    }

    ORACLE_PHASE.store(PHASE_RELEASED, Ordering::Release);
    true
}

/// The CPU0 coordinator. Returns; it is not a divergent call, so the boot code
/// after it stays reachable in the compiler's view as well as at run time.
///
/// The host ends the special run at `PHASE_DONE` after collecting receipts. A
/// resumed boot-test suite is not this oracle's pass criterion.
pub fn run_lockup_capture_oracle() {
    let threshold = crate::arch_impl::aarch64::timer_interrupt::lockup_threshold_ticks();
    let tsfreq = crate::tracing::timestamp_frequency_hz();
    ORACLE_THRESHOLD_TICKS.store(threshold, Ordering::Release);
    ORACLE_TSFREQ_HZ.store(tsfreq, Ordering::Release);
    if tsfreq == 0 {
        fail(FAIL_TIMESTAMP_FREQ);
        return;
    }

    for index in 0..ORACLE_EPISODES {
        if !run_episode(index, threshold, tsfreq) {
            return;
        }
    }

    ORACLE_PHASE.store(PHASE_DONE, Ordering::Release);
    let done_deadline = hw_now().wrapping_add(tsfreq.wrapping_mul(SETUP_CEILING_SECS * 3));
    let _ = wait_for_ack(ORACLE_EPISODES as u64 + 1, done_deadline);
}
