"""Execute native start_clock against interrupted and stationary scanout samples."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
import gt710_receipt_timing_test as timing


class StartClock(unittest.TestCase):
    run_fixture = timing.ReceiptTiming.run_fixture

    def test_interrupt_at_each_counter_edge_and_stopped_raster(self):
        native = timing.source('kernel/src/gpu/gk208/native.rs')
        clock = timing.source('kernel/src/gpu/gk208/vblank_clock.rs').replace('//!', '//')
        fixture = r'''
use std::cell::Cell;
const EPOCH:u64=2_000_000_000;
const W:u64=1344; const H:u64=806; const HZ:u64=65_000_000;
const FRAME:u64=W*H*1_000_000_000/HZ;
thread_local! {
 static NOW:Cell<u64>=const{Cell::new(EPOCH)};
 static MASKED:Cell<bool>=const{Cell::new(false)};
 static LONGEST:Cell<u64>=const{Cell::new(0)};
 static STATS:Cell<[u64;4]>=const{Cell::new([0;4])};
}
fn arch_without_interrupts<T>(f:impl FnOnce()->T)->T {
 let start=NOW.with(Cell::get); let old=MASKED.with(|m|m.replace(true));
 let result=f(); MASKED.with(|m|m.set(old));
 LONGEST.with(|m|m.set(m.get().max(NOW.with(Cell::get)-start))); result
}
mod signal {pub fn monotonic_nanos()->u64{super::NOW.with(|n|{let t=n.get();n.set(t+100);t})}}
mod vblank_clock { CLOCK }
mod native {
 use super::*;
 pub struct Registers{stopped:bool, delayed_frame:Cell<u64>}
 impl Registers{pub fn read(&self,reg:usize)->u32{
  let mut now=NOW.with(Cell::get);
  let frame=(now-EPOCH)/FRAME;
  // Service an interrupt just as every counter edge becomes observable.
  if !self.stopped && frame>self.delayed_frame.get() && !MASKED.with(Cell::get) {
   self.delayed_frame.set(frame);now+=1_400_000;
  }
  NOW.with(|n|n.set(now+300));
  let pixels=if self.stopped{0}else{(u128::from(now-EPOCH)*u128::from(HZ)/1_000_000_000)as u64};
  match reg {
   0x640414=>(H as u32)<<16|W as u32,
   0x616340=>(((pixels/(W*H))as u16).wrapping_add(65535)as u32)<<16|((pixels/W+37)%H)as u32,
   0x616344=>(pixels%W)as u32,
   _=>panic!("register {reg:x}")
  }
 }}
 struct Evo{head:u32,clock:Option<vblank_clock::VblankClock>,frame_pixels:u64,pixel_hz:u64,vblank_line:u16}
 SAMPLE
 impl Evo {
  METHODS
  fn record_clock_costs(&mut self,_:[[u64;3];2]){}
  fn record_clock_observation(&mut self,counts:[u64;4]){STATS.with(|s|s.set(counts));}
 }
 pub fn exercise(stopped:bool){
  NOW.with(|n|n.set(EPOCH+FRAME-100_000));MASKED.with(|m|m.set(false));LONGEST.with(|n|n.set(0));
  let regs=Registers{stopped,delayed_frame:Cell::new(0)};
  let mut evo=Evo{head:0,clock:None,frame_pixels:W*H,pixel_hz:HZ,vblank_line:0};
  arch_without_interrupts(||());evo.record_clock_observation([0;4]);
  let start=NOW.with(Cell::get);let result=evo.start_clock(&regs);
  if stopped {
   assert_eq!(result,Err("native vblank raster stopped"));assert!(evo.clock.is_none());
   assert_eq!(STATS.with(Cell::get)[1],0);
  } else {
   assert_eq!(result,Ok(()),"interrupts split every unprotected edge");
   assert_eq!(evo.vblank_line,37,"calibrated a delayed later line");assert!(evo.clock.is_some());
   let clock=evo.clock.as_mut().unwrap();assert!(clock.at(clock.last)>=start);
   assert!(STATS.with(Cell::get)[1]>0);
  }
  assert!(NOW.with(Cell::get)-start<=51_000_000+3*W*1_000_000_000/HZ);
  assert!(LONGEST.with(Cell::get)<=3*W*1_000_000_000/HZ,"masked beyond three lines");
 }
}
#[test]fn interrupted_edges_calibrate(){native::exercise(false);}
#[test]fn stationary_raster_refuses(){native::exercise(true);}
'''
        fixture = fixture.replace('CLOCK', clock).replace('SAMPLE', item(native, 'fn presentation_sample('))
        fixture = fixture.replace('METHODS', '\n'.join(item(native, m) for m in ['fn presentation_attempts(', 'fn start_clock(']))
        self.run_fixture(fixture)


if __name__ == '__main__':
    unittest.main()
