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
        # Proc snapshots are supplied by the fixture; formatting and exit
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

    def test_bounded_payloads_keep_band_times_and_refusal_identity(self):
        startup = (ROOT / 'libs/libuitest/src/startup.rs').read_text().replace('//!', '//')
        ring = (ROOT / 'kernel/src/window/readback_trace.rs').read_text().replace('//!', '//')
        ring = ring.replace('use alloc::', 'use std::').replace('spin::Mutex', 'crate::HostMutex')
        run_rust(self, r'''
struct HostMutex<T>(std::sync::Mutex<T>);
impl<T> HostMutex<T>{const fn new(value:T)->Self{Self(std::sync::Mutex::new(value))}fn lock(&self)->std::sync::MutexGuard<'_,T>{self.0.lock().unwrap()}}
pub mod startup{
@STARTUP@
mod libbreenix{
 pub struct Timespec; impl Timespec{pub fn new()->Self{Self}pub fn as_nanos(&self)->u128{0}}
 pub mod time{pub fn now_monotonic()->Result<std::time::Duration,()>{Ok(std::time::Duration::from_nanos(99))}pub fn clock_gettime(_:i32,_:&mut super::Timespec)->Result<(),()>{Ok(())}}
 pub mod process{pub struct Pid;impl Pid{pub fn raw(&self)->u64{1}}pub fn getpid()->Result<Pid,()>{Ok(Pid)}}
}
#[test]fn bands_do_not_alias_phase_cpu_or_teardown_slots(){
 let root=std::env::temp_dir().join(format!("a3-detail-{}",std::process::id()));std::fs::create_dir(&root).unwrap();
 let writer=Trace::create(&root).unwrap();let reader=Trace::open(&root).unwrap();
 writer.cpu_add(0,123);writer.teardown_mark(1);
 for y in [0,409,818,1227]{writer.detail("band-begin",0,y,409);writer.detail("band-pixels",0,y,409);}
 assert_eq!(reader.details().len(),9);assert!(reader.details().iter().any(|l|l.contains("phase=band-pixels subject=0 y=1227 rows=409 ns=99")));
 assert_eq!(reader.cpu_snapshot()[0],123);assert!(reader.teardown_note().contains("begin"));
 assert!(reader.snapshot().iter().all(|(_,ns)|*ns==0));
 for _ in 0..50{writer.detail("join-return",0,0,0);}
 assert_eq!(reader.details()[0],"events=58 dropped=10");
 writer.mark("checks-ready");writer.detail("band-begin",0,0,1);assert_eq!(reader.details()[0],"events=58 dropped=10");
 drop(reader);drop(writer);std::fs::remove_dir_all(root).unwrap();
}
}
pub mod readback{
@RING@
#[test]fn refusal_ring_exports_identity_and_overwrite_count(){
 for i in 0..CAPACITY+5{record("busy",[i as u64,2,3,4,5,6,7,8,1,9,0,10]);}
 let out=text();assert!(out.starts_with("retained=2048 overwritten=5\n"));
 assert!(out.contains("seq=5 begin_ns=5 end_ns=2 branch=busy observer=3 tid=4"));
 assert!(out.contains("claimant=5 epoch=6 first=7 serial=8 busy=1"));assert!(out.contains("completed=9 flags=0 ack=10"));
 record("retained-writer-fallback",[1,2,3,4,5,6,7,8,0,0,0,11]);let out=text();
 assert!(out.contains("writer_ctx=5 writer_serial=6 fence=7 answer=8 errno=11"));
}
}
'''.replace('@STARTUP@', startup).replace('@RING@', ring))

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
