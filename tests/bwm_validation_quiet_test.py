"""A momentarily empty queue must not start readback ahead of the next frame."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT = Path(__file__).resolve().parents[1]
class ValidationQuiet(unittest.TestCase):
 def test_arriving_scene_interrupts_the_quiet_wait(self):
  body = item((ROOT/'userspace/programs/src/bwm/chrome.rs').read_text(), 'fn validation_idle(')
  run_rust(self, r'''
use std::sync::Mutex;
static POLLS: Mutex<Vec<i32>> = Mutex::new(Vec::new());
static ARRIVES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
mod io {
 pub struct PollFd { pub revents:i16 }
 pub fn poll(fds:&mut[PollFd], timeout:i32)->Result<usize,()> {
  super::POLLS.lock().unwrap().push(timeout);
  if timeout != 0 && super::ARRIVES.load(std::sync::atomic::Ordering::Relaxed) { fds[0].revents=1;Ok(1) } else { Ok(0) }
 }
}
@BODY@
#[test]fn idle_gap_is_interruptible(){
 let mut fds=[io::PollFd{revents:0}];
 assert!(!validation_idle(&mut fds),"an animation frame arriving after the empty poll was blocked by readback");
 assert_eq!(*POLLS.lock().unwrap(),[0,20]);
 ARRIVES.store(false,std::sync::atomic::Ordering::Relaxed);POLLS.lock().unwrap().clear();
 assert!(validation_idle(&mut fds),"a quiet scene must still receive its full pixel check");
 assert_eq!(*POLLS.lock().unwrap(),[0,20]);
}
'''.replace('@BODY@',body))
if __name__ == '__main__': unittest.main()
