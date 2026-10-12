//! SCRATCH ONLY, never merged (#1401 forcing).
//!
//! Emulates a host stall of the vCPU that runs a scheduling pass started by
//! sched_yield in which the yielding thread keeps its CPU: once per armed
//! timed futex wait or sleep, in a window from 3 ms before its deadline to 1 ms after,
//! such a pass spins 80 ms with the scheduler lock (and PROCESS_MANAGER)
//! held, as run 20261012T122252Z-x86_64-gate-878f caught the suite runner's
//! CPU doing for 101 ms.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static YIELD_PASS: [AtomicBool; 8] = [const { AtomicBool::new(false) }; 8];
static ARMED: AtomicU64 = AtomicU64::new(0);
static DONE: AtomicU64 = AtomicU64::new(0);
static STALLS: AtomicU64 = AtomicU64::new(0);
static ARMS: AtomicU64 = AtomicU64::new(0);
static REPORTED: AtomicU64 = AtomicU64::new(0);

fn now_ns() -> u64 {
    crate::signal::monotonic_nanos()
}

fn cpu() -> usize {
    crate::per_cpu::cpu_id()
}

/// sys_yield is about to ask for a scheduling pass on this CPU.
pub fn note_yield() {
    let c = cpu();
    if c < 8 {
        YIELD_PASS[c].store(true, Ordering::Release);
    }
}

/// A timed futex wait blocked until `wake_ns`.
pub fn arm(wake_ns: u64) {
    if wake_ns >= now_ns() + 100_000_000 {
        ARMED.store(wake_ns, Ordering::Release);
        ARMS.fetch_add(1, Ordering::Relaxed);
    }
}

/// Under the scheduler lock, after a pass on this CPU chose `old` -> `new`.
pub fn after_pass(old: u64, new: u64) {
    let c = cpu();
    if c >= 8 || !YIELD_PASS[c].swap(false, Ordering::AcqRel) || old != new {
        return;
    }
    stall_if_due();
}

/// sys_yield returned without a pass (the fix's fast path): the same stall,
/// on the same CPU, with interrupts masked as they are inside a pass, but
/// holding no lock.
pub fn fast_path() {
    x86_64::instructions::interrupts::without_interrupts(stall_if_due);
}

/// Spin 80 ms once per armed deadline, when called from 3 ms before it to
/// 1 ms after it.
fn stall_if_due() {
    let deadline = ARMED.load(Ordering::Acquire);
    if deadline == 0 {
        return;
    }
    let done = DONE.load(Ordering::Acquire);
    if done == deadline {
        return;
    }
    let now = now_ns();
    if now + 3_000_000 < deadline || now > deadline + 1_000_000 {
        return;
    }
    if DONE
        .compare_exchange(done, deadline, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    STALLS.fetch_add(1, Ordering::Relaxed);
    while now_ns() < now + 80_000_000 {
        core::hint::spin_loop();
    }
}

/// Thread context: report the counts when they change.
pub fn report() {
    let stalls = STALLS.load(Ordering::Relaxed);
    let arms = ARMS.load(Ordering::Relaxed);
    let key = (stalls << 32) | arms;
    if REPORTED.swap(key, Ordering::Relaxed) != key {
        log::info!("[FORCE1401 arms={} stalls={}]", arms, stalls);
    }
}
