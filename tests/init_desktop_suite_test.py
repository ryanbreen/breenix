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
        functions = (item(source, 'fn main()').replace('fn main()', 'fn init_main()', 1)
                     + (item(source, 'fn publish_log_boot_id(') if 'fn publish_log_boot_id(' in source else '')
                     + item(source, 'fn desktop_suite_ready(') + item(source, 'fn run_desktop_suite('))
        harness = r'''
use ::std::sync::Mutex;
use ::std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
static EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static ARGS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static IDS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static BOOT_ID: AtomicBool = AtomicBool::new(false);
static STALLED: AtomicBool = AtomicBool::new(false);
static STOP_FAIL: AtomicBool = AtomicBool::new(false);
static TICKS: AtomicU64 = AtomicU64::new(0);
fn event(name: &str) { EVENTS.lock().unwrap().push(name.into()); }
macro_rules! print { ($($arg:tt)*) => {{
    let line = format!($($arg)*);
    if line.starts_with("SUITE_SEQUENCE READY ") { event(line.trim_end()); }
    else if line.starts_with("SUITE_SEQUENCE ") && line.contains("FAIL") { event("failure"); }
}} }
macro_rules! println { ($($arg:tt)*) => { print!("{}\n", format!($($arg)*)) } }
fn run_boot_checks() { event("checks"); }
fn reconcile_machine_state() { event("state"); }
fn run_boot_script() { event("script"); }
fn finish_boot_checks() { event("finish"); }
fn check_test_controls(_: u64) { event("controls"); }
fn run_shell_mode() -> ! { panic!("unexpected shell") }
fn run_console_mode() -> ! { panic!("unexpected console") }
fn run_desktop_mode() -> ! { panic!("unexpected desktop") }
fn run_bssh_pubkey_oracle() { panic!("unexpected oracle") }
pub fn reap_forever() -> ! { event("blocked"); panic!("blocked") }
fn getpid() -> Result<libbreenix::process::Pid, String> { Ok(libbreenix::process::Pid) }
pub mod std {
 pub use ::std::path;
 pub mod env {
  pub fn args() -> ::std::vec::IntoIter<String> { crate::ARGS.lock().unwrap().clone().into_iter() }
 }
 pub mod os { pub mod unix { pub mod process {
  pub trait CommandExt { fn exec(&mut self) -> String; }
  impl CommandExt for crate::std::process::Command {
   fn exec(&mut self) -> String { crate::event("direct-exec"); "unexpected exec".into() }
  }
 } } }
 pub mod fs {
  pub fn read_to_string(path: &str) -> Result<String, String> {
   match path {
    "/etc/breenix/desktop-suite" => Ok("gui-apps\n".into()),
    "/run/breenix/boot-id" => if crate::BOOT_ID.load(crate::Ordering::Relaxed) { Ok("original\n".into()) } else { Err("missing".into()) },
    "/run/breenix/ntp-state" => Ok("1 0 0 site\n".into()),
    _ => panic!("unexpected read {path}"),
   }
  }
  pub fn create_dir_all(_: &str) -> Result<(), String> { Ok(()) }
  pub fn write(path: &str, _: String) -> Result<(), String> {
   assert_eq!(path, "/run/breenix/boot-id"); crate::event("boot-id"); crate::BOOT_ID.store(true, crate::Ordering::Relaxed); Ok(())
  }
 }
 pub mod time {
  pub struct Instant(u64);
  impl Instant {
   pub fn now() -> Self { Self(crate::TICKS.load(crate::Ordering::Relaxed) * 50) }
   pub fn elapsed(&self) -> ::std::time::Duration {
    ::std::time::Duration::from_millis(crate::TICKS.load(crate::Ordering::Relaxed) * 50 - self.0)
   }
  }
 }
 pub mod process {
  pub struct Command;
  pub struct Child;
  impl Command {
   pub fn new(path: impl AsRef<str>) -> Self { assert_eq!(path.as_ref(), "/sbin/suite-gui-apps"); Self }
   pub fn args(&mut self, args: impl IntoIterator<Item=String>) -> &mut Self {
    let args: Vec<_> = args.into_iter().collect();
    assert_eq!(args, crate::ARGS.lock().unwrap()[3..]); self
   }
   pub fn env(&mut self, key: &str, value: &str) -> &mut Self {
    assert_eq!((key, value), ("BREENIX_SUITE_SEQUENCE_MEMBER", "1")); crate::event("member"); self
   }
   pub fn spawn(&mut self) -> Result<Child, String> { crate::event("spawn"); Ok(Child) }
  }
  impl Child { pub fn id(&self) -> u32 { 9 } }
 }
}
pub mod breenix_suite { pub use crate::libbreenix::suite; }
pub mod libbreenix {
 pub mod etc_reconcile {
  pub fn prepare(_: &::std::path::Path) -> Result<Vec<String>, String> { crate::event("prepare"); Ok(vec![]) }
 }
 pub mod time {
  pub struct Stamp { pub tv_sec: u64, pub tv_nsec: u64 }
  pub fn now_realtime() -> Result<Stamp, String> { Ok(Stamp { tv_sec: 0, tv_nsec: 0 }) }
  pub fn now_monotonic() -> Result<Stamp, String> { now_realtime() }
  pub fn sleep_ms(_: u64) -> Result<(), ()> { Ok(()) }
 }
 pub mod process {
  pub struct Pid;
  impl Pid { pub fn raw(&self) -> u64 { 1 } }
  pub fn wifexited(status: i32) -> bool { status == 0 }
  pub fn wtermsig(status: i32) -> i32 { status }
  pub fn wexitstatus(status: i32) -> i32 { status >> 8 }
 }
 pub mod suite { pub mod sequence {
  pub fn read(id: &str) -> Option<(Vec<String>, usize)> {
   assert_eq!(id, "gui-apps"); let ids = crate::IDS.lock().unwrap().clone();
   let index = ids.iter().position(|name| name == id)?; Some((ids, index))
  }
  pub fn prepare_owner(ids: &[String], index: usize) {
   assert_eq!(ids, *crate::IDS.lock().unwrap());
   crate::event(if index == 0 { "owner-begin" } else { "owner-reset" });
  }
  pub fn finish() {
   let last = crate::IDS.lock().unwrap().last().unwrap() == "gui-apps";
   if last { crate::event("done"); }
   else { crate::event("hand-off-pid1"); panic!("exec next suite"); }
  }
 } }
}
pub mod init_services {
 pub enum Scope { Daemons, All }
 pub struct Supervisor { ticks: usize, watched: usize }
 impl Supervisor {
  pub fn check_service_io() { panic!("unexpected service check") }
  pub fn load(_: Scope, publish: bool) -> Self {
   assert!(publish); crate::event("load"); Self { ticks: 0, watched: 0 }
  }
  pub fn tick(&mut self) {
   self.ticks += 1; assert!(self.ticks < 605, "shares wait never ended");
   crate::TICKS.store(self.ticks as u64, crate::Ordering::Relaxed); crate::event("tick");
  }
  pub fn service_state(&self, name: &str) -> Option<&str> {
   if name == "ntp" { return None; }
   assert_eq!(name, "shares");
   Some(if self.ticks < 2 || crate::STALLED.load(crate::Ordering::Relaxed) { "running" } else { "done" })
  }
  pub fn tick_child(&mut self, pid: Option<i32>) -> Option<i32> {
   assert_eq!(pid, Some(9)); self.watched += 1; crate::event("supervise");
   if self.watched == 2 { Some(0) } else { None }
  }
  pub fn stop_for_sequence(&mut self) -> Result<(), String> {
   crate::event("stop");
   if crate::STOP_FAIL.load(crate::Ordering::Relaxed) { Err("mount remains".into()) } else { Ok(()) }
  }
  pub fn poweroff(&mut self) { crate::event("poweroff"); }
  pub fn run(self) -> ! { panic!("supervisor loop") }
 }
 pub fn check_service_io() { Supervisor::check_service_io(); }
}
// FUNCTIONS
fn run(mode: &str, ids: &[&str], entry: fn()) -> Vec<String> {
 TICKS.store(0, Ordering::Relaxed);
 EVENTS.lock().unwrap().clear();
 // The last-entry fixture retains a boot identity; the middle entry has none.
 BOOT_ID.store(mode == "suite" || ids.last() == Some(&"gui-apps"), Ordering::Relaxed);
 *IDS.lock().unwrap() = ids.iter().map(|s| s.to_string()).collect();
 let mut args: Vec<String> = ["/sbin/init", mode, "/sbin/suite-gui-apps", "--hostkeys", "fixture"]
  .iter().map(|s| s.to_string()).collect();
 if ids.is_empty() { args.splice(3..3, ["--case".into(), "guskit".into()]); }
 *ARGS.lock().unwrap() = args;
 let _ = ::std::panic::catch_unwind(entry);
 EVENTS.lock().unwrap().clone()
}
fn check_orderings(entry: fn()) {
 let mut mismatches = Vec::new();
 for (mode, ids, expected) in [
  ("suite", vec!["gui-apps", "time", "files-io"], vec!["prepare", "boot-id", "checks", "state", "script",
   "owner-begin", "load", "tick", "finish", "tick", "member", "spawn", "supervise", "supervise",
   "stop", "SUITE_SEQUENCE READY gui-apps", "hand-off-pid1"]),
  ("sequence", vec!["time", "gui-apps", "files-io"], vec!["owner-reset", "boot-id", "load", "tick", "tick",
   "member", "spawn", "supervise", "supervise", "stop", "SUITE_SEQUENCE READY gui-apps", "hand-off-pid1"]),
  ("sequence", vec!["time", "gui-apps"], vec!["owner-reset", "load", "tick", "tick", "member", "spawn",
   "supervise", "supervise", "stop", "SUITE_SEQUENCE READY gui-apps", "done", "poweroff"]),
  ("suite", vec![], vec!["prepare", "boot-id", "checks", "state", "script", "load", "tick", "finish",
   "tick", "spawn", "supervise", "supervise", "SUITE_SEQUENCE READY gui-apps", "poweroff"]),
 ] {
  let actual = run(mode, &ids, entry);
  let expected = if entry as usize == desktop_entry as usize && mode == "suite" {
   expected.into_iter().filter(|name| !matches!(*name, "prepare" | "boot-id")).collect()
  } else { expected };
  if actual != expected { mismatches.push((mode, ids, actual, expected)); }
 }
 assert!(mismatches.is_empty(), "lifecycle mismatches: {mismatches:#?}");
}
fn desktop_entry() {
 run_desktop_suite(1, "/sbin/suite-gui-apps", std::env::args().skip(3).collect());
}
#[test]
fn desktop_main_orderings_and_alone() { check_orderings(init_main); }
#[test]
fn desktop_sequence_orderings_and_alone() { check_orderings(desktop_entry); }
#[test]
fn refused_teardown_cannot_emit_ready_or_hand_off() {
 STOP_FAIL.store(true, Ordering::Relaxed);
 let actual = run("sequence", &["time", "gui-apps", "files-io"], init_main);
 STOP_FAIL.store(false, Ordering::Relaxed);
 assert_eq!(actual, ["owner-reset", "boot-id", "load", "tick", "tick", "member", "spawn", "supervise", "supervise",
  "stop", "failure", "blocked"]);
}
#[test]
fn stalled_shares_still_launches_other_cases() {
 STALLED.store(true, Ordering::Relaxed);
 let actual = run("suite", &[], init_main);
 STALLED.store(false, Ordering::Relaxed);
 assert!(actual.contains(&"spawn".into()), "shares held the runner indefinitely");
 assert!(actual.contains(&"supervise".into()));
 assert!(actual.contains(&"poweroff".into()));
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
