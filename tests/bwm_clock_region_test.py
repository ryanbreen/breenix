"""Clock ticks must preserve static taskbar pixels without redrawing their text."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class ClockRegion(unittest.TestCase):
 def test_only_the_clock_region_is_repainted_and_clip_is_restored(self):
  s=(ROOT/'userspace/programs/src/bwm/chrome.rs').read_text()
  body=item(s,'pub(super) fn redraw_clock(') if 'fn redraw_clock(' in s else 'pub(super) fn redraw_clock(fb:&mut Surface,text:&[u8],font:&mut Option<CachedFont>){draw_taskbar(fb,text,font)}'
  run_rust(self,r'''
#[derive(Clone,Copy,Debug,PartialEq)]struct Rect{x:i32,y:i32,w:i32,h:i32}impl Rect{fn new(x:i32,y:i32,w:i32,h:i32)->Self{Self{x,y,w,h}}}
pub mod libgfx{pub use super::Rect;}
const TASKBAR_HEIGHT:usize=28;fn px(n:usize)->usize{n*2}
struct Surface{width:usize,clip:Rect,saved:Vec<Rect>,pixels:Vec<u8>,label_draws:usize,clock_draws:usize}
impl Surface{fn width(&self)->usize{self.width}fn push_clip(&mut self,r:Rect){self.saved.push(self.clip);self.clip=r}fn pop_clip(&mut self){self.clip=self.saved.pop().unwrap()}}
struct CachedFont;
fn draw_taskbar(fb:&mut Surface,text:&[u8],_:&mut Option<CachedFont>){assert_eq!(text,b"07:58:11 ET");for x in fb.clip.x as usize..(fb.clip.x+fb.clip.w)as usize{fb.pixels[x]=9;}if fb.clip.x<32{fb.label_draws+=1;}if fb.clip.x+fb.clip.w>fb.width as i32-32{fb.clock_draws+=1;}}
mod production{use super::*;@BODY@pub fn run(fb:&mut Surface){redraw_clock(fb,b"07:58:11 ET",&mut None)}}
#[test]fn stable_label_is_not_repainted_on_each_second(){
 let original=Rect::new(0,0,2560,1600);let mut fb=Surface{width:2560,clip:original,saved:vec![],pixels:vec![7;2560],label_draws:0,clock_draws:0};
 let _=TASKBAR_HEIGHT;let _=px;let _=Surface::width;let _=Surface::push_clip;let _=Surface::pop_clip;
 production::run(&mut fb);
 assert_eq!(fb.label_draws,0,"a clock tick redrew the unchanged Breenix label");assert_eq!(fb.clock_draws,1);
 assert!(fb.pixels[..1280].iter().all(|p|*p==7));assert!(fb.pixels[1280..].iter().all(|p|*p==9));assert_eq!(fb.clip,original);assert!(fb.saved.is_empty());
}
'''.replace('@BODY@',body).replace('struct Rect{','pub struct Rect{'))
  self.assertEqual((ROOT/'userspace/programs/src/bwm.rs').read_text().count('redraw_clock(&mut gpu.desktop'),2)
if __name__=='__main__':unittest.main()
