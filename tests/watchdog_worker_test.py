"""Exercise the production watchdog worker against ARM scheduling and I/O contracts."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class WatchdogWorker(unittest.TestCase):
    def test_timer_wait_switches_inline_and_health_io_can_sleep(self):
        source = (ROOT / 'kernel/src/drivers/watchdog_round.rs').read_text()
        source = source[source.index('#[cfg(not(test))]\npub fn wait'):]
        source = source.replace('#[cfg(not(test))]', '')
        harness = r'''
use std::cell::Cell;
thread_local! {
    static PREEMPT: Cell<u32> = const { Cell::new(0) };
    static BLOCKED: Cell<bool> = const { Cell::new(false) };
    static SWITCHES: Cell<u32> = const { Cell::new(0) };
}
pub mod per_cpu_aarch64 {
    pub fn preempt_disable() { super::PREEMPT.with(|v| v.set(v.get() + 1)); }
    pub fn preempt_enable() { super::PREEMPT.with(|v| v.set(v.get() - 1)); }
}
pub mod time { pub fn get_monotonic_time_ns() -> (u64, u64) { (0, 0) } }
pub mod task {
    pub mod thread { #[derive(PartialEq)] pub enum ThreadState { BlockedOnTimer, Ready } }
    pub mod scheduler {
        use super::thread::ThreadState;
        pub struct Scheduler;
        impl Scheduler {
            pub fn block_current_for_timer(&mut self, until: u64) {
                assert_eq!(until, 20_000_000_000);
                crate::BLOCKED.with(|v| v.set(true));
            }
        }
        pub fn current_thread_id() -> Option<u64> { Some(1) }
        pub fn with_current(f: impl FnOnce(&mut Scheduler)) { f(&mut Scheduler); }
        pub fn yield_current() { panic!("ARM worker must switch inline"); }
        pub fn check_and_clear_need_resched() -> bool { true }
        pub fn schedule() {
            crate::SWITCHES.with(|v| v.set(v.get() + 1));
            crate::BLOCKED.with(|v| v.set(false));
        }
        pub fn thread_state(_: u64) -> Option<ThreadState> {
            Some(if crate::BLOCKED.with(|v| v.get()) { ThreadState::BlockedOnTimer } else { ThreadState::Ready })
        }
    }
}
pub fn arch_halt_with_interrupts() { panic!("ARM worker must switch inline"); }
pub mod fs { pub mod ext2 {
    pub struct Fs;
    pub struct Superblock;
    pub struct Inode;
    pub fn root_inode() -> u32 { 2 }
    pub fn root_fs_read() -> Result<Option<Fs>, &'static str> {
        if crate::PREEMPT.with(|v| v.get()) != 1 { return Err("contended ARM I/O cannot sleep"); }
        Ok(Some(Fs))
    }
    impl Fs {
        pub fn persisted_metadata(&self) -> Result<(Superblock, Vec<u8>), &'static str> { Ok((Superblock, vec![1])) }
        pub fn read_inode(&self, _: u32) -> Result<Inode, &'static str> { Ok(Inode) }
    }
    impl Superblock { pub fn block_size(&self) -> u32 { 4096 } }
    impl Inode { pub fn is_dir(&self) -> bool { true } }
} }
#[test] fn timer_wait_gives_up_cpu() {
    policy::wait(20_000).unwrap();
    assert_eq!(SWITCHES.with(|v| v.get()), 1);
}
#[test] fn health_establishes_and_restores_sleepable_io() {
    policy::health().unwrap();
    assert_eq!(PREEMPT.with(|v| v.get()), 0);
}
'''
        with tempfile.TemporaryDirectory() as scratch:
            path = Path(scratch)
            (path / 'worker.rs').write_text(harness + '\nmod policy {\n' + source.replace('//!', '//') + '\n}\n')
            subprocess.run(['rustc', '--edition=2021', '--test', '-Dwarnings', str(path / 'worker.rs'), '-o', str(path / 'worker')], check=True, capture_output=True)
            result = subprocess.run([str(path / 'worker')], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == '__main__':
    unittest.main()
