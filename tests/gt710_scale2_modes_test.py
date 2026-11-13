"""Recorded EDID mode list and exact native scaler admission (#2952 u7)."""
import pathlib
import subprocess
import tempfile
import unittest
ROOT = pathlib.Path(__file__).resolve().parents[1]
class Scale2Modes(unittest.TestCase):
    def test_recorded_modes_and_viewports(self):
        code = r'''#![allow(dead_code)]
extern crate alloc;
#[path="ROOT/kernel/src/gpu/gk208/edid.rs"] mod edid;
#[path="ROOT/kernel/src/gpu/gk208/modeset.rs"] mod modeset;
fn main() {
 let hex=include_str!("ROOT/tests/fixtures/gt710-edid/lcd17-19.hex").trim();
 let e:Vec<u8>=(0..hex.len()).step_by(2).map(|i|u8::from_str_radix(&hex[i..i+2],16).unwrap()).collect();
 assert_eq!(modeset::display_modes(&e).unwrap().as_slice(), &[(1024,768,1),(1280,1024,1),(2560,2048,2)]);
 for (w,h) in [(1280,1024),(2560,2048)] {
  let m=modeset::Viewport::for_raster(1280,1024,w,h).unwrap().methods(0).unwrap();
  assert_eq!(m.iter().find(|(r,_)|*r==0x4b8).unwrap().1,w|h<<16);
  for r in [0x4c0,0x4c4,0x4c8] {assert_eq!(m.iter().find(|(a,_)|*a==r).unwrap().1,1280|1024<<16);}
 }
 for (w,h) in [(1024,768),(1280,800),(2560,1600),(1920,1536),(3840,3072),(0,0)] {
  assert!(modeset::Viewport::for_raster(1280,1024,w,h).is_err(),"admitted letterbox/ratio {w}x{h}");
 }
 assert!(modeset::Viewport::for_raster(1280,800,2560,1600).is_ok());
}
'''.replace('ROOT',str(ROOT))
        with tempfile.TemporaryDirectory() as tmp:
            src=pathlib.Path(tmp)/'check.rs'; src.write_text(code)
            exe=pathlib.Path(tmp)/'check'
            subprocess.run(['rustc','--edition=2021',str(src),'-o',str(exe)],check=True)
            subprocess.run([str(exe)],check=True)
if __name__=='__main__': unittest.main()
