"""Gus Kit selections enter PID-1 init before the GUI runner is spawned."""
import subprocess
import os
import tempfile
import unittest
from pathlib import Path

from gt710_gr_live_unwind_test import item
import metal_smb_test

ROOT = Path(__file__).resolve().parents[1]


class DesktopSuiteLaunch(unittest.TestCase):
    def test_staging_selects_init_desktop_only_for_guskit(self):
        fixture = metal_smb_test.SmbStagingTest()
        for case in (None, 'guskit', 'guskit/smb-save-open', 'bterm'):
            with self.subTest(case=case), fixture.gui_root() as (image, read, config):
                import metal_root
                metal_root.stage_root(image, 'suite', 'gui-apps', {}, smb_config=config, case=case)
                content = read('cat /etc/breenix/desktop-suite').stdout
                self.assertEqual(content, b'gui-apps\n' if case != 'bterm' else b'')
                if case != 'bterm':
                    metal_root.stage_root(image, 'suite', 'gui-apps', {}, smb_config=config, case='bterm')
                    self.assertEqual(read('cat /etc/breenix/desktop-suite').stdout, b'')
        kernel = (ROOT / 'kernel/src/main.rs').read_text()
        self.assertTrue('name == "suite-users" || name == "suite-gui-apps"' in kernel)
        self.assertTrue('id == "users" || id == "gui-apps"' in kernel)

    def test_launch_waits_for_shares_and_excludes_nested_init(self):
        source = (ROOT / 'userspace/programs/src/init.rs').read_text()
        decision = item(source, 'fn desktop_suite_ready(')
        harness = r'''
// DECISION
#[test]
fn launch_decision() {
    assert!(desktop_suite_ready(1, true, None, 0));
    assert!(desktop_suite_ready(1, true, Some("running"), 30_000));
    assert!(!desktop_suite_ready(1, true, Some("running"), 29_999));
    for state in [Some("waiting"), Some("running"), Some("backoff"), Some("stopping")] {
        assert!(!desktop_suite_ready(1, true, state, 0));
    }
    for state in ["done", "failed", "stopped", "crash-loop"] {
        assert!(desktop_suite_ready(1, true, Some(state), 0));
        assert!(!desktop_suite_ready(2, true, Some(state), 0));
        assert!(!desktop_suite_ready(1, false, Some(state), 0));
    }
}
'''.replace('// DECISION', decision)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'launch.rs').write_text(harness)
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings', '--test',
                            str(path / 'launch.rs'), '-o', str(path / 'launch')], check=True)
            subprocess.run([str(path / 'launch')], check=True)

    def test_init_keeps_supervising_until_the_suite_exits(self):
        rel = 'userspace/programs/src/init.rs'
        old = os.environ.get('INIT_OLD_COMMIT')
        source = subprocess.check_output(['git', 'show', f'{old}:{rel}'], text=True) if old else (ROOT / rel).read_text()
        functions = item(source, 'fn desktop_suite_ready(') + item(source, 'fn run_desktop_suite(')
        harness = r'''
use ::std::sync::Mutex;
static EVENTS: Mutex<Vec<&str>> = Mutex::new(Vec::new());
static STALLED: ::std::sync::atomic::AtomicBool = ::std::sync::atomic::AtomicBool::new(false);
static TICKS: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);
fn event(name: &'static str) { EVENTS.lock().unwrap().push(name); }
fn run_boot_checks() { event("checks"); }
fn reconcile_machine_state() { event("state"); }
fn run_boot_script() { event("script"); }
fn finish_boot_checks() { event("finish"); }
mod std {
 pub mod fs {
  pub fn read_to_string(_: &str) -> Result<String, String> { Ok("1 0 0 site\n".into()) }
 }
 pub mod time {
  pub struct Instant(u64);
  impl Instant {
   pub fn now() -> Self { Self(crate::TICKS.load(::std::sync::atomic::Ordering::Relaxed) * 50) }
   pub fn elapsed(&self) -> ::std::time::Duration {
    ::std::time::Duration::from_millis(crate::TICKS.load(::std::sync::atomic::Ordering::Relaxed) * 50 - self.0)
   }
  }
 }
 pub mod process {
    pub struct Command;
    pub struct Child;
    impl Command {
        pub fn new(path: &str) -> Self { assert_eq!(path, "/sbin/suite-gui-apps"); Self }
        pub fn args(self, args: Vec<String>) -> Self {
            assert_eq!(args, ["--case", "guskit", "--hostkeys", "fixture"]); Self
        }
        pub fn spawn(self) -> Result<Child, String> { crate::event("spawn"); Ok(Child) }
    }
    impl Child { pub fn id(&self) -> u32 { 9 } }
} }
mod libbreenix {
    pub mod time { pub fn sleep_ms(_: u64) -> Result<(), ()> { Ok(()) } }
    pub mod process {
        pub fn wifexited(status: i32) -> bool { status == 0 }
        pub fn wtermsig(status: i32) -> i32 { status }
        pub fn wexitstatus(status: i32) -> i32 { status >> 8 }
    }
}
mod init_services {
    pub enum Scope { Daemons }
    pub struct Supervisor { ticks: usize, watched: usize }
    impl Supervisor {
        pub fn load(_: Scope, publish: bool) -> Self {
            assert!(publish); crate::event("load"); Self { ticks: 0, watched: 0 }
        }
        pub fn tick(&mut self) {
            self.ticks += 1; assert!(self.ticks < 605, "shares wait never ended");
            crate::TICKS.store(self.ticks as u64, ::std::sync::atomic::Ordering::Relaxed);
            crate::event("tick");
        }
        pub fn service_state(&self, name: &str) -> Option<&str> {
            if name == "ntp" { return None; }
            assert_eq!(name, "shares"); Some(if self.ticks < 2 || crate::STALLED.load(::std::sync::atomic::Ordering::Relaxed) { "running" } else { "done" })
        }
        pub fn tick_child(&mut self, pid: Option<i32>) -> Option<i32> {
            assert_eq!(pid, Some(9)); self.watched += 1; crate::event("supervise");
            if self.watched == 2 { Some(0) } else { None }
        }
        pub fn poweroff(&mut self) { crate::event("poweroff"); }
        pub fn run(self) -> ! { panic!("supervisor loop") }
    }
}
// FUNCTIONS
#[test]
fn desktop_child_lifecycle() {
    STALLED.store(false, ::std::sync::atomic::Ordering::Relaxed);
    TICKS.store(0, ::std::sync::atomic::Ordering::Relaxed);
    EVENTS.lock().unwrap().clear();
    let _ = ::std::panic::catch_unwind(|| run_desktop_suite(1, "/sbin/suite-gui-apps",
        ["--case", "guskit", "--hostkeys", "fixture"].iter().map(|s| s.to_string()).collect()));
    assert_eq!(*EVENTS.lock().unwrap(), ["checks", "state", "script", "load", "tick", "finish",
        "tick", "spawn", "supervise", "supervise", "poweroff"]);
}
#[test]
fn stalled_shares_still_launches_other_cases() {
    STALLED.store(true, ::std::sync::atomic::Ordering::Relaxed);
    TICKS.store(0, ::std::sync::atomic::Ordering::Relaxed);
    EVENTS.lock().unwrap().clear();
    let _clock = std::time::Instant::now().elapsed();
    let _ = ::std::panic::catch_unwind(|| run_desktop_suite(1, "/sbin/suite-gui-apps",
        ["--case", "guskit", "--hostkeys", "fixture"].iter().map(|s| s.to_string()).collect()));
    let events = EVENTS.lock().unwrap();
    assert!(events.contains(&"spawn"), "shares held the runner indefinitely");
    assert!(events.contains(&"supervise"));
    assert!(events.contains(&"poweroff"));
}
'''.replace('// FUNCTIONS', functions)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'lifecycle.rs').write_text(harness)
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings', '--test',
                            str(path / 'lifecycle.rs'), '-o', str(path / 'lifecycle')], check=True)
            subprocess.run([str(path / 'lifecycle'), '--test-threads=1'], check=True)


if __name__ == '__main__':
    unittest.main()
