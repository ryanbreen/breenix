"""Execute the native composite-time consumer after a mid-frame content release."""
from gt710_gr_live_unwind_test import item
import gt710_receipt_timing_test as timing
source = timing.source
import unittest


class ContentReceipt(unittest.TestCase):
    run_fixture = timing.ReceiptTiming.run_fixture

    def test_retained_content_waits_for_its_own_presentation_edge(self):
        native = source('kernel/src/gpu/gk208/native.rs')
        provider = source('kernel/src/gpu/gk208/mod.rs')
        consumer = item(provider, 'pub(crate) fn completed_present_time(')
        record = item(native, 'pub(in crate::gk208) fn record_presentation(').replace('super::vblank_clock', 'crate::vblank_clock')
        sample = item(native, 'fn presentation_sample(')
        clock = source('kernel/src/gpu/gk208/vblank_clock.rs').replace('//!', '//')
        pending = item(native, '#[derive(Clone, Copy)]\nstruct PendingPresentation {')
        methods = '\n'.join(item(native, name) for name in ['fn arm_presentation(', 'pub(in crate::gk208) fn presentation_probe(', 'fn finish_presentation(', 'pub(in crate::gk208) fn content_presentation(']).replace('super::presentation_trace', 'crate::presentation_trace')
        call = 'completed_present_time(7, ready)'
        fixture = r'''
use std::cell::{Cell, RefCell};
const EPOCH:u64=2_000_000_000;
const PIXELS:u64=1688*1066;
const HZ:u64=108_000_000;
fn nanos(p:u64)->u64{(u128::from(p)*1_000_000_000/u128::from(HZ))as u64}
thread_local!{static NOW:Cell<u64>=const{Cell::new(EPOCH)};
 static OWNER:RefCell<Option<Evo>>=const{RefCell::new(None)};}
mod signal {pub fn monotonic_nanos()->u64{super::NOW.with(|n|n.get())}}
mod modeset {
 pub trait Io {fn delay_us(&mut self,us:u32);}
 impl Io for super::Registers {fn delay_us(&mut self,us:u32){self.delay_us(us);}}
}
mod vblank_clock { CLOCK }
struct Registers;
impl Registers {
 fn read(&self,r:usize)->u32{
  let p=(u128::from(signal::monotonic_nanos()-EPOCH)*u128::from(HZ)/1_000_000_000)as u64;
  match r {0x616340=>((p/PIXELS)as u32)<<16|((1047+p/1688)%1066)as u32,
   0x616344=>(p%1688)as u32,0x640414=>1066<<16|1688,0x64041c=>41<<16|359,0x640420=>1065<<16|1639,_=>panic!("register {r:x}")}
 }
 fn delay_us(&mut self,us:u32){NOW.with(|n|n.set(n.get()+u64::from(us)*1000));}
}
PENDING
mod window {pub mod pipeline {pub fn at(_:u64,_:u64,_:u64,_:u64,_:u64){}}}
mod presentation_trace {pub fn record(row:[u64;15]){assert!(row[7]>=row[1]);}}
struct Evo {head:u32,clock:Option<vblank_clock::VblankClock>,frame_pixels:u64,
 pixel_hz:u64,vblank_line:u16,latch_phase:Option<u16>,pending_presentation:Option<PendingPresentation>,last_presentation:Option<(u32,u64)>}
SAMPLE
impl Evo { RECORD METHODS }
mod ce {
 pub fn with_clock_owner<R>(f:impl FnOnce(&mut super::Registers,&mut super::Evo,())->Result<R,&'static str>)->Result<R,&'static str>{
  super::OWNER.with(|e|f(&mut super::Registers,e.borrow_mut().as_mut().unwrap(),()))
 }
}
CONSUMER
#[test]fn mid_frame_ack_cannot_complete_new_content_at_the_old_edge(){
 for phase in [1_000_000,8_000_000,16_000_000] {
  let ready=EPOCH+phase;
  NOW.with(|n|n.set(ready));
  OWNER.with(|e|*e.borrow_mut()=Some(Evo{head:0,clock:Some(vblank_clock::VblankClock::new(0,EPOCH,PIXELS,HZ)),frame_pixels:PIXELS,pixel_hz:HZ,vblank_line:1047,latch_phase:Some(1065),pending_presentation:None,last_presentation:Some((7,EPOCH))}));
  OWNER.with(|e|{let mut owner=e.borrow_mut();let e=owner.as_mut().unwrap();
   e.arm_presentation(&Registers,7).unwrap();e.pending_presentation.as_mut().unwrap().acknowledged=Some(1);});
  // W22 probes once; sleeping between probes belongs to the shared caller.
  assert_eq!(CALL.unwrap(),None,"content completed before the next edge");
  NOW.with(|n|n.set(EPOCH+nanos(PIXELS)+100_000));
  let got=CALL.expect("presentation query failed").expect("presentation did not complete");
  assert!(got>=ready,"receipt {got} predates content release {ready}");
  assert!(got.abs_diff(EPOCH+nanos(PIXELS))<1000,"receipt must name the next real edge");
  assert!(got<=signal::monotonic_nanos());
 }
}
#[test]fn predicted_probe_tracks_the_selected_physical_edge(){
 for line in [100u64,700,949,950,955] {
  let accepted=EPOCH+nanos(line*1688)+2000;NOW.with(|n|n.set(accepted));
  let mut e=Evo{head:0,clock:None,frame_pixels:PIXELS,pixel_hz:HZ,vblank_line:1047,latch_phase:Some(950),pending_presentation:None,last_presentation:None};
  e.arm_presentation(&Registers,7).unwrap();
  let edge=EPOCH+nanos(PIXELS*if line<950{1}else{2});
  assert!(e.presentation_probe(false).unwrap().abs_diff(edge+30_000)<1000);
  e.pending_presentation.as_mut().unwrap().acknowledged=Some(1);
  assert_eq!(e.presentation_probe(false),None,"worker must not poll an ordinary retained ACK forever");
  assert!(e.presentation_probe(true).is_some());
 }
}
#[test]fn a_real_latch_is_preserved_and_foreign_identity_is_refused(){
 let ready=EPOCH-1; assert!(ready<EPOCH);
 OWNER.with(|e|*e.borrow_mut()=Some(Evo{head:0,clock:Some(vblank_clock::VblankClock::new(0,EPOCH,PIXELS,HZ)),frame_pixels:PIXELS,pixel_hz:HZ,vblank_line:1047,latch_phase:Some(1065),pending_presentation:None,last_presentation:Some((7,EPOCH))}));
 NOW.with(|n|n.set(EPOCH+8_000_000));
 assert_eq!(CALL.unwrap(),Some(EPOCH));
 OWNER.with(|e|e.borrow_mut().as_mut().unwrap().last_presentation=Some((8,EPOCH)));
 assert!(CALL.is_err(),"foreign identity was accepted");
}
'''.replace('CLOCK', clock).replace('SAMPLE', sample).replace('RECORD', record).replace('PENDING', pending).replace('METHODS', methods).replace('CONSUMER', consumer).replace('CALL', call)
        # Keep the raster producer available to the repaired consumer and the
        # bodies available for the repaired path without dead-code diagnostics.
        fixture = '#![allow(dead_code)]\n' + fixture
        self.run_fixture(fixture.replace('pub(in crate::gk208)', 'pub(crate)'))


if __name__ == '__main__':
    unittest.main()
