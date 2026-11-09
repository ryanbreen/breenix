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
struct Trace;
impl Trace{fn teardown_note(&self)->String{"teardown-stage=root-delete-begin at=99".into()}}
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


if __name__ == '__main__':
    unittest.main()
