"""Exercise desktop startup failure for PID 1 and a suite-owned child init."""
from pathlib import Path
import subprocess
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class ChildFailure(unittest.TestCase):
    def test_child_reports_failure_without_starting_login_terminals(self):
        source = (ROOT / 'userspace/programs/src/init.rs').read_text()
        harness = r'''
fn run_boot_checks() {}
fn reconcile_machine_state() {}
fn run_boot_script() {}
fn finish_boot_checks() {}
mod std {
    pub mod process {
        pub fn id() -> u32 { if ::std::env::args().nth(1).as_deref() == Some("system") { 1 } else { 2 } }
        pub use ::std::process::exit;
    }
}
fn desktop_gpu_ready() -> bool { ::std::env::args().nth(2).as_deref() != Some("gpu") }
fn spawn(_: &[u8]) -> Result<(), ()> { Ok(()) }
mod init_services {
    pub enum Scope { Desktop }
    pub struct Supervisor;
    impl Supervisor {
        pub fn load(_: Scope, _: bool) -> Self { Self }
        pub fn tick(&mut self) {}
        pub fn running(&self, _: &str) -> bool { ::std::env::args().nth(2).as_deref() != Some("bwm") }
        pub fn fallback_to_console(&mut self) { println!("LOGIN-FALLBACK"); }
        pub fn run(self) -> ! { crate::std::process::exit(0) }
    }
}
// FUNCTION
fn main() { run_desktop_mode() }
'''.replace('// FUNCTION', item(source, 'fn run_desktop_mode('))
        with tempfile.TemporaryDirectory(prefix='init-child-') as tmp:
            path = Path(tmp)
            (path / 'check.rs').write_text(harness)
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings',
                            str(path / 'check.rs'), '-o', str(path / 'check')], check=True)
            for failure in ['gpu', 'bwm']:
                child = subprocess.run([str(path / 'check'), 'child', failure], capture_output=True, text=True)
                self.assertEqual(child.returncode, 1, child.stdout)
                self.assertNotIn('LOGIN-FALLBACK', child.stdout)
                system = subprocess.run([str(path / 'check'), 'system', failure], capture_output=True, text=True)
                self.assertEqual(system.returncode, 0, system.stdout)
                self.assertIn('LOGIN-FALLBACK', system.stdout)


if __name__ == '__main__':
    unittest.main()
