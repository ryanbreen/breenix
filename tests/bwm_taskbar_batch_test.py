"""Execute the production taskbar paint with a submission-counting LCD renderer."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item
ROOT = Path(__file__).resolve().parents[1]

class TaskbarBatch(unittest.TestCase):
    def test_taskbar_keeps_lcd_pixels_without_per_glyph_submissions(self):
        import os
        source=Path(os.environ.get('BWM_CHROME_SOURCE',ROOT/'userspace/programs/src/bwm/chrome.rs'))
        draw = item(source.read_text(), 'pub(super) fn draw_taskbar(').replace('pub(super)', '')
        fixture = r"""
use std::collections::BTreeMap;
type Color=u32;
const TASKBAR_HEIGHT:usize=24;
const TASKBAR_BG:Color=1;
const TASKBAR_TEXT:Color=2;
const TTF_FONT_SIZE:f32=16.;
fn ttf_font_size()->f32{TTF_FONT_SIZE}
struct Ui{cell_h:usize,cell_w:usize,scale:usize}
fn ui()->Ui{Ui{cell_h:16,cell_w:8,scale:1}}
fn px(n:usize)->usize{n} fn pxi(n:usize)->i32{n as i32}
#[derive(Clone,Copy)]struct Clip{x:i32,y:i32,w:u32,h:u32}
struct Surface{pixels:BTreeMap<(i32,i32),u32>,submits:usize}
impl Surface{fn width(&self)->usize{640}fn clip(&self)->Clip{Clip{x:0,y:0,w:640,h:480}}}
struct CachedFont;
fn fill_rect(f:&mut Surface,x:i32,y:i32,w:usize,h:usize,c:Color){f.submits+=1;for yy in y..y+h as i32{for xx in x..x+w as i32{f.pixels.insert((xx,yy),c);}}}
fn glyph(f:&mut Surface,b:u8,x:i32,y:i32,c:Color){for yy in 0..8{for xx in 0..6{f.pixels.insert((x+xx,y+yy),c+(b as u32)*3+(xx as u32)%3);}}}
fn draw_text_at(f:&mut Surface,t:&[u8],x:i32,y:i32,c:Color,_:Option<&mut CachedFont>){for(i,b)in t.iter().enumerate(){f.submits+=1;glyph(f,*b,x+8*i as i32,y,c);}}
mod ttf_font{
 use super::*;
 pub struct TextRun<'a>{pub bytes:&'a[u8],pub x:i32,pub y:i32,pub size:f32,pub fg:Color,pub clip:Clip}
 pub fn draw_runs_subpixel(f:&mut Surface,_:&mut CachedFont,r:&[TextRun<'_>]){f.submits+=1;for r in r{assert_eq!(r.size,16.);assert_eq!((r.clip.x,r.clip.y,r.clip.w,r.clip.h),(0,0,640,480));for(i,b)in r.bytes.iter().enumerate(){glyph(f,*b,r.x+8*i as i32,r.y,r.fg);}}}
}
DRAW
fn main(){
 let mut probe=Surface{pixels:BTreeMap::new(),submits:0};
 let clip=probe.clip();
 assert_eq!(TTF_FONT_SIZE*ui().scale as f32,16.);
 ttf_font::draw_runs_subpixel(&mut probe,&mut CachedFont,&[ttf_font::TextRun{bytes:b"",x:0,y:0,size:16.,fg:0,clip}]);
 for text in [&b"12:34:56 ET"[..],&b""[..]]{
 let mut reference=Surface{pixels:BTreeMap::new(),submits:0};
 fill_rect(&mut reference,0,0,640,24,TASKBAR_BG);
 draw_text_at(&mut reference,b"Breenix",8,5,TASKBAR_TEXT,None);
 if !text.is_empty(){draw_text_at(&mut reference,text,640-text.len() as i32*8-8,5,TASKBAR_TEXT,None);}
 let mut actual=Surface{pixels:BTreeMap::new(),submits:0};
 draw_taskbar(&mut actual,text,&mut Some(CachedFont));
 assert_eq!(actual.pixels,reference.pixels,"LCD pixels and placement changed");
 assert!(actual.submits<=2,"taskbar queued {} draws before scene admission",actual.submits);
 }
}
""".replace('DRAW',draw)
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'check.rs').write_text(fixture)
            priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'check.rs'),'-o',str(p/'check')],check=True)
            subprocess.run([str(p/'check')],check=True)

if __name__=='__main__': unittest.main()
