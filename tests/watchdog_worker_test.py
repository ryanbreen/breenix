"""Exercise the production watchdog worker against ARM scheduling and I/O contracts."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class WatchdogWorker(unittest.TestCase):
    def test_kcs_feeder_distinguishes_busy_and_wedged_root(self):
        path = ROOT / 'kernel/src/drivers/watchdog.rs'
        source = path.read_text()
        imports = next(line for line in source.splitlines() if line.startswith('use policy'))
        worker = source[source.index('impl policy::Transport for Kcs'):source.index('pub fn start()')]
        harness = r'''
use std::cell::{Cell, RefCell};
#[path = "WATCHDOG_ROUND"] mod round;

thread_local! {
    static NOW: Cell<u64> = const { Cell::new(0) };
    static LAST_FEED: Cell<Option<u64>> = const { Cell::new(None) };
    static EXPIRED: Cell<bool> = const { Cell::new(false) };
    static ROOT_WEDGED: Cell<bool> = const { Cell::new(false) };
    static COMPLETED: Cell<u64> = const { Cell::new(0) };
    static STALL: Cell<bool> = const { Cell::new(false) };
    static RUNNING: Cell<bool> = const { Cell::new(false) };
    static FLAGS: Cell<u8> = const { Cell::new(0) };
    static EVENTS: RefCell<Vec<(u64, String)>> = const { RefCell::new(Vec::new()) };
}
mod policy {
    pub use super::round::{feed, Transport};
    pub fn health() -> Result<(), &'static str> {
        super::Root::check_durable_roots(&super::Root::new(), None)
            .map_err(|_| "root metadata invalid")
    }
    pub fn wait(ms: u64) -> Result<(), &'static str> {
        let now = super::NOW.with(|v| {
            let delay = if v.get() == 0 && super::STALL.with(|stall| stall.get()) { 120_000 } else { ms };
            v.set(v.get() + delay);
            v.get()
        });
        if super::LAST_FEED.with(|v| v.get().is_some_and(|fed| now - fed >= 120_000)) {
            super::EXPIRED.with(|v| v.set(true));
            super::RUNNING.with(|v| v.set(false));
            super::FLAGS.with(|v| v.set(0x30));
        }
        if now < 180_000 { Ok(()) } else { Err("end of simulation") }
    }
}
mod block {
    pub enum HardwareDisk { Ahci(usize), Other }
}
mod drivers { pub mod ahci {
    pub fn completed_io(port: usize) -> u64 {
        assert_eq!(port, 0);
        super::super::COMPLETED.with(std::cell::Cell::get)
    }
} }
const ETIMEDOUT: i32 = 60;
#[derive(Debug, PartialEq)]
struct Errno(i32);
struct Root { inner: Mount }
struct Mount { state: MountState, device: Device }
struct MountState { durable: () }
struct Device;
impl Device {
    fn hardware_disk(&self) -> Option<block::HardwareDisk> {
        Some(if NOW.with(Cell::get) == 0 { block::HardwareDisk::Other } else { block::HardwareDisk::Ahci(0) })
    }
}
impl MountState {
    fn lock_interruptible(&self) -> Result<&Self, Errno> { Ok(self) }
}
impl Root {
    fn new() -> Self { Self { inner: Mount { state: MountState { durable: () }, device: Device } } }
    fn probe_durable_roots(&self, _: &(), _: Option<u64>) -> Result<(), Errno> {
        if NOW.with(Cell::get) == 0 { return Ok(()); }
        if !ROOT_WEDGED.with(Cell::get) { COMPLETED.with(|v| v.set(v.get() + 1)); }
        Err(Errno(ETIMEDOUT))
    }
    ROOT_CHECK
}
#[macro_export]
macro_rules! serial_println {
    ($($arg:tt)*) => { record(format!($($arg)*)) }
}
fn record(message: String) {
    EVENTS.with(|events| events.borrow_mut().push((NOW.with(Cell::get), message)));
}
struct Kcs;
impl Kcs {
    fn new(_: u32) -> Self { Self }
    fn request(&mut self, command: u8, data: &[u8]) -> Result<([u8; 32], usize), &'static str> {
        match command {
            1 => assert!(data.is_empty()),
            0x24 => {
                assert_eq!(data, TIMER);
                record("set".to_string());
                FLAGS.with(|flags| flags.set(flags.get() & !data[3]));
                RUNNING.with(|v| v.set(false));
            }
            0x22 => {
                assert!(data.is_empty());
                RUNNING.with(|v| v.set(true));
                LAST_FEED.with(|v| v.set(Some(NOW.with(Cell::get))));
            }
            _ => panic!("unexpected request {command:#x}"),
        }
        Ok(([0; 32], 14))
    }
    fn get(&mut self) -> Result<[u8; 8], &'static str> {
        let running = RUNNING.with(Cell::get);
        let remaining = if running {
            let last = LAST_FEED.with(Cell::get).unwrap();
            ((120_000 - (NOW.with(Cell::get) - last)) / 100) as u16
        } else { 0 };
        let [lo, hi] = remaining.to_le_bytes();
        Ok([TIMER[0] | (u8::from(running) << 6), TIMER[1], TIMER[2], FLAGS.with(Cell::get),
            TIMER[4], TIMER[5], lo, hi])
    }
}
const PERIOD_MS: u64 = 20_000;
#[test]
fn scheduled_feeder_continues_through_healthy_storage_contention() {
    assert_eq!(run(1), Err("end of simulation"));
    assert!(!EXPIRED.with(Cell::get));
    assert_eq!(LAST_FEED.with(Cell::get), Some(160_000));
}
#[test]
fn scheduled_feeder_lets_timer_expire_when_root_stops_progressing() {
    ROOT_WEDGED.with(|v| v.set(true));
    assert_eq!(run(1), Err("end of simulation"));
    assert!(EXPIRED.with(Cell::get));
    assert_eq!(LAST_FEED.with(Cell::get), Some(0));
}
#[test]
fn an_expired_timer_is_reported_before_set_clears_the_flag() {
    STALL.with(|v| v.set(true));
    assert_eq!(run(1), Err("end of simulation"));
    assert!(EXPIRED.with(Cell::get));
    assert_eq!(FLAGS.with(Cell::get), 0x20);
    EVENTS.with(|events| {
        let events = events.borrow();
        let expired = events.iter().position(|(ms, event)| *ms == 120_000 &&
            event == "[watchdog] expired-before-rearm flags=0x30 running=0 countdown-ticks=0")
            .expect("pre-Set expiry state should reach serial");
        let set = events.iter().position(|(ms, event)| *ms == 120_000 && event == "set").unwrap();
        assert!(expired < set);
    });
}
'''
        round_path = path.with_name('watchdog_round.rs')
        timer = next(line for line in source.splitlines() if line.startswith('const TIMER:'))
        fs_source = (ROOT / 'kernel/src/fs/breenfs/vfs.rs').read_text()
        root_check = fs_source[fs_source.index('    fn check_durable_roots('):
                               fs_source.index('    fn probe_durable_roots(')]
        root_check = root_check.replace('#[cfg(all(not(test), target_arch = "x86_64"))]', '')
        with tempfile.TemporaryDirectory() as scratch:
            test = Path(scratch) / 'kcs_worker.rs'
            binary = test.with_suffix('')
            test.write_text(harness.replace('WATCHDOG_ROUND', str(round_path))
                            .replace('ROOT_CHECK', root_check) + imports + '\n' + timer + '\n' + worker)
            for cfg in ([], ['--cfg', 'feature="watchdog_fault_inject"']):
                with self.subTest(cfg=cfg):
                    subprocess.run(['rustc', '--edition=2021', '--test', '-Dwarnings', *cfg,
                                    str(test), '-o', str(binary)], check=True)
                    result = subprocess.run([str(binary)], capture_output=True, text=True)
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_workers_start_after_init_publication_before_boot_pin_release(self):
        source = (ROOT / 'kernel/src/main_aarch64.rs').read_text()
        begin = source.index('fn launch_init_from_elf(')
        end = source.index('\n#[cfg(target_arch = "aarch64")]\nuse ', begin)
        launch = source[begin:end]
        call = 'kernel::drivers::i6300esb::start();'
        self.assertEqual(source.count(call), 1)
        self.assertIn(call, launch, 'workers must not contend with init construction on the boot continuation')
        self.assertLess(launch.index('manager.publish_init(publication)'), launch.index(call))
        self.assertLess(launch.index(call), launch.index('release_boot_preempt_pin();'))

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
pub mod fs { pub mod vfs {
    pub struct Fs;
    pub fn root_filesystem() -> Result<Fs, &'static str> { Ok(Fs) }
    impl Fs {
        pub fn check_live(&self) -> Result<(), &'static str> {
            if crate::PREEMPT.with(|v| v.get()) != 1 { return Err("contended ARM I/O cannot sleep"); }
            Ok(())
        }
    }
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
