"""Execute PID-1 and nested desktop startup with observed boot-check ordering."""
from pathlib import Path
import subprocess
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class DesktopBoot(unittest.TestCase):
    def test_only_pid_one_runs_boot_checks_before_supervision(self):
        source = (ROOT / 'userspace/programs/src/init.rs').read_text()
        function = item(source, 'fn run_desktop_mode(')
        harness = r'''
use ::std::sync::Mutex;
static EVENTS: Mutex<Vec<&str>> = Mutex::new(Vec::new());
static PID: ::std::sync::atomic::AtomicU32 = ::std::sync::atomic::AtomicU32::new(1);
mod std { pub mod process { pub fn id() -> u32 { crate::PID.load(::std::sync::atomic::Ordering::Relaxed) } } }
fn record(event: &'static str) { EVENTS.lock().unwrap().push(event); }
fn run_boot_checks() { record("checks"); }
fn finish_boot_checks() { record("finish"); }
fn reconcile_machine_state() { record("state"); }
fn run_boot_script() { record("script"); }
fn desktop_gpu_ready() -> bool { true }
fn spawn(_: &[u8]) -> Result<(), ()> { record("terminal"); Ok(()) }
mod init_services {
    pub enum Scope { Desktop }
    pub struct Supervisor;
    impl Supervisor {
        pub fn load(_: Scope, _: bool) -> Self { super::record("load"); Self }
        pub fn tick(&mut self) { super::record("tick"); }
        pub fn running(&self, _: &str) -> bool { true }
        pub fn run(self) -> ! { panic!("supervisor") }
    }
}
// FUNCTION
#[test]
fn desktop_boot_order() {
    for (pid, expected) in [
        (1, vec!["checks", "state", "script", "load", "tick", "finish", "terminal"]),
        (2, vec!["load", "tick", "terminal"]),
    ] {
        PID.store(pid, ::std::sync::atomic::Ordering::Relaxed);
        EVENTS.lock().unwrap().clear();
        let _ = ::std::panic::catch_unwind(run_desktop_mode);
        assert_eq!(*EVENTS.lock().unwrap(), expected);
    }
}
'''.replace('// FUNCTION', function)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'boot.rs').write_text(harness)
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings',
                            '--test', str(path / 'boot.rs'), '-o', str(path / 'boot')], check=True)
            subprocess.run([str(path / 'boot')], check=True)


if __name__ == '__main__':
    unittest.main()
