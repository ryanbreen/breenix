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
        # Permit running the same behavioral oracle across the additive argument.
        call = 'completed_present_time(7, ready)' if 'not_before' in consumer else 'completed_present_time(7)'
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
   0x616344=>(p%1688)as u32,0x640414=>1066<<16|1688,_=>panic!("register {r:x}")}
 }
 fn delay_us(&mut self,us:u32){NOW.with(|n|n.set(n.get()+u64::from(us)*1000));}
}
struct Evo {head:u32,clock:Option<vblank_clock::VblankClock>,frame_pixels:u64,
 pixel_hz:u64,vblank_line:u16,last_presentation:Option<(u32,u64)>}
SAMPLE
impl Evo { RECORD }
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
  OWNER.with(|e|*e.borrow_mut()=Some(Evo{head:0,clock:Some(vblank_clock::VblankClock::new(0,EPOCH,PIXELS,HZ)),frame_pixels:PIXELS,pixel_hz:HZ,vblank_line:1047,last_presentation:Some((7,EPOCH))}));
  let got=CALL.expect("presentation did not complete");
  assert!(got>=ready,"receipt {got} predates content release {ready}");
  assert!(got.abs_diff(EPOCH+nanos(PIXELS))<1000,"receipt must name the next real edge");
  assert!(got<=signal::monotonic_nanos());
 }
}
#[test]fn a_real_latch_is_preserved_and_foreign_identity_is_refused(){
 let ready=EPOCH-1; assert!(ready<EPOCH);
 OWNER.with(|e|*e.borrow_mut()=Some(Evo{head:0,clock:Some(vblank_clock::VblankClock::new(0,EPOCH,PIXELS,HZ)),frame_pixels:PIXELS,pixel_hz:HZ,vblank_line:1047,last_presentation:Some((7,EPOCH))}));
 NOW.with(|n|n.set(EPOCH+8_000_000));
 assert_eq!(CALL,Some(EPOCH));
 OWNER.with(|e|e.borrow_mut().as_mut().unwrap().last_presentation=Some((8,EPOCH)));
 assert_eq!(CALL,None);
}
'''.replace('CLOCK', clock).replace('SAMPLE', sample).replace('RECORD', record).replace('CONSUMER', consumer).replace('CALL', call)
        # Keep the raster producer available to the repaired consumer and the
        # bodies available for the repaired path without dead-code diagnostics.
        fixture = '#![allow(dead_code)]\n' + fixture
        self.run_fixture(fixture.replace('pub(in crate::gk208)', 'pub(crate)'))


if __name__ == '__main__':
    unittest.main()
