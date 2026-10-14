use core::sync::atomic::{AtomicU64, Ordering};
static SIGNAL: AtomicU64 = AtomicU64::new(0);
static DEADLINE: AtomicU64 = AtomicU64::new(0);
static STALL_START: AtomicU64 = AtomicU64::new(0);
static STALL_END: AtomicU64 = AtomicU64::new(0);
static EXPIRY: AtomicU64 = AtomicU64::new(0);
static DELIVERY: AtomicU64 = AtomicU64::new(0);
static OVERRUN: AtomicU64 = AtomicU64::new(0);
static REMOVED: AtomicU64 = AtomicU64::new(0);
pub fn now() -> u64 { crate::signal::monotonic_nanos() }
pub fn arm(signal: usize, clock: i32, interval: u64, value: u64, deadline: u64) {
    if clock == 1 && interval == 50_000_000 && value == interval && SIGNAL.compare_exchange(0, signal as u64, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
        DEADLINE.store(deadline, Ordering::Release);
    }
}
pub fn observe() {
    if DEADLINE.load(Ordering::Acquire) != 0 && STALL_END.load(Ordering::Acquire) == 0 {
        let guard = crate::process::manager();
        core::hint::black_box(guard.as_ref().map(|m| m.iter_processes().count()));
    }
}
pub fn force() {
    let deadline = DEADLINE.load(Ordering::Acquire);
    let current = now();
    if deadline != 0 && current >= deadline.saturating_sub(1_000_000) && STALL_START.compare_exchange(0, current, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
        while now() < deadline + 172_000_000 { core::hint::spin_loop(); }
        STALL_END.store(now(), Ordering::Release);
    }
}
pub fn expiry(signal: usize) {
    if SIGNAL.load(Ordering::Acquire) == signal as u64 { let _ = EXPIRY.compare_exchange(0, now(), Ordering::SeqCst, Ordering::SeqCst); }
}
pub fn delivery(signal: usize, overrun: i32) {
    if SIGNAL.load(Ordering::Acquire) == signal as u64 && DELIVERY.compare_exchange(0, now(), Ordering::SeqCst, Ordering::SeqCst).is_ok() { OVERRUN.store(overrun as u64, Ordering::Release); }
}
pub fn removed(signal: usize) {
    if SIGNAL.load(Ordering::Acquire) == signal as u64 { REMOVED.store(1, Ordering::Release); }
}
pub fn dump() {
    if REMOVED.swap(0, Ordering::AcqRel) != 0 {
        crate::serial_println!("P1540 deadline={} stall_start={} stall_end={} expiry={} delivery={} overrun={}", DEADLINE.load(Ordering::Acquire), STALL_START.load(Ordering::Acquire), STALL_END.load(Ordering::Acquire), EXPIRY.load(Ordering::Acquire), DELIVERY.load(Ordering::Acquire), OVERRUN.load(Ordering::Acquire));
    }
}
