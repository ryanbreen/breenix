"""Execute the runner's production collection branch with controlled supervisor replies."""
from pathlib import Path
import os
import subprocess
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class RunnerCollection(unittest.TestCase):
    def test_request_selection_primary_failure_and_fallback_record(self):
        rel = 'libs/libbreenix/src/suite.rs'
        baseline = os.environ.get('UITEST_OLD_COMMIT')
        source = subprocess.check_output(['git', 'show', f'{baseline}:{rel}'], cwd=ROOT, text=True) if baseline else (ROOT / rel).read_text()
        start = source.index('            let outcome = if let Some(session) = session.as_mut()')
        end = source.index('            if screen.gui {', start)
        body = source[start:end]
        harness = r'''
use std::sync::Mutex;
#[path="RESULT"]pub mod result;
#[path="COLLECTION"]pub mod collection;
use collection::{Request,Trigger};
const MSG_MAX:usize=160;
#[derive(Debug,PartialEq)]pub enum Outcome{Pass{ms:u64},Fail{ms:u64,msg:String},Skip{msg:String}}
NOTES_IMPL
static CALL:Mutex<Option<Request>>=Mutex::new(None);
static RECORDS:Mutex<Vec<String>>=Mutex::new(Vec::new());
fn emit(line:&str){RECORDS.lock().unwrap().push(line.trim().into());}
fn ms_since(_:Option<i128>)->u64{1500}
struct Case{collects:bool}
pub struct Results{pub accounting:result::Accounting<'static>}
impl Results{pub fn drain(&mut self){}}
pub struct Session{error:Option<&'static str>}
impl Session{
    pub fn finish(&mut self,left:u64)->Result<(),String>{assert_eq!(left,1500);Ok(())}
    pub fn collect_until(&mut self,deadline:Option<i128>,request:Request,suite:&str,name:&str,mut record:impl FnMut(&str)->Result<(),String>)->Result<(),String>{
        assert_eq!(deadline,Some(3_000_000_000));*CALL.lock().unwrap()=Some(request);
        if let Some(error)=self.error{return Err(error.into());}
        if let Request::Collect(trigger)=request{record(&format!("SUITE {suite} COLLECT {name} trigger={} screen=none:unavailable windows=0 remnants=0 artifacts=none:unavailable",trigger.name()))?;}
        Ok(())
    }
}
fn run(collects:bool,primary:bool,trigger:Trigger,error:Option<&'static str>,missing:bool)->Outcome{
    let mut session=Some(Session{error});let case=Case{collects};let limit_ms:u64=3000;let start=Some(0);let suite="test";let name="cat/case";
    let mut results=Some(Results{accounting:result::Accounting::new("test","cat/case",if missing{&["label"]}else{&[]},collects).unwrap()});
    // The old branch has no accounting input before its discard-only finish.
    let _ = (&case.collects,&trigger,&mut results);
    let outcome=if primary{Outcome::Fail{ms:100,msg:"primary".into()}}else{Outcome::Pass{ms:100}};
METHOD_IMPL
outcome
}
#[test]fn actual_runner_selects_and_reports(){
    for (primary,trigger,error,expected) in [(true,Trigger::Fail,None,Some(Request::Collect(Trigger::Fail))),
        (true,Trigger::Timeout,Some("collector-timeout"),Some(Request::Collect(Trigger::Timeout))),
        (true,Trigger::Crash,Some("session supervisor died"),Some(Request::Collect(Trigger::Crash))),
        (false,Trigger::Fail,None,Some(Request::Discard))]{
        *CALL.lock().unwrap()=None;RECORDS.lock().unwrap().clear();
        let outcome=run(true,primary,trigger,error,false);assert_eq!(*CALL.lock().unwrap(),expected);
        if primary{assert_eq!(outcome,Outcome::Fail{ms:100,msg:"primary".into()});}
        else{assert_eq!(outcome,Outcome::Pass{ms:100});}
        let records=RECORDS.lock().unwrap();
        if let Some(error)=error{
            let reason=if error=="session supervisor died"{"supervisor-died"}else{"collector-timeout"};
            assert_eq!(*records,[format!("SUITE test COLLECT cat/case trigger={} screen=none:{reason} windows=0 remnants=0 artifacts=none:{reason}",trigger.name())]);
        }
    }
    *CALL.lock().unwrap()=None;
    assert!(matches!(run(true,false,Trigger::Fail,Some("session supervisor died"),false),Outcome::Fail{msg,..}if msg=="session supervisor died"));
    assert!(matches!(run(true,false,Trigger::Fail,None,true),Outcome::Fail{msg,..}if msg.contains("frame check `label` never ran")));
    assert_eq!(*CALL.lock().unwrap(),Some(Request::Collect(Trigger::Fail)));
    *CALL.lock().unwrap()=None;run(false,true,Trigger::Fail,None,false);assert!(CALL.lock().unwrap().is_none());
}
'''.replace('RESULT', str(ROOT / 'libs/libbreenix/src/suite_result.rs')).replace('COLLECTION', str(ROOT / 'libs/libbreenix/src/suite_collection.rs')).replace('METHOD_IMPL', body)
        notes = item(source, 'fn plain(') + '\n' + item(source, 'fn with_note(').replace('fn with_note(', 'pub fn with_note(')
        run_rust(self, harness.replace('NOTES_IMPL', notes))


if __name__ == '__main__':
    unittest.main()
