"""Exercise startup presentation and chrome rendering at their device boundaries."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT = Path(__file__).resolve().parents[1]
class StartupWork(unittest.TestCase):
    def test_hidden_pointer_reuses_verified_fill_presentation(self):
        source = (ROOT/'userspace/programs/src/bwm.rs').read_text()
        start = source.index('    }, || {', source.index('let (prepared, presented)')) + len('    }, || {')
        body = source[start:source.index('\n    });', start)]
        run_rust(self, r'''
use std::cell::Cell;
mod libuitest{pub mod startup{pub fn mark(_: &str){}}}
fn cursor_pointer(p:bool)->Option<bool>{p.then_some(p)}
fn measure_check<T>(f:impl FnOnce()->T)->T{f()}
struct Gpu{presents:usize,checks:Cell<usize>,fail:bool}
impl Gpu{
 fn present_startup_desktop(&mut self,_:Option<bool>)->Result<(),&'static str>{self.presents+=1;if self.fail{Err("present")}else{Ok(())}}
 fn check(&self,_:u64,_:&[()],p:Option<bool>)->u64{self.checks.set(self.checks.get()+1);if p.is_some(){7}else{9}}
}
fn verify(gpu:&mut Gpu,first_pointer:bool)->Result<u64,&'static str>{@BODY@}
#[test]fn independent_full_check_survives_reused_present(){
 for pointer in [false,true]{for fail in [false,true]{let mut gpu=Gpu{presents:0,checks:Cell::new(0),fail};
 let result=verify(&mut gpu,pointer);
 assert_eq!(gpu.presents,usize::from(pointer),"hidden cursor re-presented the unchanged verified desktop");
 assert_eq!(gpu.checks.get(),usize::from(!pointer||!fail));
 assert_eq!(result,if pointer&&fail{Err("present")}else{Ok(if pointer{7}else{9})});}}
}
'''.replace('@BODY@',body))

    def test_title_warmup_includes_chrome_buttons(self):
        source=(ROOT/'userspace/programs/src/bwm.rs').read_text()
        start=source.index('        libuitest::startup::mark("title-plan-begin")')
        body=source[start:source.index('\n    }, || {',start)]
        run_rust(self,r'''
use std::collections::BTreeSet;
mod libuitest{pub mod startup{pub fn mark(_: &str){}}}
struct Ui{scale:u32}fn ui()->Ui{Ui{scale:2}}fn ttf_font_size()->f32{14.0}
struct Window{window_id:u64,chromeless:bool,title:Vec<u8>,title_len:usize}
struct Stack;impl Stack{fn minimized(&self,id:u64)->bool{id==2}}
fn ordered_windows<'a>(w:&'a[Window],_:&Stack)->impl Iterator<Item=&'a Window>{w.iter()}
struct Font{seen:BTreeSet<u8>}impl Font{fn prepare_subpixel(&mut self,t:&[&[u8]],size:f32)->Result<(),String>{assert_eq!(size,28.0);self.seen.extend(t.iter().flat_map(|s|s.iter()).copied());Ok(())}}
#[test]fn every_first_window_glyph_is_warm(){
 let windows=[Window{window_id:1,chromeless:false,title:b"startup app".to_vec(),title_len:11},Window{window_id:2,chromeless:false,title:b"Q".to_vec(),title_len:1}];let stack=Stack;let mut ui_font=Some(Font{seen:BTreeSet::new()});
 let result:Result<(),String>=(||{@BODY@})();result.unwrap();
 let seen=&ui_font.unwrap().seen;for b in b"startup appx-"{assert!(seen.contains(b),"cold chrome glyph {} remained after warmup",*b as char);}assert!(!seen.contains(&b'Q'));
}
'''.replace('@BODY@',body))

    def test_text_keeps_pixels_and_batches_glyph_draws(self):
        body=item((ROOT/'userspace/programs/src/bwm/chrome.rs').read_text(),'pub(super) fn draw_text_at(').replace('pub(super) ','')
        run_rust(self,r'''
type Color=u32;
struct Ui{scale:u32,cell_w:usize,font:u32}fn ui()->Ui{Ui{scale:2,cell_w:8,font:0}}fn ttf_font_size()->f32{14.0}
#[derive(Clone,Copy)]struct Clip;
struct Surface{pixels:Vec<(u8,i32,i32,Color)>,submits:usize}impl Surface{fn height(&self)->usize{1600}fn width(&self)->usize{2560}fn clip(&self)->Clip{Clip}}
struct CachedFont;
mod ttf_font{use super::*;
 pub struct TextRun<'a>{pub bytes:&'a[u8],pub x:i32,pub y:i32,pub size:f32,pub fg:Color,pub clip:Clip}
 pub fn draw_text_subpixel(f:&mut Surface,_:&mut CachedFont,s:&str,x:i32,y:i32,size:f32,c:Color){assert_eq!(size,28.0);for(i,b)in s.bytes().enumerate(){f.submits+=1;f.pixels.push((b,x+8*i as i32,y,c));}}
 pub fn draw_runs_subpixel(f:&mut Surface,_:&mut CachedFont,r:&[TextRun<'_>]){f.submits+=1;for r in r{let _=r.clip;assert_eq!(r.size,28.0);for(i,b)in r.bytes.iter().enumerate(){f.pixels.push((*b,r.x+8*i as i32,r.y,r.fg));}}}
}
mod bitmap_font{use super::*;pub fn draw_text_at(f:&mut Surface,t:&[u8],x:usize,y:usize,c:Color,_:u32){for(i,b)in t.iter().enumerate(){f.pixels.push((*b,x as i32+8*i as i32,y as i32,c));}}}
@BODY@
#[test]fn chrome_lcd_pixels_and_draw_count(){
 let _=ttf_font::draw_text_subpixel;let _=ttf_font::draw_runs_subpixel;
 for text in [b"startup app".as_slice(),b"x",b"-"]{let mut f=Surface{pixels:vec![],submits:0};draw_text_at(&mut f,text,13,27,9,Some(&mut CachedFont));
 assert_eq!(f.pixels,text.iter().enumerate().map(|(i,b)|(*b,13+8*i as i32,27,9)).collect::<Vec<_>>());assert_eq!(f.submits,1,"chrome submitted once per glyph");}
 let mut f=Surface{pixels:vec![],submits:0};draw_text_at(&mut f,b"xy",-8,27,9,None);assert_eq!(f.pixels,vec![(b'y',0,27,9)]);
 draw_text_at(&mut f,b"offscreen",0,1600,9,Some(&mut CachedFont));assert_eq!(f.submits,0);
}
'''.replace('@BODY@',body))
if __name__=='__main__':unittest.main()
