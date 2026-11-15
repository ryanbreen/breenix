"""Drive bcheck's startup self-check and ScrollBar drawing through host readback."""
from pathlib import Path
import os
import subprocess
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]

class Scrollbar(unittest.TestCase):
    def test_fitting_catalogs_skip_sampling_and_long_catalog_checks_exact_pixels(self):
        app = (ROOT/'userspace/programs/src/bcheck.rs').read_text()
        block = app[app.index('    let visible_h ='):app.index('    let mut active:')]
        bar = (ROOT/'libs/libbui/src/widget/scroll_bar.rs').read_text()
        methods = '\n'.join(item(bar, name) for name in ['pub fn new(', 'pub fn draw(', 'pub fn can_scroll(', 'pub fn max_scroll(', 'fn thumb_rect('])
        code = r'''
#![allow(dead_code)]
use libbui::Rect;
#[path="LAYOUT"] mod layout;
use layout::Layout;
#[derive(Clone,Copy)] struct Color([u8;3]);
impl Color {fn rgb(r:u8,g:u8,b:u8)->Self {Self([b,g,r])}}
struct FrameBuf {width:usize,scale:i32,pixels:Vec<u8>}
mod shapes {pub fn fill_rect(f:&mut super::FrameBuf,x:i32,y:i32,w:i32,h:i32,c:super::Color) {
 for y in y*f.scale..(y+h)*f.scale {for x in x*f.scale..(x+w)*f.scale {let at=(y as usize*f.width+x as usize)*4;f.pixels[at..at+3].copy_from_slice(&c.0);}}
}}
use libbui::Theme;
struct ScrollBar {rect:Rect,content_height:i32,view_height:i32,offset:i32,dragging:bool,drag_anchor:i32,enabled:bool,redraw:bool}
impl ScrollBar {METHODS}
struct TestDef {category:String}
CONTENT
struct Window {width:u32,height:u32}
impl Window {fn width(&self)->u32 {self.width} fn height(&self)->u32 {self.height} fn id(&self)->u32 {1}}
thread_local! {static PIXELS:std::cell::RefCell<Vec<u8>>=const {std::cell::RefCell::new(Vec::new())};static CAPTURES:std::cell::Cell<usize>=const {std::cell::Cell::new(0)};static CORRUPT:std::cell::Cell<bool>=const {std::cell::Cell::new(false)};}
mod libbreenix {pub mod window {pub fn capture(_:u32,p:&mut[u8],_:Option<()>)->Result<(),String> {
 super::super::CAPTURES.with(|n|n.set(n.get()+1));super::super::PIXELS.with(|v|p.copy_from_slice(&v.borrow()));Ok(())
}}}
fn frame(win:&mut Window,_:&[TestDef],_:i32,_:&mut Option<libgfx::ttf_font::CachedFont>,_:f32,scroll:Option<(&ScrollBar,&Theme)>)->Result<(),()> {
 let mut f=FrameBuf {width:win.width as usize,scale:(win.width/800) as i32,pixels:vec![0;win.width as usize*win.height as usize*4]};
 if let Some((bar,theme))=scroll {bar.draw(&mut f,theme);if CORRUPT.with(|v|v.get()) {let at=((bar.rect.y*f.scale) as usize*f.width+(bar.rect.x*f.scale) as usize)*4;f.pixels[at]=99;}}
 PIXELS.with(|p|*p.borrow_mut()=f.pixels);Ok(())
}
fn main() {
 let args:Vec<_>=std::env::args().collect();let rows:usize=args[1].parse().unwrap();let ui_scale:i32=args[2].parse().unwrap();
 CORRUPT.with(|v|v.set(args[3]=="bad"));
 let mut win=Window {width:800*ui_scale as u32,height:600*ui_scale as u32};let mut ttf_font=None;let font_size=10.0;
 let tests:Vec<_>=(0..rows).map(|_|TestDef{category:"one".into()}).collect();
 let layout=Layout::new(None,font_size,ui_scale as u32,win.width as i32,win.height as i32);
 BLOCK
 let _ = &mut scroll_bar;
 let expected=usize::from(rows==100);assert_eq!(CAPTURES.with(|n|n.get()),expected,"fitting catalog must not sample an absent scrollbar");
}
'''.replace('LAYOUT',str(ROOT/'userspace/programs/src/bcheck/layout.rs')).replace('METHODS',methods).replace('CONTENT',item(app,'fn content_height(')).replace('BLOCK',block)
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp);(path/'src').mkdir();(path/'src/main.rs').write_text(code)
            (path/'Cargo.toml').write_text(f'[package]\nname="bcheck-scrollbar-host"\nversion="0.0.0"\nedition="2021"\n[workspace]\n[dependencies]\nlibbui={{path="{ROOT}/libs/libbui"}}\nlibgfx={{path="{ROOT}/libs/libgfx"}}\n')
            env=dict(os.environ,CARGO_BUILD_JOBS='4',RUSTFLAGS='-Dwarnings',CARGO_TARGET_DIR='/tmp/bcheck-scrollbar-host-target')
            built=subprocess.run(['taskpolicy','-b','cargo','build','--offline','--manifest-path',str(path/'Cargo.toml')],env=env,capture_output=True,text=True)
            self.assertEqual(built.returncode,0,built.stdout+built.stderr)
            for scale in [1,2]:
                for rows in [1,2,5,100]:
                    with self.subTest(scale=scale,rows=rows):
                        ran=subprocess.run(['/tmp/bcheck-scrollbar-host-target/debug/bcheck-scrollbar-host',str(rows),str(scale),'good'],capture_output=True,text=True)
                        self.assertEqual(ran.returncode,0,ran.stdout+ran.stderr)
                        self.assertNotIn('FAILED',ran.stdout)
                        if rows==100:self.assertIn('2 pixels exact',ran.stdout)
                ran=subprocess.run(['/tmp/bcheck-scrollbar-host-target/debug/bcheck-scrollbar-host','100',str(scale),'bad'],capture_output=True,text=True)
                self.assertIn('scrollbar layout FAILED',ran.stdout,'long catalogs must retain exact readback')

if __name__=='__main__':unittest.main()
