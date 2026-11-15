"""Calibrate the production clock with expensive uncached raster reads."""
import unittest
from gt710_gr_live_unwind_test import item
import gt710_receipt_timing_test as timing


class SlowEdge(unittest.TestCase):
    run_fixture = timing.ReceiptTiming.run_fixture

    def test_slow_full_samples_keep_the_original_edge(self):
        native = timing.source('kernel/src/gpu/gk208/native.rs')
        clock = timing.source('kernel/src/gpu/gk208/vblank_clock.rs').replace('//!', '//')
        fixture = r'''
use std::cell::Cell;
const EPOCH:u64=2_000_000_000;
const W:u64=1344;const H:u64=806;const HZ:u64=65_000_000;
const FRAME:u64=W*H*1_000_000_000/HZ;
thread_local!{static NOW:Cell<u64>=const{Cell::new(EPOCH)};
 static COST:Cell<[[u64;3];2]>=const{Cell::new([[0;3];2])};}
fn arch_without_interrupts<T>(f:impl FnOnce()->T)->T{f()}
mod signal{pub fn monotonic_nanos()->u64{super::NOW.with(|n|{let t=n.get();n.set(t+1_000);t})}}
mod vblank_clock{CLOCK}
mod native{
 use super::*;
 struct Registers;
 impl Registers{fn read(&self,r:usize)->u32{
  let now=NOW.with(|n|{let t=n.get();n.set(t+6_000);t+3_000});
  let pixels=(u128::from(now-EPOCH)*u128::from(HZ)/1_000_000_000)as u64;
  match r{0x640414=>(H as u32)<<16|W as u32,
   0x616340=>(((pixels/(W*H))as u16).wrapping_add(65535)as u32)<<16|((pixels/W+37)%H)as u32,
   0x616344=>(pixels%W)as u32,_=>panic!("{r:x}")}
 }}
 struct Evo{head:u32,clock:Option<vblank_clock::VblankClock>,frame_pixels:u64,pixel_hz:u64,vblank_line:u16}
 SAMPLE
 impl Evo{METHODS
  fn record_clock_observation(&mut self,_:[u64;4]){}
  fn record_clock_costs(&mut self,c:[[u64;3];2]){COST.with(|v|v.set(c));}
 }
 pub fn exercise(){
  NOW.with(|v|v.set(EPOCH+FRAME-100_000));
  let mut evo=Evo{head:0,clock:None,frame_pixels:W*H,pixel_hz:HZ,vblank_line:0};
  evo.record_clock_costs([[0;3];2]);
  let started=NOW.with(Cell::get);
  assert_eq!(evo.start_clock(&Registers),Ok(()),"full-sample cadence misses every edge");
  assert_eq!(evo.vblank_line,37,"a later line was substituted for the edge");
  let clock=evo.clock.as_mut().unwrap();
  let edge=clock.at(clock.last);
  let expected=EPOCH+(edge-EPOCH+FRAME/2)/FRAME*FRAME;
  assert!(edge.abs_diff(expected)<1_000,"wrong edge_ns {edge} vs {expected}");
  assert!(NOW.with(Cell::get)-started<51_000_000);
  let [vert,full]=COST.with(Cell::get);
  assert!(vert[0]>0 && vert[0]<=vert[1] && vert[1]<=vert[2]);
  assert!(full[0]>=20_000 && full[0]<=full[1] && full[1]<=full[2]);
  assert!(vert[0]<W*1_000_000_000/HZ);
 }
}
#[test]fn expensive_raster_reads_calibrate(){native::exercise();}
'''
        fixture = fixture.replace('CLOCK', clock).replace('SAMPLE', item(native, 'fn presentation_sample('))
        fixture = fixture.replace('METHODS', '\n'.join(item(native, m) for m in ['fn presentation_attempts(', 'fn start_clock(']))
        self.run_fixture(fixture)


if __name__ == '__main__':
    unittest.main()
