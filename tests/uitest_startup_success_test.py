"""Run the production startup case through passing and failing sessions."""
from pathlib import Path
import os
import subprocess
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT = Path(__file__).resolve().parents[1]

def source(path):
    old = os.environ.get('UITEST_OLD_COMMIT')
    return subprocess.check_output(['git', 'show', f'{old}:{path}'], cwd=ROOT, text=True) if old else (ROOT/path).read_text()

class StartupSuccess(unittest.TestCase):
    def test_comparison_measurement_survives_the_shared_name_validator(self):
        runner = source('libs/libbreenix/src/suite.rs')
        caller = source('userspace/programs/src/uitest_suite/comparison_cases.rs')
        call = next(line.strip() for line in caller.splitlines() if 'breenix_suite::suite::value(' in line)
        run_rust(self, r'''
use std::{fmt::Write, sync::Mutex};
static OUTPUT: Mutex<String> = Mutex::new(String::new());
fn record(kind: &str, rest: &str) { *OUTPUT.lock().unwrap() = format!("{kind} {rest}"); }
@WORD@
@VALUE@
#[test] fn emitted_comparison_measurement() {
    let elapsed = 123u64;
    @CALL@
    assert_eq!(&*OUTPUT.lock().unwrap(), "VALUE compare-ns=123ns expect=0..500000000ns");
}
'''.replace('@WORD@', item(runner, 'fn is_word('))
           .replace('@VALUE@', item(runner, 'pub fn value('))
           .replace('@CALL@', call.replace('breenix_suite::suite::', '')))

    def test_startup_verdict_measurement_boundaries(self):
        trace = source('libs/libuitest/src/startup.rs').replace('//!', '//')
        run_rust(self, r'''
pub mod libbreenix { pub mod time {pub fn now_monotonic()->Result<std::time::Duration,()> {Ok(std::time::Duration::ZERO)}} }
@TRACE@
#[test] fn measurement_boundaries() {
    assert!(verdict(400_000_000, true));
    assert!(verdict(1_900_000_000, true));
    assert!(verdict(2_000_000_000, true));
    assert!(!verdict(2_100_000_000, true));
    assert!(!verdict(0, false));
}
'''.replace('@TRACE@', trace))

    def test_every_start_reports_one_bounded_phase_line(self):
        body = item(source('userspace/programs/src/uitest_suite/sessions.rs'), 'fn start(kiosk: bool)')
        trace = source('libs/libuitest/src/startup.rs').replace('//!', '//')
        run_rust(self, r'''
use std::sync::Mutex;
static OUTPUT: Mutex<String> = Mutex::new(String::new());
static FAIL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
macro_rules! println {($($args:tt)*) => {{OUTPUT.lock().unwrap().push_str(&format!($($args)*)); OUTPUT.lock().unwrap().push('\n');}};}
pub mod libbreenix { pub mod time {pub fn now_monotonic()->Result<std::time::Duration,()> {Ok(std::time::Duration::from_nanos(99))}} }
pub mod libuitest { pub mod startup { use crate::libbreenix; @TRACE@ } }
type CaseResult = Result<(), String>;
fn monotonic_ns()->Result<u64,String>{Ok(1000)}
struct Session;
impl Session {
 fn kiosk()->Result<Self,String>{Ok(Self)} fn desktop()->Result<Self,String>{Ok(Self)}
 fn startup_times(&self)->Vec<(&'static str,u64)>{libuitest::startup::PHASES.iter().enumerate().map(|(i,&p)|(p,if i==0{100}else{200+i as u64})).collect()}
 fn teardown(&mut self)->CaseResult{Ok(())}
}
fn first_window(_: &mut Session,title:&str,end:u64)->CaseResult {
 assert_eq!((title,end),("startup app",2000001000));
 if FAIL.load(std::sync::atomic::Ordering::Relaxed){Err("planted startup failure".into())}else{Ok(())}
}
fn check(ok:bool,message:&str)->CaseResult{if ok{Ok(())}else{Err(message.into())}}
pub mod breenix_suite { pub mod suite {
 pub fn value(name:&str,ns:i64,unit:&str,bound:Option<(i64,i64)>){
  if name=="startup"{assert_eq!((ns,unit,bound),(0,"ns",Some((0,2000000000))));}
  super::super::OUTPUT.lock().unwrap().push_str(&format!("VALUE {name}={ns}{unit}\n"));
 }
}}
@BODY@
#[test]fn emits_on_success_and_error(){
 println!("test output hook");
 for failed in [false,true] {for kiosk in [false,true] {
  OUTPUT.lock().unwrap().clear();FAIL.store(failed,std::sync::atomic::Ordering::Relaxed);
  assert_eq!(start(kiosk).is_err(),failed);
  let output=OUTPUT.lock().unwrap();
  assert!(output.contains("VALUE startup=0ns\n"), "missing measurement on success or failure: {output}");
  let lines:Vec<_>=output.lines().filter(|s|s.starts_with("UITEST_STARTUP ")).collect();
  assert_eq!(lines.len(),1,"each start needs one phase line, including passes: {output}");
  let line=lines[0];assert!(line.len()<4096,"unbounded phase record");
  for (i,phase) in libuitest::startup::PHASES.iter().enumerate(){
   let ns=if i==0{100}else{200+i as u64};
   assert!(line.split_whitespace().any(|s|s==format!("{phase}={ns}")),"missing {phase}: {line}");
  }
 }}
}
'''.replace('@TRACE@', trace).replace('@BODY@', body))

if __name__ == '__main__': unittest.main()
