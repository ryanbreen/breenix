"""Run recovery's pause and prepared-wait helper with a counted PREEMPT model."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class PausePreemption(unittest.TestCase):
    def test_pause_preserves_count_and_protects_publication(self):
        recovery = (ROOT / "kernel/src/gpu/gk208/ce/recovery.rs").read_text()
        pause = recovery[recovery.index("fn pause()"):recovery.index("pub(super) fn probe(")]
        blocking = (ROOT / "kernel/src/syscall/blocking_io.rs").read_text()
        wait = blocking[blocking.index("pub(crate) fn wait_prepared("):blocking.index("/// PM-free adapter")]
        harness = r'''
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::SeqCst};
static COUNT: AtomicU32 = AtomicU32::new(0);
static UNDERFLOWS: AtomicU32 = AtomicU32::new(0);
static BLOCKED: AtomicBool = AtomicBool::new(false);
static CASE: AtomicU32 = AtomicU32::new(0);
static PUBLISHED_AT: AtomicU32 = AtomicU32::new(0);
static CLEANUPS: AtomicU32 = AtomicU32::new(0);
static HALTS: AtomicU32 = AtomicU32::new(0);
mod per_cpu {
    use super::*;
    pub fn preempt_count() -> u32 { COUNT.load(SeqCst) }
    pub fn preempt_disable() { COUNT.fetch_add(1, SeqCst); }
    pub fn preempt_enable() {
        if COUNT.load(SeqCst) == 0 { UNDERFLOWS.fetch_add(1, SeqCst); }
        else { COUNT.fetch_sub(1, SeqCst); }
    }
}
mod time { pub mod timer { pub fn get_monotonic_time_ns() -> (u64, u64) { (1, 0) } } }
mod task {
    pub mod thread {
        #[derive(Clone, Copy, PartialEq)]
        pub enum ThreadState { BlockedOnIO, Ready }
        pub struct Thread { pub state: ThreadState }
    }
    pub mod scheduler {
        use super::thread::{Thread, ThreadState};
        use crate::*;
        pub struct Scheduler { thread: Thread }
        impl Scheduler { pub fn current_thread_mut(&mut self) -> Option<&mut Thread> { Some(&mut self.thread) } }
        pub fn current_thread_id() -> Option<u64> { Some(7) }
        pub fn with_current<T>(f: impl FnOnce(&mut Scheduler) -> T) -> Option<T> {
            let state = if BLOCKED.load(SeqCst) { ThreadState::BlockedOnIO } else { ThreadState::Ready };
            Some(f(&mut Scheduler { thread: Thread { state } }))
        }
        pub fn yield_current() { assert_eq!(COUNT.load(SeqCst), 0, "sleep must release PREEMPT"); }
    }
    pub mod waitqueue {
        use super::thread::ThreadState;
        use crate::*;
        pub enum PrepareOutcome { Mismatch, PublishFailed, OutOfMemory, Queued }
        pub struct WaitQueueHead;
        impl WaitQueueHead {
            pub const fn new() -> Self { Self }
            pub fn prepare_to_wait_checked(&self, state: ThreadState, wake: Option<u64>, check: impl FnOnce() -> bool) -> PrepareOutcome {
                assert!(state == ThreadState::BlockedOnIO && check());
                assert_eq!(wake, Some(1_010_000_000));
                PUBLISHED_AT.store(COUNT.load(SeqCst), SeqCst);
                match CASE.load(SeqCst) {
                    1 => PrepareOutcome::Mismatch,
                    2 => PrepareOutcome::PublishFailed,
                    3 => PrepareOutcome::OutOfMemory,
                    _ => {
                        // Case 4 wakes after publication, before the signal check.
                        BLOCKED.store(CASE.load(SeqCst) != 4, SeqCst);
                        PrepareOutcome::Queued
                    }
                }
            }
            pub fn finish_wait(&self) { CLEANUPS.fetch_add(1, SeqCst); }
            pub fn take_waiter(&self, tid: u64) { assert_eq!(tid, 7); }
            pub fn finish_wait_for(&self, tid: u64) {
                assert_eq!(tid, 7);
                assert!(COUNT.load(SeqCst) > 0, "cleanup must retain PREEMPT");
                BLOCKED.store(false, SeqCst);
                CLEANUPS.fetch_add(1, SeqCst);
            }
        }
    }
}
mod syscall {
    use crate::*;
    pub fn check_signals_for_restartable_wait() -> Option<i32> {
        assert!(COUNT.load(SeqCst) > 0, "publication through signal check must hold PREEMPT");
        if CASE.load(SeqCst) == 5 { Some(4) } else { None }
    }
    pub mod errno { pub const ESRCH: i32 = 3; pub const ENOMEM: i32 = 12; }
    pub mod blocking_io {
        use super::errno;
        use crate::task::{thread::ThreadState, waitqueue::{PrepareOutcome, WaitQueueHead}};
        WAIT
    }
}
fn arch_halt_while_blocked() {
    assert_eq!(COUNT.load(SeqCst), 0);
    HALTS.fetch_add(1, SeqCst);
    BLOCKED.store(false, SeqCst);
}
PAUSE
fn main() {
    for incoming in [0, 1] {
        for case in 0..6 {
            COUNT.store(incoming, SeqCst);
            UNDERFLOWS.store(0, SeqCst);
            CLEANUPS.store(0, SeqCst);
            HALTS.store(0, SeqCst);
            BLOCKED.store(false, SeqCst);
            CASE.store(case, SeqCst);
            let result = pause();
            assert_eq!(per_cpu::preempt_count(), incoming, "pause leaked PREEMPT in case {case}");
            assert_eq!(UNDERFLOWS.load(SeqCst), 0);
            assert_eq!(PUBLISHED_AT.load(SeqCst), 1, "publication must hold one PREEMPT level");
            assert_eq!(HALTS.load(SeqCst), u32::from(case == 0));
            assert_eq!(CLEANUPS.load(SeqCst), u32::from(case != 3));
            assert_eq!(result, if [2, 3, 5].contains(&case) { Err("native recovery worker wait failed") } else { Ok(()) });
        }
    }
}
'''.replace("WAIT", wait).replace("PAUSE", pause)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / "pause.rs").write_text(harness)
            priority = ["taskpolicy", "-b"] if sys.platform == "darwin" else []
            subprocess.run(priority + ["rustc", "--edition=2021", "-Dwarnings", str(path / "pause.rs"), "-o", str(path / "pause")], check=True, cwd=ROOT)
            subprocess.run([str(path / "pause")], check=True)


if __name__ == "__main__":
    unittest.main()
