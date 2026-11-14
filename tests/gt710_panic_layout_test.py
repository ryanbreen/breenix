"""Run prepared cell geometry and the panic pixel oracle at monitor sizes."""
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def function(source, name):
    start = source.index('fn ' + name + '(')
    start = source.rfind('\n', 0, start) + 1
    end = source.index('{', start)
    depth = 1
    at = end + 1
    while depth:
        depth += (source[at] == '{') - (source[at] == '}')
        at += 1
    return source[start:at]


class PanicLayout(unittest.TestCase):
    def test_full_grid_and_independent_pixel_coverage(self):
        prepared = (ROOT / 'kernel/src/gpu/gk208/prepared.rs').read_text()
        panic = (ROOT / 'kernel/src/gpu/panic.rs').read_text()
        stream = function(prepared, 'text_stream').replace('pub(crate) ', '')
        oracle = function(panic[panic.rindex('fn expected('):], 'expected')
        # The pre-fix signature has no raster input. Exercise that implementation
        # too, rather than making the red result a missing-API compile failure.
        stream_call = 'text_stream(42,43,8,width,height)' if re.search(r'\bwidth: u32', stream) else 'text_stream(42,43,8)'
        layout = ROOT / 'kernel/src/gpu/panic_layout.rs'
        layout_source = ('#[path="' + str(layout) + '"] mod panic_layout;\nuse panic_layout::Layout;\n') if layout.exists() else ''
        expected_call = 'expected(Layout::new(8,width,height).unwrap(),x,y)' if 'layout: Layout' in oracle else 'expected(8,x,y)'
        code = r'''
extern crate alloc;
use alloc::vec::Vec;
use gpu_abi::{draw,pipeline,stream};
use std::sync::atomic::{AtomicU8,Ordering};
const COLS:usize=80;const ROWS:usize=32;const CELLS:usize=COLS*ROWS;
const BG:u32=0xff6e1216;const FG:u32=0xffffebeb;
static SHOWN:[AtomicU8;CELLS]=[const{AtomicU8::new(b'X')};CELLS];
// A solid font makes lost glyph pixels measurable independently of the layout.
enum FontWeight{Regular} enum RasterHeight{Size16} struct Raster;
impl Raster{fn raster(&self)->&'static [[u8;8];16]{&[[255;8];16]}}
fn get_raster(_:char,_:FontWeight,_:RasterHeight)->Option<Raster>{Some(Raster)}
LAYOUT
STREAM
ORACLE
#[test]
fn every_cell_and_pixel_is_present(){
 for (width,height) in [(1280,1024),(1024,768),(2560,2048),(2560,1600)]{
  let scale=if 40+80*8*2<=width && 32+31*46+32<=height{2}else{1};
  let words=STREAM_CALL.unwrap();
  assert_eq!(words[7],draw::COMMAND);assert_eq!(words[7+9],2560);
  let mut clipped=0;let mut wrong=0;
  for i in 0..CELLS{
   let at=7+draw::HEADER+i*draw::INSTANCE;
   let rect=&words[at..at+4];let p=&words[at+4..at+20];
   let x=40+(i%80) as u32*8*scale;let y=32+(i/80) as u32*23*scale;
   clipped+=usize::from(rect[2]>width || rect[3]>height);
   wrong+=usize::from(rect!=[x,y,x+8*scale,y+16*scale] || p[..8]!=[x,y,0,0,8,16,scale,1]);
  }
  let mut foreground=0;
  for y in 0..height as usize{for x in 0..width as usize{
   foreground+=usize::from(EXPECTED_CALL==FG);
  }}
  assert_eq!((clipped,wrong,foreground),(0,0,CELLS*8*16*(scale*scale) as usize),"{width}x{height}");
 }
}
'''.replace('LAYOUT', layout_source).replace('STREAM_CALL', stream_call).replace('EXPECTED_CALL', expected_call).replace('STREAM', stream).replace('ORACLE', oracle)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            lib = p / 'libgpu_abi.rlib'
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-type=rlib', '--crate-name=gpu_abi', '--cfg', 'feature="alloc"', '-Dwarnings', str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(lib)], check=True)
            (p / 'check.rs').write_text(code)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings', '--extern', f'gpu_abi={lib}', str(p / 'check.rs'), '-o', str(p / 'check')], check=True)
            subprocess.run([str(p / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
