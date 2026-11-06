"""Tie native flip receipt times to submission rather than reaper scheduling."""
import unittest
from gt710_gr_live_unwind_test import item
import gt710_receipt_timing_test as timing


class SubmittedReceipt(unittest.TestCase):
    run_fixture = timing.ReceiptTiming.run_fixture

    def test_early_completion_and_late_reaps_name_the_submission_edge(self):
        native = timing.source('kernel/src/gpu/gk208/native.rs')
        clock = timing.source('kernel/src/gpu/gk208/vblank_clock.rs').replace('//!', '//')
        sample = item(native, 'fn presentation_sample(')
        schedule = item(native, 'pub(in crate::gk208) fn presentation_schedule(')
        record = item(native, 'pub(in crate::gk208) fn record_presentation(')
        pending = item(native, '#[derive(Clone, Copy)]\nstruct PendingPresentation {') if 'struct PendingPresentation {' in native else ''
        arm = item(native, 'fn arm_presentation(') if 'fn arm_presentation(' in native else '''
        fn arm_presentation(&mut self, _: &Registers, _: u32)->Result<(),&'static str>{Ok(())}
        '''
        finish = item(native, 'fn finish_presentation(') if 'fn finish_presentation(' in native else '''
        fn finish_presentation(&mut self, regs:&Registers, resource:u32)->Result<bool,&'static str>{
            self.record_presentation(regs,resource)?;Ok(true)
        }
        '''
        content = item(native, 'pub(in crate::gk208) fn content_presentation(')
        rate = timing.source('userspace/programs/src/wm_suite/rate.rs')
        fixture = r'''
#![allow(dead_code)] // Extracted native APIs include paths outside this fixture.
use std::cell::Cell;
const EPOCH:u64=2_000_000_000;
const PIXELS:u64=1688*1066;
const HZ:u64=108_000_000;
fn nanos(p:u64)->u64{(u128::from(p)*1_000_000_000/u128::from(HZ))as u64}
thread_local!{static NOW:Cell<u64>=const{Cell::new(EPOCH)};}
mod signal{pub fn monotonic_nanos()->u64{super::NOW.with(Cell::get)}use std::cell::Cell;}
mod vblank_clock{ CLOCK }
mod rate{ RATE }
mod presentation_trace {pub fn record(row:[u64;15]){assert!(row[7]>=row[1]);assert_ne!(row[2]>>16,row[9]>>16);}}
mod native{
 use super::*;
 pub struct Registers{pub base:u16}
 impl Registers{pub fn read(&self,r:usize)->u32{
  let p=(u128::from(signal::monotonic_nanos()-EPOCH)*u128::from(HZ)/1_000_000_000)as u64;
  match r{0x616340=>u32::from(self.base.wrapping_add((p/PIXELS)as u16))<<16|((1047+p/1688)%1066)as u32,
   0x616344=>(p%1688)as u32,0x640414=>1066<<16|1688,0x64041c=>41<<16|359,0x640420=>1065<<16|1639,_=>panic!("reg {r:x}")}
 }}
 PENDING
 pub struct Evo{head:u32,clock:Option<vblank_clock::VblankClock>,frame_pixels:u64,pixel_hz:u64,
  vblank_line:u16,latch_phase:Option<u16>,last_presentation:Option<(u32,u64)>,PENDING_FIELD}
 SAMPLE
 impl Evo{ SCHEDULE RECORD ARM FINISH CONTENT }
 fn setup(base:u16)->Evo{Evo{head:0,clock:Some(vblank_clock::VblankClock::new(base,EPOCH,PIXELS,HZ)),frame_pixels:PIXELS,pixel_hz:HZ,vblank_line:1047,latch_phase:Some(1065),last_presentation:Some((9,EPOCH)),PENDING_INIT}}
 pub fn late(){
  let regs=Registers{base:65535};let mut e=setup(65535);
  NOW.with(|n|n.set(EPOCH+1_000_000));e.arm_presentation(&regs,7).unwrap();
  NOW.with(|n|n.set(EPOCH+nanos(PIXELS)+17_000_000));
  assert!(e.finish_presentation(&regs,7).unwrap());
  assert!(e.last_presentation.unwrap().1.abs_diff(EPOCH+nanos(PIXELS))<1000,"late reap relabelled the accepted frame");
 }
 pub fn retained_content(){
  let regs=Registers{base:0};let mut e=setup(0);
  let accepted=EPOCH+1_000_000;NOW.with(|n|n.set(accepted));
  e.arm_presentation(&regs,7).unwrap();
  e.pending_presentation.as_mut().unwrap().acknowledged=Some(1);
  assert_eq!(e.content_presentation(&regs,7,accepted).unwrap(),None,
   "fresh ordinary acknowledgement published content before its edge");
  NOW.with(|n|n.set(EPOCH+nanos(PIXELS)+100_000));
  let at=e.content_presentation(&regs,7,accepted).unwrap().unwrap();
  assert!(at>=accepted);assert!(at.abs_diff(EPOCH+nanos(PIXELS))<1000);
  assert!(e.content_presentation(&regs,9,accepted).is_err());
  assert!(e.content_presentation(&regs,7,at+1).is_err());
 }
 pub fn retained_replacement(){
  let regs=Registers{base:0};let mut e=setup(0);
  NOW.with(|n|n.set(EPOCH+1_000_000));e.arm_presentation(&regs,7).unwrap();
  assert!(e.arm_presentation(&regs,8).is_err(),"an outstanding real flip was replaced");
  e.pending_presentation.as_mut().unwrap().acknowledged=Some(1);
  // Ordinary retained acceptance already completed; it must not pin a
  // historical content timestamp across a subsequent refresh or binding.
  for resource in [7,8] {
   NOW.with(|n|n.set(EPOCH+2_000_000));e.arm_presentation(&regs,resource).unwrap();
   let p=e.pending_presentation.as_ref().unwrap();assert_eq!(p.resource,resource);
   assert_eq!(p.accepted_ns,EPOCH+2_000_000);
   assert_eq!(e.content_presentation(&regs,resource,EPOCH+2_000_000).unwrap(),None);
   e.pending_presentation.as_mut().unwrap().acknowledged=Some(1);
  }
  NOW.with(|n|n.set(EPOCH+nanos(PIXELS)+100_000));
  assert!(e.content_presentation(&regs,8,EPOCH+2_000_000).unwrap().unwrap()>=EPOCH+2_000_000);
 }
 pub fn stream(){
  let regs=Registers{base:65535};let mut e=setup(65535);let mut times=Vec::new();
  for frame in 0..310{
   NOW.with(|n|n.set(EPOCH+nanos(frame*PIXELS)+1_000_000));e.arm_presentation(&regs,7).unwrap();
   let delay=if frame==309{17_000_000}else{100_000};
   NOW.with(|n|n.set(EPOCH+nanos((frame+1)*PIXELS)+delay));
   assert!(e.finish_presentation(&regs,7).unwrap());times.push(e.last_presentation.unwrap().1);
  }
  assert!(rate::measure(&times).unwrap().passes,"reaper scheduling changed the W2 frame floor");
  times.remove(150);assert!(!rate::measure(&times).unwrap().passes,"a missing presentation was hidden");
 }
 // Existing early-acceptance fixtures use a synthetic deadline at phase1065.
 // W20's R240-mode fixture has counter-relative deadline952 and includes its band.
 pub fn schedule(){
  let regs=Registers{base:0};let mut e=setup(0);e.latch_phase=Some(952);
  let period=((u128::from(PIXELS)*1_000_000_000+u128::from(HZ)-1)/u128::from(HZ))as u64;
  let lead=((114u128*1688*1_000_000_000+u128::from(HZ)-1)/u128::from(HZ))as u64;
  assert_eq!(e.presentation_schedule(&regs).unwrap(),(period,lead));
  e.latch_phase=None;assert_eq!(e.presentation_schedule(&regs).unwrap(),(period,period));
  e.frame_pixels-=1;assert!(e.presentation_schedule(&regs).is_err());
 }
 pub fn calibrate(){
  let regs=Registers{base:0};let mut e=setup(0);e.latch_phase=None;
  NOW.with(|n|n.set(EPOCH+1_000_000));e.arm_presentation(&regs,7).unwrap();
  NOW.with(|n|n.set(EPOCH+nanos(950*1688)+1000));
  e.pending_presentation.as_mut().unwrap().last_negative=Some(presentation_sample(&regs,0).unwrap());
  NOW.with(|n|n.set(EPOCH+nanos(952*1688)+1000));
  assert!(!e.finish_presentation(&regs,7).unwrap());
  assert_eq!(e.latch_phase,Some(950),"deadline must use the conservative negative side of the measured transition");
  NOW.with(|n|n.set(EPOCH+nanos(PIXELS)+100_000));assert!(e.finish_presentation(&regs,7).unwrap());
 }
 pub fn late_accept(){
  for line in [952,953,954,955,956,960,1050]{for delay in [100_000,17_000_000]{
   let regs=Registers{base:65535};let mut e=setup(65535);e.latch_phase=Some(952);
   NOW.with(|n|n.set(EPOCH+nanos(line*1688)+2000));e.arm_presentation(&regs,7).unwrap();
   if delay==100_000 {
    NOW.with(|n|n.set(EPOCH+nanos(PIXELS)+100_000));
    assert!(!e.finish_presentation(&regs,7).unwrap(),"late acceptance was published at +1");
   }
   NOW.with(|n|n.set(EPOCH+nanos(2*PIXELS)+delay));
   assert!(e.finish_presentation(&regs,7).unwrap());
   assert!(e.last_presentation.unwrap().1.abs_diff(EPOCH+nanos(2*PIXELS))<1000,"late/band acceptance did not keep +2 under delayed first poll");
  }}
 }
 pub fn check(){
  for base in [0,65535]{for phase in [1_000_000,8_000_000,16_000_000]{
   for delay in [100_000,600_000,5_000_000,17_000_000]{
    let regs=Registers{base};
    let mut e=Evo{head:0,clock:Some(vblank_clock::VblankClock::new(base,EPOCH,PIXELS,HZ)),
     frame_pixels:PIXELS,pixel_hz:HZ,vblank_line:1047,latch_phase:Some(1065),last_presentation:Some((9,EPOCH)),PENDING_INIT};
    let accepted=EPOCH+phase;NOW.with(|n|n.set(accepted));
    e.arm_presentation(&regs,7).unwrap();
    // A notifier visible at acceptance is not a completed presentation.
    assert!(!e.finish_presentation(&regs,7).unwrap(),"accepted commands completed before their next edge");
    let expected=EPOCH+nanos(PIXELS);
    NOW.with(|n|n.set(expected+delay));
    assert!(e.finish_presentation(&regs,7).unwrap());
    let got=e.last_presentation.unwrap();assert_eq!(got.0,7);
    assert!(got.1>=accepted);
    assert!(got.1.abs_diff(expected)<1000,"reap delay {delay} changed latch: {:?} expected {expected}",got);
   }
  }}
 }
}
#[test]fn accepted_retained_content_can_be_superseded(){native::retained_replacement();}
#[test]fn schedule_uses_programmed_raster_and_calibrated_lead(){native::schedule();}
#[test]fn retained_content_waits_after_ordinary_acknowledgement(){native::retained_content();}
#[test]fn deadline_comes_from_a_bracketed_mode_transition(){native::calibrate();}
#[test]fn late_acceptances_and_boundary_band_use_the_second_edge(){native::late_accept();}
#[test]fn submitted_frame_is_independent_of_completion_poll_delay(){native::check();}
#[test]fn a_late_reap_keeps_the_accepted_frame(){native::late();}
#[test]fn reaper_schedule_does_not_change_the_presentation_rate(){native::stream();}
'''.replace('CLOCK', clock).replace('RATE', rate).replace('PENDING_FIELD', 'pending_presentation:Option<PendingPresentation>,' if pending else '').replace('PENDING_INIT', 'pending_presentation:None,' if pending else '').replace('PENDING', pending).replace('SAMPLE', sample).replace('SCHEDULE',schedule).replace('RECORD', record).replace('ARM', arm).replace('FINISH', finish).replace('CONTENT', content)
        self.run_fixture(fixture.replace('pub(in crate::gk208)', 'pub(crate)'))


if __name__ == '__main__':
    unittest.main()
