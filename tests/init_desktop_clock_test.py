"""Run the production desktop-suite startup with shares ready before NTP."""
from pathlib import Path
import subprocess
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class DesktopClock(unittest.TestCase):
    def test_initial_clock_step_precedes_cases_or_launch_is_refused(self):
        source = (ROOT / 'userspace/programs/src/init.rs').read_text()
        functions = item(source, 'fn desktop_suite_ready(') + item(source, 'fn run_desktop_suite(')
        harness = r'''
use ::std::sync::atomic::{AtomicU64, Ordering};
static NOW: AtomicU64 = AtomicU64::new(0);
static CLOCK_AT: AtomicU64 = AtomicU64::new(200);
static SPAWN_AT: AtomicU64 = AtomicU64::new(0);
fn publish_log_boot_id(_: bool) { panic!("standalone log identity already prepared") }
fn run_boot_checks() {}
fn reconcile_machine_state() {}
fn run_boot_script() {}
fn finish_boot_checks() {}
pub fn reap_forever() -> ! { panic!("teardown failed") }
pub mod std {
 pub use ::std::env;
 pub mod time {
  pub struct Instant(u64);
  impl Instant {
   pub fn now() -> Self { Self(crate::NOW.load(crate::Ordering::Relaxed)) }
   pub fn elapsed(&self) -> ::std::time::Duration {
    ::std::time::Duration::from_millis(crate::NOW.load(crate::Ordering::Relaxed) - self.0)
   }
  }
 }
 pub mod fs {
  pub fn read_to_string(path: &str) -> Result<String, String> {
   assert_eq!(path, "/run/breenix/ntp-state");
   if crate::NOW.load(crate::Ordering::Relaxed) >= crate::CLOCK_AT.load(crate::Ordering::Relaxed) {
    Ok("1 200 200 site\n".into())
   } else { Err("not synchronized".into()) }
  }
 }
 pub mod process {
  pub struct Command;
  pub struct Child;
  impl Command {
   pub fn new(_: &str) -> Self { Self }
   pub fn args(&mut self, _: Vec<String>) -> &mut Self { self }
   pub fn env(&mut self, _: &str, _: &str) -> &mut Self { panic!("standalone member") }
   pub fn spawn(&mut self) -> Result<Child, String> {
    crate::SPAWN_AT.store(crate::NOW.load(crate::Ordering::Relaxed), crate::Ordering::Relaxed);
    Ok(Child)
   }
  }
  impl Child { pub fn id(&self) -> u32 { 9 } }
 }
}
pub mod breenix_suite { pub use crate::libbreenix::suite; }
pub mod libbreenix {
 pub mod suite { pub mod sequence {
  pub fn read(_: &str) -> Option<(Vec<String>, usize)> { None }
  pub fn prepare_owner(_: &[String], _: usize) { panic!("standalone owner") }
  pub fn finish() { panic!("standalone handoff") }
 } }
 pub mod time { pub fn sleep_ms(_: u64) -> Result<(), ()> { Ok(()) } }
 pub mod process {
  pub fn wifexited(_: i32) -> bool { true }
  pub fn wtermsig(status: i32) -> i32 { status }
  pub fn wexitstatus(status: i32) -> i32 { status >> 8 }
 }
}
pub mod init_services {
 pub enum Scope { Daemons }
 pub struct Supervisor;
 impl Supervisor {
  pub fn load(_: Scope, _: bool) -> Self { Self }
  pub fn tick(&mut self) { crate::NOW.fetch_add(50, crate::Ordering::Relaxed); }
  pub fn service_state(&self, name: &str) -> Option<&str> {
   match name { "shares" => Some("done"), "ntp" => Some("running"), _ => panic!("unknown service") }
  }
  pub fn tick_child(&mut self, _: Option<i32>) -> Option<i32> { Some(0) }
  pub fn stop_for_sequence(&mut self) -> Result<(), String> { panic!("standalone stop") }
  pub fn poweroff(&mut self) {}
  pub fn run(self) -> ! { panic!("end of init loop") }
 }
}
// FUNCTIONS
fn main() {
 for clock_at in [200, u64::MAX] {
  NOW.store(0, Ordering::Relaxed);
  SPAWN_AT.store(0, Ordering::Relaxed);
  CLOCK_AT.store(clock_at, Ordering::Relaxed);
  let _ = ::std::panic::catch_unwind(|| run_desktop_suite(1, "/sbin/suite-gui-apps", vec![]));
  let spawned = SPAWN_AT.load(Ordering::Relaxed);
  if clock_at == u64::MAX {
   assert_eq!(spawned, 0, "cases launched while NTP could still step realtime");
   assert!(NOW.load(Ordering::Relaxed) >= 30_000);
  } else {
   assert!(spawned >= clock_at, "cases started at {spawned} before initial clock step at {clock_at}");
  }
 }
}
'''.replace('// FUNCTIONS', functions)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'clock.rs').write_text(harness)
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings',
                            str(path / 'clock.rs'), '-o', str(path / 'clock')], check=True)
            result = subprocess.run([str(path / 'clock')], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == '__main__':
    unittest.main()
