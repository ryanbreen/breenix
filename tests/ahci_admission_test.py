"""Force slot contention through the production x86 admission loop."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

HARNESS = r'''
use std::cell::Cell;
use std::sync::{Mutex, MutexGuard, atomic::{AtomicU64, Ordering}};
#[allow(unused_imports)]
use per_cpu as io_cpu;
use read_failure::AhciReadError;

#[allow(dead_code)]
#[path = "READ_FAILURE"] mod read_failure;

const AHCI_TIMEOUT_SECS: u64 = 5;
const OWNER_WITHOUT_TID: u64 = u64::MAX;
static PORT_IO_OWNER: [AtomicU64; 1] = [AtomicU64::new(21)];
static PORT_IO_LOCK: [PortLock; 1] = [PortLock(Mutex::new(()))];
static PORT_IO_WAITERS: [task::waitqueue::Queue; 1] = [task::waitqueue::Queue];
struct PortLock(Mutex<()>);
impl PortLock {
    fn lock(&self) -> MutexGuard<'_, ()> { self.0.try_lock().unwrap() }
}
fn io_tid() -> Option<u64> { task::scheduler::current_thread_id() }
thread_local! {
    static BRAKES: Cell<u32> = const { Cell::new(0) };
    static WAITED: Cell<u32> = const { Cell::new(0) };
    static NOW: Cell<u64> = const { Cell::new(0) };
    static RELEASE: Cell<bool> = const { Cell::new(true) };
}
pub mod per_cpu {
    pub fn preempt_count() -> u32 { super::BRAKES.with(|v| v.get()) }
    pub fn preempt_disable() { super::BRAKES.with(|v| v.set(v.get() + 1)); }
    pub fn preempt_enable() { super::BRAKES.with(|v| v.set(v.get() - 1)); }
    pub fn in_interrupt() -> bool { false }
}
pub mod time {
    pub fn get_monotonic_time_ns() -> (u64, u64) {
        (super::NOW.with(|v| v.get()), 0)
    }
}
pub mod block { pub mod cache_kernel {
    pub fn park_for_wait_retry() { panic!("unexpected retry"); }
}}
pub mod task {
    pub mod thread {
        pub enum ThreadState { BlockedOnIO }
        pub fn current_wait_must_unwind() -> bool { false }
    }
    pub mod scheduler {
        pub struct Scheduler;
        impl Scheduler { pub fn idle_thread(&self) -> u64 { 0 } }
        pub fn current_thread_id() -> Option<u64> { Some(26) }
        pub fn with_current<T>(f: impl FnOnce(&Scheduler) -> T) -> Option<T> { Some(f(&Scheduler)) }
    }
    pub mod waitqueue {
        use crate::{AHCI_TIMEOUT_SECS, NOW, PORT_IO_LOCK, PORT_IO_OWNER, RELEASE, WAITED};
        use crate::per_cpu::*;
        use std::sync::atomic::Ordering;

        #[derive(PartialEq)]
        pub enum PrepareOutcome { Queued, Mismatch, OutOfMemory }
        pub struct Queue;
        impl Queue {
            pub fn prepare_to_wait_checked(
                &self, _: super::thread::ThreadState, deadline: Option<u64>,
                busy: impl FnOnce() -> bool,
            ) -> PrepareOutcome {
                assert_eq!(preempt_count(), 1);
                assert_eq!(deadline, Some(AHCI_TIMEOUT_SECS * 1_000_000_000));
                assert!(busy());
                PrepareOutcome::Queued
            }
            pub fn finish_wait(&self) { assert_eq!(preempt_count(), 1); }
        }
        pub fn schedule_current_wait() {
            assert_eq!(preempt_count(), 1);
            preempt_enable();
            let _port = PORT_IO_LOCK[0].lock();
            if RELEASE.with(|v| v.get()) {
                PORT_IO_OWNER[0].store(0, Ordering::Release);
            } else {
                NOW.with(|v| v.set(AHCI_TIMEOUT_SECS));
            }
            WAITED.with(|v| v.set(v.get() + 1));
            preempt_disable();
        }
    }
}

#[test]
fn preemptible_reader_parks_and_preserves_caller_brake() {
    for count in [0, 1] {
        PORT_IO_OWNER[0].store(21, Ordering::Release);
        BRAKES.with(|v| v.set(count));
        let _guard = begin_port_io(0, true).unwrap();
        assert_eq!(PORT_IO_OWNER[0].load(Ordering::Acquire), 26);
        assert_eq!(per_cpu::preempt_count(), count);
        assert_eq!(WAITED.with(|v| v.get()), count + 1);
    }
}

#[test]
fn busy_port_past_deadline_restores_caller_brake() {
    RELEASE.with(|v| v.set(false));
    PORT_IO_OWNER[0].store(21, Ordering::Release);
    assert_eq!(begin_port_io(0, true).unwrap_err(), (AhciReadError::AdmissionTimeout, 21));
    assert_eq!(WAITED.with(|v| v.get()), 1);
    assert_eq!(per_cpu::preempt_count(), 0);
}
'''


class AhciAdmission(unittest.TestCase):
    def test_x86_slot_contention_from_preemptible_thread(self):
        path = ROOT / 'kernel/src/drivers/ahci/mod.rs'
        source = path.read_text()
        start = source.index('fn can_park_for_port_io(')
        end = source.index('fn end_port_io(', start)
        admission = source[start:end].removesuffix('#[inline]\n')
        # Select the x86 Rust branches while running the fake scheduler on the host.
        admission = admission.replace('#[cfg(target_arch = "x86_64")]', '')
        admission = admission.replace('#[cfg(target_arch = "aarch64")]', '#[cfg(any())]')
        harness = HARNESS.replace('READ_FAILURE', str(ROOT / 'kernel/src/block/read_failure.rs'))
        with tempfile.TemporaryDirectory() as scratch:
            test = Path(scratch) / 'admission.rs'
            binary = test.with_suffix('')
            test.write_text(harness + admission)
            subprocess.run(['rustc', '--edition=2021', '--test', '-Dwarnings', str(test), '-o', str(binary)],
                           check=True)
            result = subprocess.run([str(binary), '--test-threads=1'], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == '__main__':
    unittest.main()
