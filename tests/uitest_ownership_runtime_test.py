"""Execute the production reap path with a compositor exit, without a guest."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class ReapOwnership(unittest.TestCase):
    def test_exit_report_follows_display_and_compositor_recovery(self):
        path = 'libs/libuitest/src/supervisor_runtime.rs'
        baseline = os.environ.get('UITEST_OLD_COMMIT')
        source = subprocess.check_output(['git', 'show', f'{baseline}:{path}'], cwd=ROOT, text=True) if baseline else (ROOT / path).read_text()
        body = item(source, 'fn reap(')
        call = 'reap(&mut channel, &mut outputs, true'
        if 'ownership: &mut Ownership' in body:
            call += ', Some(7), &mut ownership, true'
        call += ').unwrap();'
        harness = r'''
use std::{collections::BTreeMap, sync::Mutex};
#[path = "OWNERSHIP"] pub mod ownership;
use ownership::{Custody, Event, Ownership};
static EVENTS: Mutex<Vec<&str>> = Mutex::new(Vec::new());
struct Display;
impl Custody for Display {
    fn take_display(&mut self)->Result<(),String>{EVENTS.lock().unwrap().push("take");Ok(())}
    fn claim_compositor(&mut self)->Result<(),String>{EVENTS.lock().unwrap().push("claim");Ok(())}
    fn release_compositor(&mut self)->Result<(),String>{Ok(())}
    fn give_display(&mut self)->Result<(),String>{Ok(())}
}
#[derive(Debug)] enum Errno { EINTR, ECHILD, EIO }
#[derive(Debug)] enum Error { Os(Errno) }
impl std::fmt::Display for Error {fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{write!(f,"wait error")}}
mod process {
    use super::*;
    pub const WNOHANG:i32=1;
    pub struct Pid(u64); impl Pid {pub fn raw(&self)->u64{self.0}}
    static FIRST:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(true);
    pub fn waitpid(_:i32,status:&mut i32,_:i32)->Result<Pid,Error>{
        if FIRST.swap(false,std::sync::atomic::Ordering::SeqCst){*status=9;Ok(Pid(7))}else{Err(Error::Os(Errno::ECHILD))}
    }
}
struct Output {status:Option<i32>,stdin:Option<()>}
enum Message {Exited{pid:u64,status:i32}}
struct Channel;
impl Channel {fn queue(&mut self,m:&Message)->Result<(),String>{let Message::Exited{pid,status}=m;assert_eq!((*pid,*status),(7,9));EVENTS.lock().unwrap().push("exit");Ok(())}}
BODY
#[test] fn reap_recovers_before_reporting() {
    let mut ownership=Ownership::default();
    ownership.apply(Event::Begin,&mut Display).unwrap();
    ownership.apply(Event::Started,&mut Display).unwrap();
    EVENTS.lock().unwrap().clear();
    let mut channel=Channel;
    let mut outputs=BTreeMap::from([(7,Output{status:None,stdin:Some(())})]);
    CALL
    assert_eq!(*EVENTS.lock().unwrap(),["take","claim","exit"]);
    assert_eq!(outputs[&7].status,Some(9));assert!(outputs[&7].stdin.is_none());
    assert_eq!(ownership.owner(), ownership::Owner::SupervisorHolds);
    ownership.apply(Event::End,&mut Display).unwrap();
    ownership.apply(Event::Reclaim,&mut Display).unwrap();
}
'''.replace('OWNERSHIP', str(ROOT / 'libs/libuitest/src/ownership.rs')).replace('BODY', body).replace('CALL', call)
        # The fixture's errno alphabet also includes the interrupted-wait branch.
        harness = harness.replace('let mut ownership=Ownership::default();', 'let _interrupted = Error::Os(Errno::EINTR); let _io_error = Error::Os(Errno::EIO);\n    let mut ownership=Ownership::default();')
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'check.rs').write_text(harness)
            command = ['rustc', '--edition=2021', '--test', '-Dwarnings', str(path / 'check.rs'), '-o', str(path / 'check')]
            if sys.platform == 'darwin': command = ['taskpolicy', '-b', *command]
            result = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            result = subprocess.run([str(path / 'check')], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_session_frame_does_not_redraw_the_runner_panel(self):
        rel = 'libs/libbreenix/src/suite.rs'
        baseline = os.environ.get('UITEST_OLD_COMMIT')
        source = subprocess.check_output(['git', 'show', f'{baseline}:{rel}'], cwd=ROOT, text=True) if baseline else (ROOT / rel).read_text()
        frame = item(source, 'fn frame(&mut self, feed: &Feed)')
        harness = r'''
use std::sync::Mutex;
static EVENTS:Mutex<Vec<&str>>=Mutex::new(Vec::new());
struct Feed;
struct Screen{gui:bool,session_active:bool,fb:Option<()>,area:Option<()>,live:bool}
mod diagnostics {pub fn draw_live(_: &mut (),_: (),_: &()){super::EVENTS.lock().unwrap().push("live");}}
mod suite_stage {pub fn tick()->Result<(),String>{super::EVENTS.lock().unwrap().push("stage");Ok(())}}
fn flush_panel(_: &mut ()){EVENTS.lock().unwrap().push("flush");}
impl Screen {
    fn stage_result(&mut self,r:Result<(),String>){r.unwrap();}
    fn with_live<R>(_: &Feed,f:impl FnOnce(&())->R)->R{f(&())}
FRAME
}
#[test]fn panel_stays_quiet_during_a_session(){
    for gui in [false,true]{
        let mut screen=Screen{gui,session_active:true,fb:Some(()),area:Some(()),live:true};
        EVENTS.lock().unwrap().clear();screen.frame(&Feed);
        assert!(screen.session_active);
        assert!(EVENTS.lock().unwrap().is_empty(),"runner redrew while session owned the display");
        screen.session_active=false;screen.frame(&Feed);
        assert_eq!(*EVENTS.lock().unwrap(),if gui{vec!["stage"]}else{vec!["live","flush"]});
    }
}
'''.replace('FRAME', frame)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'check.rs').write_text(harness)
            command = ['rustc', '--edition=2021', '--test', '-Dwarnings', str(path / 'check.rs'), '-o', str(path / 'check')]
            if sys.platform == 'darwin': command = ['taskpolicy', '-b', *command]
            result = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            result = subprocess.run([str(path / 'check')], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == '__main__':
    unittest.main()
