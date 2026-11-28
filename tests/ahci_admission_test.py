"""Force slot contention through the production x86 admission loop."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

HARNESS = r'''
use std::cell::Cell;
use std::sync::{Mutex, MutexGuard, atomic::{AtomicU8, AtomicU64, Ordering}};
#[allow(unused_imports)]
use per_cpu as io_cpu;
use read_failure::AhciReadError;

#[allow(dead_code)]
#[path = "READ_FAILURE"] mod read_failure;

const AHCI_TIMEOUT_SECS: u64 = 5;
const OWNER_WITHOUT_TID: u64 = u64::MAX;
const PORT_READY: u8 = 0;
static PORT_RECOVERY: [AtomicU8; 1] = [AtomicU8::new(PORT_READY)];
static AHCI_ABAR: AtomicU64 = AtomicU64::new(0);
fn ensure_port_ready(_: u64, _: usize) -> Result<(), &'static str> { panic!("unexpected recovery") }
fn end_port_io(_: usize, _: MutexGuard<'static, ()>) { panic!("unexpected release") }
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
    static BASELINE: Cell<u32> = const { Cell::new(0) };
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
        pub fn current_thread_id() -> Option<u64> { Some(26) }
    }
    // The park contract as `begin_port_io` relies on it (task::park, E2.2;
    // its own rules are checked in tools/park-host-tests): a context may park
    // when its depth is its baseline, and a wait returns at the entry depth.
    pub mod park {
        use crate::{AHCI_TIMEOUT_SECS, BASELINE, NOW, PORT_IO_LOCK, PORT_IO_OWNER, RELEASE, WAITED};
        use crate::per_cpu::*;
        use std::sync::atomic::Ordering;

        #[derive(Debug, PartialEq)]
        pub enum ParkError { NotSleepable }
        #[allow(dead_code)]
        pub enum WaitEnd { Condition, Woken, Timeout, Interrupted(i32), Killed, NoMemory }
        pub enum WaitMode { Uninterruptible }
        pub fn require_sleepable() -> Result<(), ParkError> {
            if preempt_count() == BASELINE.with(|v| v.get()) { Ok(()) } else { Err(ParkError::NotSleepable) }
        }
        pub fn wait(
            _: &super::waitqueue::Queue,
            mut done: impl FnMut() -> bool,
            deadline: Option<u64>,
            _: WaitMode,
        ) -> Result<WaitEnd, ParkError> {
            require_sleepable()?;
            let entry = preempt_count();
            assert_eq!(deadline, Some(AHCI_TIMEOUT_SECS * 1_000_000_000));
            assert!(!done(), "the port is still owned when the wait begins");
            let _port = PORT_IO_LOCK[0].lock();
            let end = if RELEASE.with(|v| v.get()) {
                PORT_IO_OWNER[0].store(0, Ordering::Release);
                WaitEnd::Woken
            } else {
                NOW.with(|v| v.set(AHCI_TIMEOUT_SECS));
                WaitEnd::Timeout
            };
            WAITED.with(|v| v.set(v.get() + 1));
            assert_eq!(preempt_count(), entry);
            Ok(end)
        }
    }
    pub mod waitqueue {
        pub struct Queue;
    }
}

#[test]
fn preemptible_reader_parks_and_preserves_caller_brake() {
    // A kernel thread at depth 0, and a syscall holding its brake as its
    // baseline.
    for count in [0, 1] {
        PORT_IO_OWNER[0].store(21, Ordering::Release);
        BRAKES.with(|v| v.set(count));
        BASELINE.with(|v| v.set(count));
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

/// E2.2: a kernel thread holding a level beyond its baseline may not park,
/// and x86 refuses it rather than sleeping with the level held.
#[test]
fn x86_refuses_a_context_holding_a_level() {
    PORT_IO_OWNER[0].store(21, Ordering::Release);
    BRAKES.with(|v| v.set(1));
    BASELINE.with(|v| v.set(0));
    assert_eq!(begin_port_io(0, true).unwrap_err(), (AhciReadError::CannotPark, 21));
    assert_eq!(WAITED.with(|v| v.get()), 0);
    assert_eq!(per_cpu::preempt_count(), 1);
}
'''


class AhciAdmission(unittest.TestCase):
    def test_x86_slot_contention_from_preemptible_thread(self):
        path = ROOT / 'kernel/src/drivers/ahci/mod.rs'
        source = path.read_text()
        start = source.index('/// Acquire exclusive ownership of a port')
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
