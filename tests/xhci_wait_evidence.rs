//! Failure snapshots must distinguish delivery, wake and command progress.
#[path = "../kernel/src/drivers/xhci/wait_evidence.rs"]
mod wait_evidence;
use std::sync::atomic::{AtomicU64, Ordering};
static CLOCK: AtomicU64 = AtomicU64::new(100);
fn now() -> u64 { CLOCK.load(Ordering::Relaxed) }
mod per_cpu { pub fn preempt_count() -> u32 { 0x10001 } }
mod task { pub mod scheduler { pub fn current_cpu() -> usize { 2 } } }
mod log_notice {
    pub static LINES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    pub struct Record;
    impl Record {
        pub const fn new() -> Self { Self }
        pub fn post_again(&'static self, args: core::fmt::Arguments) -> bool {
            LINES.lock().unwrap().push(format!("{args}")); true
        }
    }
}
#[test]
fn failure_separates_command_progress_from_irq_and_deferred_wake() {
    wait_evidence::begin(0x1230, 33, 15 << 10, 3100, 8);
    wait_evidence::blocked(3100);
    wait_evidence::irq(20);
    wait_evidence::report();
    let first = std::mem::take(&mut *log_notice::LINES.lock().unwrap());
    assert!(first[0].contains("irq=1 irq_waiter=20 irq_cpu=2 irq_preempt=65537 wakes=0"));
    assert!(first[0].contains("phase=2 deadline=3100 worker_cpu=2 worker_preempt=65537"));
    assert!(first[1].contains("wanted=0x1230 kind=33 command=0x3c00 began=100 deadline=3100 irq=8 end=0 code=0"));
    wait_evidence::wake();
    CLOCK.store(3100, Ordering::Relaxed);
    wait_evidence::returned(true);
    wait_evidence::end(1);
    wait_evidence::report();
    let second = std::mem::take(&mut *log_notice::LINES.lock().unwrap());
    assert!(second[0].contains("wakes=1 phase=3"));
    assert!(second[1].contains("end=3100 code=1"));
    for i in 0..20 { wait_evidence::begin(i, 33, 0, 6100, 9); }
    wait_evidence::report();
    let bounded = std::mem::take(&mut *log_notice::LINES.lock().unwrap());
    assert_eq!(bounded.len(), 9, "one summary and eight command slots");
    assert!(bounded.iter().any(|line| line.contains("seq=21 wanted=0x13")));
}
