"""Exercise passing and failing reap exports, and pin the measurement exit paths."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class MeasurementExport(unittest.TestCase):
    def test_reap_exports_on_pass_and_fail(self):
        source = (ROOT / 'userspace/programs/src/wm_suite/desktop.rs').read_text()
        stop = item(source, '    pub fn stop(').replace('#[cfg(target_arch = "aarch64")]', '')
        report = item(source, 'fn report_reap(') if 'fn report_reap(' in source else ''
        # Proc snapshots are supplied by the fixture; all formatting and exit
        # control flow come from production, including the success path.
        report = report.replace('std::fs::read_to_string', 'read_fixture')
        run_rust(self, r'''
use std::sync::{Mutex,atomic::{AtomicU64,AtomicBool,Ordering}};
static NOW:AtomicU64=AtomicU64::new(1);
static PASS:AtomicBool=AtomicBool::new(false);
static LINES:Mutex<Vec<String>>=Mutex::new(Vec::new());
fn now()->u64{NOW.fetch_add(1_000_000,Ordering::Relaxed)}
fn pause(){} fn case_ms_left()->u64{10_000}
fn failure_diagnostic(label:&str,text:&str){LINES.lock().unwrap().push(format!("{label} {text}"));}
fn read_fixture(_:impl AsRef<std::path::Path>)->Result<String,std::io::Error>{
 Ok("17 11 0 0 100\n18 11 0 0 200\n19 11 0 0 300\n20 11 71 0 400\n21 11 0 0 500\n22 11 0 0 600\n17 99 0 0 100\n20 99 72 0 400\n".into())
}
mod signal{pub const SIGKILL:i32=9;}
struct Child{pid:i32,reap_ns:u64}
impl Child{
 fn signal(&self,_:i32)->Result<(),String>{Ok(())}
 fn status(&mut self)->Result<Option<i32>,String>{Ok(PASS.load(Ordering::Relaxed).then_some(9))}
 @STOP@
}
@UNTIL@
@DEADLINE@
@REPORT@
#[test]fn both_outcomes_export(){
 // Keep the fixture helpers used even before production gains the export.
 let _=read_fixture("unused");
 for pass in [true,false]{
  PASS.store(pass,Ordering::Relaxed); NOW.store(1,Ordering::Relaxed); LINES.lock().unwrap().clear();
  let mut child=Child{pid:71,reap_ns:0}; let result=child.stop(); assert_eq!(result.is_ok(),pass); let _elapsed=child.reap_ns;
  let lines=LINES.lock().unwrap();
  assert!(lines.iter().any(|l|l.starts_with("reap ")&&l.contains("kill_returned_ns=")),"missing kill return on pass={pass}: {lines:?}");
  assert!(lines.iter().any(|l|l.starts_with("reap-wait ")&&l.contains("wait_returned_ns=")),"missing wait return on pass={pass}: {lines:?}");
  for stage in 17..=22 { assert!(lines.iter().any(|l|l.starts_with("exit-phase ")&&l.contains(&format!("stage={stage} "))&&l.contains("tgid=71 ")),"missing group stage {stage} on pass={pass}: {lines:?}"); }
  assert!(!lines.iter().any(|l|l.starts_with("exit-phase ")&&l.contains("tid=99 ")),"unrelated process leaked");
 }
}
'''.replace('@STOP@', stop).replace('@UNTIL@', item(source, 'pub fn until('))
           .replace('@DEADLINE@', item(source, 'pub fn until_from(')).replace('@REPORT@', report))

    def test_observation_and_startup_exports_are_unconditional(self):
        desktop = (ROOT / 'userspace/programs/src/wm_suite/desktop.rs').read_text()
        startup = (ROOT / 'libs/libuitest/src/startup.rs').read_text()
        compositor = (ROOT / 'userspace/programs/src/bwm/compositor.rs').read_text()
        cases = (ROOT / 'userspace/programs/src/wm_suite/cases.rs').read_text()
        self.assertIn('self.failure();', item(desktop, 'impl Drop for StartupTrace'))
        self.assertIn('self.export();', item(desktop, 'impl Drop for WaitObservation'))
        for phase in ['band-begin', 'band-scanout', 'band-desktop', 'band-pixels',
                      'whole-check-return', 'report-begin', 'report-return', 'join-begin', 'join-return']:
            self.assertIn('"' + phase + '"', startup)
        for phase in ['band-begin', 'band-scanout', 'band-desktop', 'band-pixels',
                      'whole-check-return', 'report-begin', 'report-return']:
            self.assertIn('"' + phase + '"', compositor)
        floor = item(cases, 'pub fn frame_floor(')
        self.assertLess(floor.index('"frame-endpoints"'), floor.index('d.shown(&c)?'))
        window = (ROOT / 'kernel/src/syscall/window.rs').read_text()
        readback = item(window, 'fn readback(')
        for branch in ['busy', 'claim-baseline', 'gpu-fallback']:
            self.assertIn('"' + branch + '"', readback)
        self.assertIn('"/proc/breenix/window-readback"', desktop)


if __name__ == '__main__':
    unittest.main()
