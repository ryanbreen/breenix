"""Exercise the native console ribbon's paint and dropped-stage control."""
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
source = r'''extern crate alloc;
mod boot_ribbon { pub fn reached()->Vec<bool>{vec![true,false]} }
mod console {
 use crate::boot_ribbon;
 pub mod raster {
  use std::ops::Range;
  pub struct Layout {pub width:usize,pub height:usize,pub bytes_per_pixel:usize}
  impl Layout {
   pub fn line(&self,y:usize)->Range<usize>{y*self.width*4..(y+1)*self.width*4}
   pub fn fill(&self,bytes:&mut [u8],c:[u8;3]){for px in bytes.chunks_exact_mut(4){px[..3].copy_from_slice(&c);}}
  }
 }
 #[path="RIBBON"] pub mod ribbon;
}
fn main(){
 use console::{raster::Layout,ribbon::Ribbon};
 let layout=Layout{width:1280,height:800,bytes_per_pixel:4};
 let normal=Ribbon::now(&layout,false).unwrap();
 let dropped=Ribbon::now(&layout,true).unwrap();
 let mut frame=vec![0;1280*800*4];
 normal.paint(&mut frame,&layout,772..778);
 assert_eq!(&frame[(772*1280+112)*4..(772*1280+112)*4+3], &[26,112,77]);
 assert_eq!(normal.colour(112,772),Some([26,112,77]));
 assert_eq!(dropped.colour(112,772),Some([48,67,96]));
 assert_eq!(normal.colour(688,772),Some([48,67,96]));
}
'''.replace('RIBBON', str(ROOT / 'kernel/src/console/ribbon.rs'))
with tempfile.TemporaryDirectory() as tmp:
    path = Path(tmp)
    (path / 'test.rs').write_text(source)
    command = (['taskpolicy', '-b'] if sys.platform == 'darwin' else []) + ['rustc', '--edition=2021', '-Dwarnings', str(path / 'test.rs'), '-o', str(path / 'test')]
    subprocess.run(command, check=True)
    subprocess.run([str(path / 'test')], check=True)
