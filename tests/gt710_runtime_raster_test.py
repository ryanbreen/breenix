"""Execute runtime raster selection against bounded, faultable EVO registers."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from gt710_gr_live_unwind_test import item

class RuntimeRaster(unittest.TestCase):
    def test_larger_and_equal_extent_modes(self):
        source=(ROOT/'kernel/src/gpu/gk208/native.rs').read_text()
        select=item(source,'    fn select_raster(').replace('super::present::','present::')
        extra=''
        if '    fn apply_raster(' in source:
            extra=item(source,'    fn apply_raster(').replace('super::present::','present::')
        call='e.select_raster(r,s,index).map(|_|())' if 'surface: ' in select else 'e.select_raster(r,s.width,s.height)'
        code=r'''#![allow(dead_code)]
extern crate alloc;
use std::collections::BTreeMap;
#[path="ROOT/kernel/src/gpu/gk208/edid.rs"] mod edid;
#[path="ROOT/kernel/src/gpu/gk208/modeset.rs"] mod modeset;
#[path="ROOT/kernel/src/gpu/gk208/present.rs"] mod present;
#[path="ROOT/kernel/src/gpu/gk208/memory.rs"] mod memory;
#[derive(Default)] struct Registers{r:BTreeMap<usize,u32>,detaches:usize,fail:usize,calls:usize}
impl Registers{fn read(&self,r:usize)->u32{*self.r.get(&r).unwrap_or(&0)}}
impl modeset::Io for Registers{fn read(&mut self,r:usize)->u32{Registers::read(self,r)}
 fn write(&mut self,r:usize,v:u32){self.r.insert(r,v);}fn delay_us(&mut self,_:u32){}}
#[derive(Clone)]struct RasterMode{timing:edid::Timing,pll:modeset::Pll,scripts:[Vec<modeset::Op>;4]}
struct Evo{rasters:Vec<RasterMode>,modes:Vec<(u32,u32,u32)>,raster_known:bool,
 head:u32,sor:u32,total:u64,push:u64,notifier:u64,put:u32,protocol:u32,
 limits:(),coefficients:modeset::Pll,scripts:[Vec<modeset::Op>;4],surfaces:[memory::Range;2],
 frame_pixels:u64,pixel_hz:u64,clock:Option<()>,latch_phase:Option<()>,last_presentation:Option<()>}
impl Evo{SELECT
 EXTRA
 fn submit_mode(&mut self,r:&mut Registers,m:&[(u32,u32)])->Result<(), &'static str>{
  for &(a,v) in m {r.r.insert(0x640000+a as usize,v);r.r.insert(0x660000+a as usize,v);}
  if m.contains(&(0x200,0)){r.detaches+=1;}Ok(())}
}
fn write_words(_: &mut Registers,_:u64,_:u64,_:&[u32])->Result<(), &'static str>{Ok(())}
fn complete(r: &mut Registers,_:u64,_:u64,_:&(),_:&modeset::Pll,_:(u32,u32,bool,bool),_:&[Vec<modeset::Op>;4])->Result<u32,&'static str>{r.calls+=1;if r.calls==r.fail {Err("injected completion timeout")} else {Ok(1)}}
fn t(w:u32,h:u32,clock:u32)->edid::Timing{edid::Timing{width:w,height:h,clock_khz:clock,
 hsync_start:w+24,hsync_end:w+104,htotal:w+160,vsync_start:h+3,vsync_end:h+9,vtotal:h+40,flags:24}}
fn run(w:u32,h:u32,fault:usize){let p=t(w,h,65000);let big=t(1920,1080,148500);
 let pll=modeset::Pll{m:1,n:1,p:1,fraction:0};let recipe=|timing|RasterMode{timing,pll:pll.clone(),scripts:core::array::from_fn(|_|Vec::new())};
 let mut e=Evo{rasters:vec![recipe(p),recipe(big)],modes:vec![(1920,1080,1),(w,h,1),(w*2,h*2,2)],raster_known:true,
 head:0,sor:0,total:1<<30,push:0,notifier:0,put:0,protocol:1,limits:(),coefficients:pll,scripts:core::array::from_fn(|_|Vec::new()),
 surfaces:[memory::Range{start:0x10000,size:u64::from(((w*4+255)&!255)*h)};2],frame_pixels:1,pixel_hz:1,clock:None,latch_phase:None,last_presentation:None};
 let mut r=Registers::default();for(a,v)in modeset::head_methods(&p,0,0,1,0x10000,(u64::from(w)*4+255)&!255).unwrap(){r.r.insert(0x640000+a as usize,v);r.r.insert(0x660000+a as usize,v);}
 r.r.insert(0x616308,1);r.fail=fault;
 let s=present::Surface{resource:7,address:0x1000000,width:1920,height:1080,pitch:7680,block_height:0,format:0,dma:1};
 let result=select(&mut e,&mut r,s,0);
 if fault!=0 {
  assert!(result.is_err());
  assert_eq!(r.read(0x640200),0x101,"failed switch left the SOR detached");
  assert_eq!(r.read(0x640450),65000000,"failed switch did not restore the old timing");
  let old=present::Surface{width:w,height:h,pitch:(w*4+255)&!255,address:0x10000,..s};
  select(&mut e,&mut r,old,1).unwrap();assert_eq!(r.read(0x640200),0x101);return;
 }
 result.expect("larger listed scale-1 raster must be selectable");
 assert_eq!(r.read(0x640450),148500000,"scale-1 extent was mistaken for synthetic scale 2");
 assert_eq!(r.read(0x640460),s.address as u32>>8);assert_eq!(r.read(0x640468),1920|1080<<16);
}
fn select(e:&mut Evo,r:&mut Registers,s:present::Surface,index:u32)->Result<(), &'static str>{let _=index;CALL}
fn main(){run(1366,768,0);run(960,540,0);run(1366,768,1);run(1366,768,2);}
'''.replace('ROOT',str(ROOT)).replace('SELECT',select).replace('EXTRA',extra).replace('CALL',call)
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'check.rs').write_text(code)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'check.rs'),'-o',str(p/'check')],check=True)
            subprocess.run([str(p/'check')],check=True)

if __name__=='__main__':unittest.main()
