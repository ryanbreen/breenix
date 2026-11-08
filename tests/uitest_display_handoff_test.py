"""A session nests above its runner without an intervening console repaint."""
from pathlib import Path
import os
import subprocess
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT = Path(__file__).resolve().parents[1]
class DisplayHandoff(unittest.TestCase):
    def test_runner_frame_stays_below_supervisor(self):
        path = 'libs/libbreenix/src/suite.rs'
        old = os.environ.get('UITEST_OLD_COMMIT')
        text = subprocess.check_output(['git','show',f'{old}:{path}'],cwd=ROOT,text=True) if old else (ROOT/path).read_text()
        release = item(text, 'fn release_for_session(')
        reclaim = item(text, 'fn reclaim_after_session(')
        run_rust(self, r'''
use std::sync::Mutex;
static STACK:Mutex<Vec<u64>>=Mutex::new(Vec::new());
static RESTORES:Mutex<u32>=Mutex::new(0);
#[path="OWNERSHIP"]mod ownership;
mod graphics {
 pub fn give_back_display()->Result<(),String>{let mut s=super::STACK.lock().unwrap();s.pop().ok_or("empty stack")?;if s.is_empty(){*super::RESTORES.lock().unwrap()+=1;}Ok(())}
 pub fn take_over_display()->Result<(),String>{let mut s=super::STACK.lock().unwrap();if s.last()!=Some(&1){s.push(1)}Ok(())}
}
struct Screen{gui:bool,fb:Option<()>,gpu:Option<()>,session_active:bool}
impl Screen {RELEASE RECLAIM}
struct Display(u64);
impl ownership::Custody for Display {
 fn take_display(&mut self)->Result<(),String>{let mut s=STACK.lock().unwrap();if s.last()!=Some(&self.0){s.push(self.0)}Ok(())}
 fn claim_compositor(&mut self)->Result<(),String>{Ok(())}
 fn release_compositor(&mut self)->Result<(),String>{Ok(())}
 fn give_display(&mut self)->Result<(),String>{graphics::give_back_display()}
}
#[test]fn preserve_nested_custody_and_avoid_full_console_work(){
 use ownership::{Ownership,Event,Owner};
 for (gui,fb,gpu) in [(false,Some(()),None),(true,None,None),(false,None,Some(()))] {
  *STACK.lock().unwrap()=vec![1];*RESTORES.lock().unwrap()=0;
  let mut screen=Screen{gui,fb,gpu,session_active:false};
  let _=(screen.gui,screen.fb,screen.gpu);
  screen.release_for_session().unwrap();assert!(screen.session_active);
  assert_eq!(*STACK.lock().unwrap(),[1],"session start exposed the console instead of retaining the runner below it");
  let mut owner=Ownership::default();owner.apply(Event::Begin,&mut Display(2)).unwrap();
  assert_eq!(owner.owner(),Owner::SupervisorReady);
  owner.apply(Event::Started,&mut Display(2)).unwrap();STACK.lock().unwrap().push(3);
  assert_eq!(*STACK.lock().unwrap(),[1,2,3]);
  STACK.lock().unwrap().pop();owner.apply(Event::Died,&mut Display(2)).unwrap();
  owner.apply(Event::Recover,&mut Display(2)).unwrap();assert_eq!(owner.owner(),Owner::SupervisorHolds);
  owner.apply(Event::End,&mut Display(2)).unwrap();assert_eq!(owner.owner(),Owner::CaseEnd);
  screen.reclaim_after_session().unwrap();assert!(!screen.session_active);
  owner.apply(Event::Reclaim,&mut Display(1)).unwrap();assert_eq!(owner.owner(),Owner::RunnerHolds);
  // Both reclaim calls are idempotent for the retained runner.
  assert_eq!(*STACK.lock().unwrap(),[1]);
  assert_eq!(*RESTORES.lock().unwrap(),0,"a full console repaint was scheduled between graphical owners");
 }
}
'''.replace('OWNERSHIP',str(ROOT/'libs/libuitest/src/ownership.rs')).replace('RELEASE',release).replace('RECLAIM',reclaim))
if __name__ == '__main__': unittest.main()
