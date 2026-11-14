"""Prepare terminal output independently of the live runtime raster."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from gt710_gr_live_unwind_test import item

class TerminalRaster(unittest.TestCase):
    def test_preparation_while_a_smaller_mode_is_held(self):
        native=(ROOT/'kernel/src/gpu/gk208/native.rs').read_text()
        emergency=(ROOT/'kernel/src/gpu/gk208/emergency.rs').read_text()
        function=item(native,'    pub(in crate::gk208) fn terminal_display(').replace('pub(in crate::gk208)','pub').replace('super::present::','present::').replace('super::emergency::','emergency::')
        display=item(emergency,'pub(crate) struct Display').replace('pub(crate)','pub')
        raster=item(emergency,'pub(crate) struct Raster') if 'pub(crate) struct Raster' in emergency else ''
        code=r'''#![allow(dead_code)]
extern crate alloc;
use std::{cell::RefCell,collections::BTreeMap};
thread_local!{static REGS:RefCell<BTreeMap<usize,u32>>=RefCell::new(BTreeMap::new());}
#[path="ROOT/kernel/src/gpu/gk208/edid.rs"]mod edid;
#[path="ROOT/kernel/src/gpu/gk208/modeset.rs"]mod modeset;
#[path="ROOT/kernel/src/gpu/gk208/present.rs"]mod present;
struct Registers(usize);
impl Registers{fn read(&self,r:usize)->u32{REGS.with(|s|*s.borrow().get(&r).unwrap_or(&0))}}
impl modeset::Io for Registers{fn read(&mut self,r:usize)->u32{Registers::read(self,r)}fn write(&mut self,r:usize,v:u32){REGS.with(|s|s.borrow_mut().insert(r,v));}fn delay_us(&mut self,_:u32){}}
fn write_words(_: &mut Registers,_:u64,_:u64,_:&[u32])->Result<(), &'static str>{Ok(())}
#[derive(Clone)]struct RasterMode{timing:edid::Timing,pll:modeset::Pll,scripts:[Vec<modeset::Op>;4]}
struct Evo{head:u32,sor:u32,total:u64,push:u64,notifier:u64,protocol:u32,limits:modeset::PllLimits,rasters:Vec<RasterMode>}
mod emergency{IMPORT RASTER
DISPLAY}
impl Evo{FUNCTION}
fn main(){let t=edid::Timing{width:1280,height:1024,clock_khz:108000,hsync_start:1328,hsync_end:1440,htotal:1688,vsync_start:1025,vsync_end:1028,vtotal:1066,flags:30};
 let e=Evo{head:0,sor:0,total:1<<30,push:0x10000,notifier:0x11000,protocol:1,
 limits:modeset::PllLimits{reg:0x614100,reference:27000,min_vco:500000,max_vco:1000000,min_input:1000,max_input:27000,min_m:1,max_m:255,min_n:1,max_n:255,min_p:1,max_p:63},
 rasters:vec![RasterMode{timing:t,pll:modeset::Pll{m:2,n:71,p:9,fraction:4096},scripts:core::array::from_fn(|_|Vec::new())}]};
 let surface=present::Surface{resource:42,address:0x8000000,width:1280,height:1024,pitch:5120,block_height:0,format:0,dma:1};
 for (w,h) in [(1024,768),(1280,1024)] {REGS.with(|s|{let mut s=s.borrow_mut();s.insert(0x64041c,0);s.insert(0x640420,w|h<<16);});
  let d=e.terminal_display(0,surface).expect("panic preparation depends on active runtime raster");
  assert_eq!(d.methods.iter().find(|(m,_)|*m==0x4c0).unwrap().1,1280|1024<<16);
 }
}
'''.replace('ROOT',str(ROOT)).replace('IMPORT','use super::modeset;' if raster else '').replace('RASTER',raster).replace('DISPLAY',display).replace('FUNCTION',function)
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'check.rs').write_text(code)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'check.rs'),'-o',str(p/'check')],check=True)
            subprocess.run([str(p/'check')],check=True)
if __name__=='__main__':unittest.main()
