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
 assert_eq!((title,end),("startup app",300001000));
 if FAIL.load(std::sync::atomic::Ordering::Relaxed){Err("planted startup failure".into())}else{Ok(())}
}
fn check(ok:bool,message:&str)->CaseResult{if ok{Ok(())}else{Err(message.into())}}
pub mod breenix_suite { pub mod suite {
 pub fn value(name:&str,ns:i64,unit:&str,bound:Option<(i64,i64)>){
  if name=="startup"{assert_eq!((ns,unit,bound),(0,"ns",Some((0,300000000))));}
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
