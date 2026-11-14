"""A rejected optional PLL/script must not disable the preferred raster."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from gt710_gr_live_unwind_test import item

class OptionalTiming(unittest.TestCase):
    def test_optional_recipes_are_filtered_before_publication(self):
        source=(ROOT/'kernel/src/gpu/gk208/native.rs').read_text()
        if 'fn runtime_rasters(' in source:
            recipe=item(source,'pub(super) fn runtime_rasters(').replace('pub(super) ','')
        else:
            start=source.index('    let rasters = modes.iter().map(')
            end=source.index('    let surface = layout.objects[0].range;',start)
            recipe='fn runtime_rasters(rom:&[u8],modes:&[edid::Timing],route:&board::Route,head:u32)->Result<Vec<RasterMode>, &\'static str>{let limits=();let protocol=1;'+source[start:end]+'Ok(rasters)}'
        code=r'''#![allow(dead_code)]
extern crate alloc;
#[path="ROOT/kernel/src/gpu/gk208/edid.rs"] mod edid;
#[derive(Clone)] struct RasterMode {timing:edid::Timing,pll:(),scripts:[Vec<()>;4]}
mod board {pub struct Route {pub sor:u8,pub conf:u32}}
mod modeset {
 pub fn board_pll(_: &[u8],_:u32)->Result<(), &'static str>{Ok(())}
 pub fn pll(_: &(),clock:u32)->Result<(), &'static str>{if clock==27000 {Err("VCO")}else{Ok(())}}
 pub struct Error {pub reason:&'static str}
 pub fn board_scripts(_: &[u8],_:u32,_:u32,_:u32,clock:u32)->Result<[Vec<()>;4],Error>{
  if clock==30000 {Err(Error{reason:"script"})}else{Ok(core::array::from_fn(|_|Vec::new()))}}
}
RECIPE
fn timing(clock:u32)->edid::Timing{edid::Timing{width:1024,height:768,clock_khz:clock,
 hsync_start:1048,hsync_end:1184,htotal:1344,vsync_start:771,vsync_end:777,vtotal:806,flags:24}}
fn main(){let route=board::Route{sor:0,conf:16};
 let modes=[timing(65000),timing(27000),timing(30000),timing(75000)];
 let rasters=runtime_rasters(&[],&modes,&route,0).expect("optional timing disabled preferred takeover");
 assert_eq!(rasters.iter().map(|r|r.timing.clock_khz).collect::<Vec<_>>(),[65000,75000]);
 assert!(runtime_rasters(&[],&[timing(27000),timing(65000)],&route,0).is_err());
}
'''.replace('ROOT',str(ROOT)).replace('RECIPE',recipe)
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'check.rs').write_text(code)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'check.rs'),'-o',str(p/'check')],check=True)
            subprocess.run([str(p/'check')],check=True)

if __name__=='__main__':unittest.main()
