//! SCRATCH ONLY, never merged (#1401 forcing).
//!
//! Emulates a host stall of the vCPU that runs a scheduling pass started by
//! sched_yield in which the yielding thread keeps its CPU (the x86 branch of
//! Scheduler::schedule that returns None for a sole runnable user thread): once per armed
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

/// Under the scheduler lock, at the start of a pass on this CPU: whether
/// sys_yield asked for it. Cleared on every pass.
pub fn take_yield_pass() -> bool {
    let c = cpu();
    c < 8 && YIELD_PASS[c].swap(false, Ordering::AcqRel)
}

/// sys_yield returned without a pass (the fix's fast path): the same stall,
/// on the same CPU, with interrupts masked as they are inside a pass, but
/// holding no lock.
pub fn fast_path() {
    x86_64::instructions::interrupts::without_interrupts(stall_if_due);
}

/// Spin 80 ms once per armed deadline, when called from 3 ms before it to
/// 1 ms after it. The baseline calls it in the scheduling pass's branch that
/// keeps the yielding user thread on its CPU (another CPU online, nothing
/// else runnable), with the scheduler lock and PROCESS_MANAGER held.
pub fn stall_if_due() {
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
    trace(3, crate::per_cpu::current_thread_id_lock_free().unwrap_or(0), deadline, now);
    STALLS.fetch_add(1, Ordering::Relaxed);
    while now_ns() < now + 80_000_000 {
        core::hint::spin_loop();
    }
    trace(4, crate::per_cpu::current_thread_id_lock_free().unwrap_or(0), deadline, now_ns());
}

struct Event { kind: AtomicU64, tid: AtomicU64, cpu: AtomicU64, deadline: AtomicU64, at: AtomicU64 }
static EVENTS: [Event; 2048] = [const { Event { kind: AtomicU64::new(0), tid: AtomicU64::new(0), cpu: AtomicU64::new(0), deadline: AtomicU64::new(0), at: AtomicU64::new(0) } }; 2048];
static INDEX: AtomicU64 = AtomicU64::new(0);
static DUMP: AtomicBool = AtomicBool::new(false);
pub fn trace(kind: u64, tid: u64, deadline: u64, at: u64) {
    let i = INDEX.fetch_add(1, Ordering::Relaxed) as usize;
    if let Some(e) = EVENTS.get(i) {
        e.tid.store(tid, Ordering::Relaxed);
        e.cpu.store(cpu() as u64, Ordering::Relaxed);
        e.deadline.store(deadline, Ordering::Relaxed);
        e.at.store(at, Ordering::Relaxed);
        e.kind.store(kind, Ordering::Release);
    }
}
pub fn observe_write(bytes: &[u8]) {
    if bytes.windows(b"SUITE time DONE ".len()).any(|s| s == b"SUITE time DONE ") { DUMP.store(true, Ordering::Release); }
}
pub fn report() {
    if !DUMP.swap(false, Ordering::AcqRel) { return; }
    log::info!("[FORCE1401 arms={} stalls={} records={}]", ARMS.load(Ordering::Relaxed), STALLS.load(Ordering::Relaxed), INDEX.load(Ordering::Relaxed));
    for e in &EVENTS {
        let k=e.kind.load(Ordering::Acquire);
        if k!=0 { log::info!("[SLEEP_TRACE kind={} tid={} cpu={} deadline={} at={}]", k, e.tid.load(Ordering::Relaxed), e.cpu.load(Ordering::Relaxed), e.deadline.load(Ordering::Relaxed), e.at.load(Ordering::Relaxed)); }
    }
}
