//! Boot continuation oracle. Hot-path writers only publish plain atomics.
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub static BOOT_TID: AtomicU64 = AtomicU64::new(0);
pub static SWITCHED_AWAY: AtomicU64 = AtomicU64::new(0);
pub static SWITCH_PREEMPT: AtomicU64 = AtomicU64::new(u64::MAX);
static TESTS_COMPLETED: AtomicU64 = AtomicU64::new(0);
static REPORTED: AtomicBool = AtomicBool::new(false);

pub fn begin() {
    BOOT_TID.store(
        crate::task::scheduler::current_thread_id().unwrap(),
        Ordering::Release,
    );
}

pub fn tests_completed() {
    TESTS_COMPLETED.store(1, Ordering::Release);
}

pub fn finish() {
    BOOT_TID.store(0, Ordering::Release);
    report();
}

/// Called at the final boot marker, or at the terminal tally if boot was lost.
pub fn report() {
    if REPORTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let switched = SWITCHED_AWAY.load(Ordering::Acquire);
    let completed = TESTS_COMPLETED.load(Ordering::Acquire);
    crate::serial_println!(
        "[BOOT_DISK_WAIT_ORACLE:x86:switched_away={}:tests_completed={}:{}]",
        switched,
        completed,
        if switched == 0 && completed == 1 {
            "PASS"
        } else {
            "FAIL"
        }
    );
    crate::serial_println!(
        "[BOOT_DISK_WAIT_TRACE:x86:switch_preempt={}]",
        SWITCH_PREEMPT.load(Ordering::Acquire)
    );
}
