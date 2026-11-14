"""Bounded pass/fail phase pages retain per-step app-output waits across fork."""
from pathlib import Path
import unittest
from ui_idle_capture_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class CasePhases(unittest.TestCase):
    def test_case_pages_retain_completed_and_inflight_waits(self):
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
    control::phase("case-begin");
    control::phase("launch-begin");
    let pid = unsafe { fork() }; assert!(pid >= 0);
    if pid == 0 {
        let watchdog = control::Watchdog::new(control::Wait::AppOutput).unwrap();
        assert_eq!(watchdog.deadline_ms(),10100);
        assert_eq!(watchdog.deadline_ns(),10_100_000_000);
        NOW.store(150, Ordering::Relaxed);
        control::phase("launch-done"); // In-flight wait split at this boundary.
        NOW.store(180, Ordering::Relaxed);
        drop(watchdog);
        NOW.store(200, Ordering::Relaxed);
        control::phase("stroke-done");
        for _ in 0..1000 { control::phase("launch-done"); control::phase("unknown"); }
        NOW.store(210, Ordering::Relaxed);
        let _unfinished = control::Watchdog::new(control::Wait::AppOutput).unwrap();
        NOW.store(230, Ordering::Relaxed);
        control::phase("stop-issued");
        assert_eq!(unsafe { kill(getpid(),9) },0);
        unsafe { _exit(99) }
    }
    let mut status = 0; assert_eq!(unsafe { waitpid(pid,&mut status,0) },pid);
    assert_eq!(status & 0x7f,9);
    let lines = scope.phases("gui-apps","guskit/smb-share-gone");
    assert_eq!(lines.len(),20,"{lines:?}");
    for line in &lines {
        assert!(line.starts_with("UI_PHASE gui-apps guskit/smb-share-gone phase="),"{line}");
        assert!(line.contains(" app_output_ms="),"{line}");
        assert!(line.len()<256,"{line}");
        assert!(!line.contains("unknown"));
    }
    let find = |name:&str| lines.iter().find(|l| l.contains(&format!(" name={name} "))).unwrap();
    assert!(find("launch-done").contains("at_ms=150 step_ms=50 app_output_ms=50 app_output_total_ms=50"));
    assert!(find("stroke-done").contains("at_ms=200 step_ms=50 app_output_ms=30 app_output_total_ms=80"));
    assert!(find("stop-issued").contains("at_ms=230 step_ms=30 app_output_ms=20 app_output_total_ms=100"));
    assert!(find("recovery").contains("at_ms=0 step_ms=0 app_output_ms=0 app_output_total_ms=0"));
    assert!(scope.report("gui-apps","case")[0].contains("wait=AppOutput started=2 completed=1"));
    assert!(scope.phases("other","case").is_empty(),"report only the instrumented case");
    drop(scope);
    let next = control::Scope::new().unwrap();
    assert!(next.phases("gui-apps","guskit/smb-share-gone").is_empty());
    NOW.store(300, Ordering::Relaxed);
    control::phase("case-begin");
    let watchdog = control::Watchdog::new(control::Wait::AppOutput).unwrap();
    NOW.store(340, Ordering::Relaxed);
    drop(watchdog);
    control::phase("finish-done");
    // The same API prints a normally completed case, with no failure predicate.
    let pass = next.phases("gui-apps","guskit/smb-share-gone");
    assert_eq!(pass.len(),20);
    assert!(pass.last().unwrap().contains("name=finish-done at_ms=340 step_ms=40 app_output_ms=40 app_output_total_ms=40"));
}
'''.replace('MODULE', str(ROOT / 'libs/libbreenix/src/suite_control.rs'))
        result = run_rust(source)
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
