"""The grouped desktop painter preserves every gradient and grid pixel."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class Background(unittest.TestCase):
 def test_gradient_and_grid_pixels(self):
  s=(ROOT/'userspace/programs/src/bwm/chrome.rs').read_text()
  helpers='\n'.join(item(s,k) for k in ['pub(super) fn background_pixel(', 'pub(super) fn background_row(', 'pub(super) fn paint_background('])
  if 'fn background_runs(' in s: helpers+='\n'+item(s,'pub(super) fn background_runs(')
  run_rust(self,r'''
use std::sync::atomic::{AtomicUsize,Ordering};
static SCALE:AtomicUsize=AtomicUsize::new(1);fn px(n:usize)->usize{n*SCALE.load(Ordering::Relaxed)}
#[derive(Clone,Copy,Debug,PartialEq)]struct Color(u32);impl Color{fn rgb(r:u8,g:u8,b:u8)->Self{Self(0xff000000|u32::from(r)<<16|u32::from(g)<<8|u32::from(b))}}
struct Surface{w:usize,h:usize,pixels:Vec<Color>}
impl Surface{fn width(&self)->usize{self.w}fn height(&self)->usize{self.h}fn batch(&mut self,f:impl FnOnce(&mut Self)){f(self)}}
fn fill_rect(fb:&mut Surface,x:i32,y:i32,w:usize,h:usize,c:Color){for row in y as usize..y as usize+h{for col in x as usize..x as usize+w{assert!(row<fb.h&&col<fb.w);fb.pixels[row*fb.w+col]=c;}}}
mod painter{use super::*;@HELPERS@ pub fn run(fb:&mut Surface){paint_background(fb)}pub fn pixel(x:usize,y:usize,h:usize)->Color{background_pixel(x,y,h)}}
#[test]fn all_pixels_match_at_both_display_scales_and_short_edges(){
 for(w,h,scale)in[(2560,1600,2),(1024,768,1),(17,19,1)]{
  SCALE.store(scale,Ordering::Relaxed);let mut fb=Surface{w,h,pixels:vec![Color(0);w*h]};painter::run(&mut fb);
  for y in 0..h{for x in 0..w{assert_eq!(fb.pixels[y*w+x],painter::pixel(x,y,h),"pixel {x},{y} at {w}x{h} scale{scale}");}}
 }
}
'''.replace('@HELPERS@',helpers))
if __name__=='__main__':unittest.main()
