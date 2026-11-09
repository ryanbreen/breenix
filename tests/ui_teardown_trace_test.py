"""Keep teardown phase evidence when a control deadline expires."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from ui_idle_capture_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class TeardownTrace(unittest.TestCase):
    def test_deadline_names_last_shared_phase_and_pass_stays_quiet(self):
        method = item((ROOT / 'libs/libuitest/src/session.rs').read_text(), '    pub fn teardown(')
        result = run_rust(r'''
#[derive(Clone)]struct ProcessExit{pid:u64}
enum Message{Teardown}
struct Channel;
impl Channel{fn send(&mut self,_:&Message)->Result<(),String>{Ok(())}}
pub struct Trace;
impl Trace{fn teardown_note(&self)->String{"teardown-stage=root-delete-begin at=99".into()}}
mod startup{pub use crate::Trace;}
struct Session{control:Channel,stopped:bool,exits:Vec<ProcessExit>,startup:Option<Trace>}
fn now()->Result<i128,String>{Ok(100)}
impl Session{
fn poll(&mut self,_:i128)->Result<(),String>{Err("session control deadline".into())}
METHOD
}
fn main(){
 let mut session=Session{control:Channel,stopped:false,exits:vec![ProcessExit{pid:42}],startup:Some(Trace)};
 let error=session.teardown().err().unwrap();
 assert!(error.contains("session control deadline"));
 assert!(error.contains("teardown-stage=root-delete-begin"),"teardown lost phase evidence: {error}");
 session.stopped=true;assert_eq!(session.teardown().unwrap()[0].pid,42);
 let _=session.startup.as_ref().unwrap().teardown_note();
}
'''.replace('METHOD', method))
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_shared_stamps_outlive_unlink_without_changing_startup_records(self):
        trace = (ROOT / 'libs/libuitest/src/startup.rs').read_text()
        result = run_rust(r'''
extern crate self as libbreenix;
pub mod time{
 pub struct Time;impl Time{pub fn as_nanos(&self)->u128{99}}
 pub fn now_monotonic()->Result<Time,String>{Ok(Time)}
}
#[path="TRACE"]mod startup;
fn main(){
 let root=std::env::temp_dir().join(format!("ui-teardown-{}",std::process::id()));
 std::fs::create_dir(&root).unwrap();
 let writer=startup::Trace::create(&root).unwrap();let reader=startup::Trace::open(&root).unwrap();
 assert_eq!(reader.teardown_note(),"teardown-stage=unreached at=0");
 writer.at("connect-begin",3);writer.mark("case-ready");
 let original=startup::phase_line(&reader.snapshot());
 assert!(startup::PHASES.len()<128);assert!(startup::verdict(99,true));
 std::env::set_var("BREENIX_CONFIG_ROOT",&root);startup::at("first-composite",7);startup::mark("app-commit");
 startup::teardown(1);assert_eq!(reader.teardown_note(),"teardown-stage=begin at=99");
 let baseline=startup::phase_line(&reader.snapshot());
 for stage in 2..=7{writer.teardown_mark(stage);}
 std::fs::remove_dir_all(&root).unwrap();
 writer.teardown_mark(5);assert_eq!(reader.teardown_note(),"teardown-stage=root-delete-begin at=99");
 writer.teardown_mark(6);assert_eq!(reader.teardown_note(),"teardown-stage=root-delete-done at=99");
 assert_eq!(startup::phase_line(&reader.snapshot()),baseline);
 assert!(!original.contains("teardown"));assert!(!baseline.contains("teardown"));
}
'''.replace('TRACE', str(ROOT / 'libs/libuitest/src/startup.rs')))
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
