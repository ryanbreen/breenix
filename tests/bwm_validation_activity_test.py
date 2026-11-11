"""Typing must leave idle validation pending until an opportunity fits its cost."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class ValidationActivity(unittest.TestCase):
 def test_static_scene_admission_does_not_reschedule_validation(self):
  import re
  helper=item((ROOT/'userspace/programs/src/bwm/chrome.rs').read_text(),'pub(super) fn defer_checks_after_activity(')
  call=re.search(r'defer_checks_after_activity\(&mut windows[^;]+;', (ROOT/'userspace/programs/src/bwm.rs').read_text())[0]
  run_rust(self,r'''
struct Window{frame_checked:bool,check_after_ms:u64}
fn monotonic_ms()->Option<u64>{Some(100)}
struct Gpu;impl Gpu{fn validation_delay_ms(&self)->u64{700}}
mod production{use super::*;@HELPER@
 pub fn turn(mut windows:&mut[Window],input_ready:bool,scene_changed:bool){let gpu=Gpu;@CALL@std::hint::black_box(scene_changed);}
}
#[test]fn a_static_client_keeps_its_first_frame_opportunity(){
 let mut w=[Window{frame_checked:false,check_after_ms:20}];
 production::turn(&mut w,false,true);
 assert_eq!(w[0].check_after_ms,20,"scene-only admission deferred first-frame validation by the prior capture cost");
 production::turn(&mut w,true,false);assert_eq!(w[0].check_after_ms,800,"input still needs its observed-cost quiet interval");
}
'''.replace('@HELPER@',helper).replace('@CALL@',call))
 def test_continued_input_postpones_pending_checks_without_losing_them(self):
  source=(ROOT/'userspace/programs/src/bwm/chrome.rs').read_text()
  body=item(source,'pub(super) fn defer_checks_after_activity(') if 'fn defer_checks_after_activity(' in source else 'pub(super) fn defer_checks_after_activity(_: &mut [Window], _:u64, _:u64, _:bool) {}'
  run_rust(self,r'''
struct Window{frame_checked:bool,check_after_ms:u64}
mod production{use super::*;@BODY@pub fn defer(w:&mut[Window],now:u64,cost:u64,active:bool){defer_checks_after_activity(w,now,cost,active)}}
#[test]fn an_observed140ms_check_does_not_restart_between50ms_inputs(){
 let mut windows=[Window{frame_checked:false,check_after_ms:0},Window{frame_checked:false,check_after_ms:999},Window{frame_checked:true,check_after_ms:7}];
 assert!(!windows[0].frame_checked && windows[2].frame_checked);
 for now in (0..=250).step_by(50){production::defer(&mut windows,now,140,true);assert!(windows[0].check_after_ms>now+50,"validation restarted between consecutive input samples");}
 assert_eq!(windows[0].check_after_ms,390);assert_eq!(windows[1].check_after_ms,999);assert_eq!(windows[2].check_after_ms,7);
 production::defer(&mut windows,389,140,false);assert_eq!(windows[0].check_after_ms,390,"quiet time must not move the deadline again");
 assert!(390>=windows[0].check_after_ms,"pending work must become eligible after input stops");
}
'''.replace('@BODY@',body))
  self.assertIn('defer_checks_after_activity(&mut windows',(ROOT/'userspace/programs/src/bwm.rs').read_text())
if __name__=='__main__':unittest.main()
