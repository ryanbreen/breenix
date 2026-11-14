"""Failure-only phase evidence must survive a case killed at its deadline."""
from pathlib import Path
import unittest
from ui_idle_capture_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class CasePhases(unittest.TestCase):
    def test_killed_case_retains_bounded_stamps_without_success_output(self):
        source = r'''extern crate self as libbreenix;
use std::sync::atomic::{AtomicU64, Ordering};
static NOW: AtomicU64 = AtomicU64::new(100);
pub mod time {
    pub struct Stamp(u64);
    impl Stamp { pub fn as_nanos(&self)->i128 { i128::from(self.0)*1_000_000 } }
    pub fn now_monotonic()->Result<Stamp,String> {
        Ok(Stamp(super::NOW.load(super::Ordering::Relaxed)))
    }
}
#[path="MODULE"] mod control;
extern "C" { fn fork()->i32; fn waitpid(p:i32,s:*mut i32,f:i32)->i32;
    fn getpid()->i32; fn kill(p:i32,s:i32)->i32; fn _exit(s:i32)->!; }
fn main() {
    let scope = control::Scope::new().unwrap();
    // Exercise the existing watchdog on the same page as the phase stamps.
    let watchdog = control::Watchdog::new(control::Wait::AppOutput).unwrap();
    assert_eq!(watchdog.deadline_ms(),10100);
    assert_eq!(watchdog.deadline_ns(),10_100_000_000);
    control::phase("launch-begin");
    let pid = unsafe { fork() }; assert!(pid >= 0);
    if pid == 0 {
        NOW.store(150, Ordering::Relaxed);
        control::phase("launch-done");
        for _ in 0..1000 { control::phase("launch-done"); control::phase("unknown"); }
        NOW.store(200, Ordering::Relaxed);
        control::phase("stop-issued");
        assert_eq!(unsafe { kill(getpid(),9) },0);
        unsafe { _exit(99) }
    }
    let mut status = 0; assert_eq!(unsafe { waitpid(pid,&mut status,0) },pid);
    assert_eq!(status & 0x7f,9);
    drop(watchdog);
    assert!(scope.failure_phases("gui-apps","guskit/smb-share-gone",false).is_empty());
    let lines = scope.failure_phases("gui-apps","guskit/smb-share-gone",true);
    assert_eq!(lines.len(),1,"{lines:?}");
    let line = &lines[0];
    assert!(line.starts_with("UI_PHASE gui-apps guskit/smb-share-gone "),"{line}");
    assert!(line.contains("launch-begin=100 launch-done=150"),"{line}");
    assert!(line.contains("stop-issued=200 stop-written=0"),"{line}");
    assert!(line.contains("recovery=0"),"{line}");
    assert!(!line.contains("unknown"));
    assert!(line.len()<2048);
    assert!(scope.report("gui-apps","case")[0].contains("wait=AppOutput started=1 completed=1"));
    drop(scope);
    let next = control::Scope::new().unwrap();
    assert!(next.failure_phases("next","case",true).is_empty());
}
'''.replace('MODULE', str(ROOT / 'libs/libbreenix/src/suite_control.rs'))
        result = run_rust(source)
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
