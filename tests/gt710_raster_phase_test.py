"""Run raster changes and receipt retirement on a controlled hardware counter."""
import os
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch
from gt710_runtime_raster_test import fixture_source, ROOT
from gt710_gr_live_unwind_test import item
import gt710_receipt_timing_test as timing


class RasterPhase(unittest.TestCase):
    run_fixture = timing.ReceiptTiming.run_fixture

    def test_phase_follows_monitor_timing(self):
        original = Path.read_text
        revision = os.environ.get('BREENIX_RECEIPT_SOURCE_REV')
        def read(path, *args, **kwargs):
            if revision and str(path).startswith(str(ROOT / 'kernel')):
                return subprocess.check_output(['git','show',f'{revision}:{path.relative_to(ROOT)}'],cwd=ROOT,text=True)
            return original(path,*args,**kwargs)
        with patch.object(Path, 'read_text', read):
            fixture = fixture_source()
            native = read(ROOT / 'kernel/src/gpu/gk208/native.rs')
            clock = read(ROOT / 'kernel/src/gpu/gk208/vblank_clock.rs').replace('//!','//')
        fixture = fixture[:fixture.index('fn main()')]
        fixture = fixture.replace('clock:Option<()>,latch_phase:Option<()>,last_presentation:Option<()>',
            'clock:Option<vblank_clock::VblankClock>,latch_phase:Option<u16>,last_presentation:Option<(u32,u64)>,vblank_line:u16,pending_presentation:Option<PendingPresentation>')
        fixture = fixture.replace('last_presentation:None};','last_presentation:None,vblank_line:0,pending_presentation:None};')
        fixture = fixture.replace('fn read(&self,r:usize)->u32{*self.r.get(&r).unwrap_or(&0)}',r'''fn read(&self,r:usize)->u32{
 let total=*self.r.get(&0x640414).unwrap_or(&0);let w=u64::from(total&0xffff);let h=u64::from(total>>16);
 if matches!(r,0x616340|0x616344){
  let hz=u64::from(*self.r.get(&0x640450).unwrap());
  let p=(u128::from(NOW.with(|n|n.get())-EPOCH)*u128::from(hz)/1_000_000_000)as u64;
  if r==0x616340{return ((p/(w*h))as u32)<<16|((p/w+37)%h)as u32;}
  return (p%w)as u32;
 } *self.r.get(&r).unwrap_or(&0)
}''')
        complete = item(item(native,'fn present_inner('),'fn complete(&mut self)')
        complete = complete.replace('fn complete(', 'fn complete_update(')
        fixture += r'''
const EPOCH:u64=2_000_000_000;
thread_local!{static NOW:std::cell::Cell<u64>=const{std::cell::Cell::new(EPOCH)};}
fn arch_without_interrupts<T>(f:impl FnOnce()->T)->T{f()}
mod signal{pub fn monotonic_nanos()->u64{super::NOW.with(|n|{let t=n.get();n.set(t+100);t})}}
mod window{pub mod pipeline{pub fn at(_:u64,_:u64,_:u64,_:u64,_:u64){}}}
mod presentation_trace{pub fn record<const N:usize>(r:[u64;N],_:u64){assert!(r[7]>=r[1]);}}
mod vblank_clock{ CLOCK }
PENDING
SAMPLE
impl Evo{ METHODS
 fn record_clock_costs(&mut self,_:[[u64;3];2]){}
 fn record_clock_observation(&mut self,_:[u64;4]){}
}
struct Update<'a>{evo:&'a mut Evo,regs:&'a mut Registers}
impl Update<'_>{ COMPLETE }
fn completion(r:&mut Registers,_:u64,_:u64,_:&(),_:&modeset::Pll,_:(u32,u32,bool,bool),_:&[Vec<modeset::Op>;4],_:bool)->Result<u32,&'static str>{
 let total=r.read(0x640414);let pixels=u64::from(total&0xffff)*u64::from(total>>16);
 let p=u128::from(NOW.with(|n|n.get())-EPOCH)*u128::from(r.read(0x640450))/1_000_000_000;
 Ok(u32::from(p>=u128::from(2*pixels)))
}
fn at(e:&Evo,p:u64){NOW.with(|n|n.set(EPOCH+(u128::from(p)*1_000_000_000/u128::from(e.pixel_hz))as u64));}
fn setup()->(Evo,Registers,RasterMode){
 let timing=t(1280,1024,108000);let pll=modeset::Pll{m:1,n:1,p:1,fraction:0};
 let next=RasterMode{timing,pll:pll.clone(),scripts:core::array::from_fn(|_|vec![])};
 let pixels=u64::from(timing.htotal)*u64::from(timing.vtotal);
 let mut r=Registers::default();r.r.insert(0x640414,timing.htotal|timing.vtotal<<16);r.r.insert(0x640450,108000000);
 let e=Evo{rasters:vec![next.clone()],modes:vec![(1280,1024,1)],raster_known:true,
 head:0,sor:0,total:1<<30,push:0,notifier:0,put:0,protocol:1,limits:(),coefficients:pll,scripts:core::array::from_fn(|_|vec![]),
 surfaces:[memory::Range{start:0x10000,size:1280*1024*4};2],frame_pixels:pixels,pixel_hz:108000000,
 clock:Some(vblank_clock::VblankClock::new(0,EPOCH,pixels,108000000)),latch_phase:Some((timing.vtotal-4)as u16),
 vblank_line:37,last_presentation:Some((8,EPOCH)),pending_presentation:None};
 (e,r,next)
}
fn apply(e:&mut Evo,r:&mut Registers,next:&RasterMode){
 let mut attach=modeset::head_methods(&next.timing,0,0,1,0x10000,5120).unwrap();
 attach.extend([(0x84,0x80000000),(0x80,0)]);e.apply_raster(r,next,&attach).unwrap();
}
#[test]fn same_timing_keeps_phase_and_retires_each_next_edge(){
 let (mut e,mut r,next)=setup();let phase=e.latch_phase;
 apply(&mut e,&mut r,&next);
 assert_eq!(e.latch_phase,phase,"raster change discarded timing calibration");
 assert!(e.clock.is_some());assert!(e.last_presentation.is_none());
 for frame in 0..4 {
  let pixels=e.frame_pixels;at(&e,frame*pixels+pixels/4);e.arm_presentation(&r,7).unwrap();
  at(&e,(frame+1)*pixels+2000);
  assert!(e.finish_presentation(&r,7).unwrap(),"same-timing frame waited for +2");
 }
}
#[test]fn timing_change_calibrates_from_a_bracketed_hardware_edge(){
 let (mut e,mut r,mut next)=setup();next.timing=t(1024,768,65000);
 apply(&mut e,&mut r,&next);assert!(e.clock.is_none());assert_eq!(e.latch_phase,None);
 let w=u64::from(next.timing.htotal);let pixels=e.frame_pixels;
 at(&e,pixels-w/2);e.start_clock(&r).unwrap();assert_eq!(e.vblank_line,37);
 at(&e,pixels+pixels/4);e.arm_presentation(&r,7).unwrap();
 // The notifier's negative and positive polls straddle the hardware counter edge.
 at(&e,2*pixels-w/2);
 Update{evo:&mut e,regs:&mut r}.complete_update().unwrap();
 at(&e,2*pixels+100);
 assert!(e.finish_presentation(&r,7).unwrap(),"cold timing retirement waited for +2 despite hardware edge");
 assert!(e.latch_phase.is_some());
}
'''.replace('CLOCK',clock).replace('PENDING',item(native,'#[derive(Clone, Copy)]\nstruct PendingPresentation {'))
        fixture = fixture.replace('SAMPLE',item(native,'fn presentation_sample('))
        methods = '\n'.join(item(native,m) for m in ['fn presentation_attempts(', 'fn start_clock(', 'fn arm_presentation(', 'fn finish_presentation('])
        fixture = fixture.replace('COMPLETE',complete).replace('METHODS',methods).replace('super::vblank_clock','vblank_clock').replace('super::presentation_trace','presentation_trace').replace('pub(in crate::gk208)','pub(crate)')
        self.run_fixture(fixture)


if __name__ == '__main__':
    unittest.main()
