"""Count native corner conversions and check the unchanged vertex bytes."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class QuadCoordinates(unittest.TestCase):
    def test_convert_four_corners_once_per_rectangle(self):
        # Count calls at the arithmetic boundary; execute the production quads
        # body, with the actual integer binary32 routine and shader metadata.
        encoder = (ROOT / 'kernel/src/gpu/gk208/encode.rs').read_text().replace(
            'gpu_abi::float::ratio_bits(', 'crate::count_ratio(').replace('include_bytes!("caps/integer.inputs")', 'include_bytes!("' + str(ROOT / 'kernel/src/gpu/gk208/caps/integer.inputs') + '")')
        check = r'''
pub fn check() {
 for (width,height,rects) in [
  (64,64,vec![[0,0,64,64],[1,2,17,31]]),
  (319,197,vec![[0,0,319,197],[17,25,123,191],[159,98,160,99]]),
  (2560,1600,vec![[1279,799,1281,801],[0,0,1,1]])] {
  let t=Target{id:1,address:0x400000,width,height,format:gpu_abi::format::B8G8R8A8,tile_mode:0};
  let mut f=Frame{probe_mask:0,chunks:Vec::new(),data:Vec::new(),removed_prefix:DATA_START};
  crate::CALLS.store(0,std::sync::atomic::Ordering::Relaxed);
  quads(&mut f,t,rects.iter().copied(),[0,0,width,height],&[1,2,3,4],gpu_abi::pipeline::SOLID,None,None,None).unwrap();
  let calls=crate::CALLS.load(std::sync::atomic::Ordering::Relaxed);
  // Compare every vertex field with the old six-vertex calculation.
  let mut expected=Vec::new();
  for &[x0,y0,x1,y1] in &rects {
   for [x,y] in [[x0,y0],[x1,y0],[x1,y1],[x0,y0],[x1,y1],[x0,y1]] {
    let coord=|p:u32,size:u32| {let n=i64::from(p)*2-i64::from(size);
     gpu_abi::float::ratio_bits(n.unsigned_abs() as u32,size)|if n<0{1<<31}else{0}};
    for word in [coord(x,width),coord(y,height),0,0x3f800000,0,0,0,0] {expected.extend(word.to_le_bytes());}
   }
  }
  assert_eq!(&f.data[256..256+expected.len()],expected.as_slice());
  assert_eq!(calls,rects.len()*4,"convert each distinct rectangle coordinate once");
 }
}
'''
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as d:
            p = Path(d)
            (p / 'encode.rs').write_text(encoder + check)
            (p / 'check.rs').write_text(r'''
extern crate alloc;
use std::sync::atomic::{AtomicUsize,Ordering};
static CALLS:AtomicUsize=AtomicUsize::new(0);
fn count_ratio(n:u32,d:u32)->u32{CALLS.fetch_add(1,Ordering::Relaxed);gpu_abi::float::ratio_bits(n,d)}
#[allow(dead_code)] // Import the production catalogue; this check uses SOLID.
#[path="SHADERS"]pub mod shaders;
#[allow(dead_code)] // Probe geometry remains provided by the production module.
#[path="CAPS"]pub mod caps;
#[allow(dead_code)] // Import the complete encoder to exercise its private quads.
mod encode;
fn main(){encode::check();}
'''.replace('SHADERS', str(ROOT / 'kernel/src/gpu/gk208/gr/shaders.rs')).replace('CAPS', str(ROOT / 'kernel/src/gpu/gk208/caps.rs')))
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi', '--crate-type', 'rlib', '--cfg', 'feature="alloc"', '-Dwarnings', str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(p / 'libgpu_abi.rlib')], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', '--extern', f'gpu_abi={p / "libgpu_abi.rlib"}', str(p / 'check.rs'), '-o', str(p / 'check')], check=True)
            subprocess.run([str(p / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
