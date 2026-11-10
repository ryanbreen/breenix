#!/usr/bin/env python3
"""Force idle-to-fork preemption while the deferred UFS queue is locked."""
from pathlib import Path
import os
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
source = (ROOT / "kernel/src/task/process_task.rs").read_text()
start = source.index("pub fn reclaim_deferred_process_resources() {")
end = source.index("\n}", start) + 2
production = source[start:end]
harness = r'''
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
static RECLAIM_PASS_ID: AtomicU32 = AtomicU32::new(0);
static RECLAIM_DRAIN_ACTIVE: AtomicBool = AtomicBool::new(false);
static PREEMPT_DEPTH: AtomicU32 = AtomicU32::new(0);
static DEFERRED_LOCK: AtomicBool = AtomicBool::new(false);
static OWED: AtomicU32 = AtomicU32::new(0);
static DELIVERED: AtomicU32 = AtomicU32::new(0);
#[macro_export]
macro_rules! trace_count { ($($token:tt)*) => { () }; }
#[macro_export]
macro_rules! proof_cover { ($($token:tt)*) => { () }; }
mod process { pub fn process_manager_held_on_current_cpu() -> bool { false } }
mod tracing { pub mod providers { pub mod teardown {
    pub fn scheduler_scope_active() -> bool { false }
} } }
mod ipc { pub mod shmem { pub fn drain_signal_pages() {} } }
mod socket { pub mod object { pub fn drain_release_queue() {} } }
mod fs { pub mod ufs {
    pub fn wake_deferred() {
        use crate::*;
        assert!(!DEFERRED_LOCK.swap(true, Ordering::Relaxed));
        if PREEMPT_DEPTH.load(Ordering::Relaxed) == 0 {
            // A timer can dispatch fork over the idle lock holder. Fork's
            // syscall guard then prevents that holder from resuming.
            reclaim_preempt_disable();
            assert!(!DEFERRED_LOCK.load(Ordering::Relaxed),
                "fork spins on the deferred UFS mutex held by preempted idle");
            reclaim_preempt_enable();
        }
        DELIVERED.fetch_add(OWED.swap(0, Ordering::Relaxed), Ordering::Relaxed);
        DEFERRED_LOCK.store(false, Ordering::Relaxed);
    }
} }
fn next_reclaim_pass_id(pass: u32) -> u32 { pass }
fn reclaim_preempt_disable() { PREEMPT_DEPTH.fetch_add(1, Ordering::Relaxed); }
fn reclaim_preempt_enable() {
    assert!(PREEMPT_DEPTH.fetch_sub(1, Ordering::Relaxed) > 0);
}
fn reclaim_deferred_process_resources_for_pass(pass: u32, boot_test_owned: bool) {
    assert!(pass > 0);
    assert!(!boot_test_owned);
}
#[test]
fn idle_queue_holder_cannot_be_preempted_by_fork() {
    OWED.store(1, Ordering::Relaxed);
    reclaim_deferred_process_resources();
    assert_eq!(DELIVERED.load(Ordering::Relaxed), 1);
    assert_eq!(PREEMPT_DEPTH.load(Ordering::Relaxed), 0);
    assert!(!RECLAIM_DRAIN_ACTIVE.load(Ordering::Relaxed));
    assert!(!DEFERRED_LOCK.load(Ordering::Relaxed));
}
'''
with tempfile.TemporaryDirectory(prefix="breenix-ufs-reclaim-") as temporary:
    path = Path(temporary)
    test = path / "check.rs"
    test.write_text(harness + "\n" + production)
    subprocess.run([os.environ.get("RUSTC", "rustc"), "--edition=2021", "-Dwarnings",
                    "--test", str(test), "-o", str(path / "check")], check=True)
    subprocess.run([str(path / "check")], check=True)
