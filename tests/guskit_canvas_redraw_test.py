"""Count the production renderer's primitives on a busy drawing and pointer preview."""
import unittest
from guskit_new_save_test import ROOT, run_host
from gt710_gr_live_unwind_test import item


class CanvasRedraw(unittest.TestCase):
    def test_noise_pointer_preview_emits_only_changed_pixels_and_one_copy(self):
        source = (ROOT / 'userspace/programs/src/guskit.rs').read_text()
        mapping = (ROOT / 'userspace/programs/src/guskit/node.rs').read_text()
        functions = '\n'.join(item(mapping, f'pub fn {name}(')
                              for name in ['hue_at', 'sv_at', 'hsv_to_rgb'])
        methods = '\n'.join(item(source, f'fn {name}(') for name in ['color', 'draw_stroke'])
        run_host(self, r'''
#[cfg(test)] mod check {
use std::cell::Cell;
thread_local! { static PRIMITIVES:Cell<usize>=const { Cell::new(0) }; static BLITS:Cell<usize>=const { Cell::new(0) }; }
#[allow(dead_code)] #[path="CANVAS"] mod canvas;
use canvas::{Canvas,History,Point,Radius};
#[derive(Clone,Copy,Debug,PartialEq)] struct Color([u8;3]);
impl Color { const WHITE:Self=Self([255;3]); fn rgb(r:u8,g:u8,b:u8)->Self { Self([r,g,b]) } }
struct Surface { width:usize,height:usize,pixels:Vec<Color> }
impl Surface {
    fn new(width:usize,height:usize,_:libgfx::surface::Format)->Result<Self,String> { Ok(Self { width,height,pixels:vec![Color::WHITE;width*height] }) }
    fn batch(&mut self,f:impl FnOnce(&mut Self)) { f(self); }
    fn clear(&mut self,c:Color) { PRIMITIVES.with(|n|n.set(n.get()+1)); self.pixels.fill(c); }
    fn put_pixel(&mut self,x:usize,y:usize,c:Color) { PRIMITIVES.with(|n|n.set(n.get()+1)); self.pixels[y*self.width+x]=c; }
    fn wait(&self)->Result<(),String> { Ok(()) }
}
mod libgfx {
    pub mod surface { pub struct Format; impl Format { pub const BGRX:Self=Self; } }
    pub mod blit {
        pub fn blit(dst:&mut super::super::Surface,src:&super::super::Surface,x:i32,y:i32) {
            super::super::PRIMITIVES.with(|n|n.set(n.get()+1)); super::super::BLITS.with(|n|n.set(n.get()+1));
            for sy in 0..src.height { for sx in 0..src.width { let (dx,dy)=(sx+x as usize,sy+y as usize); if dx<dst.width && dy<dst.height { dst.pixels[dy*dst.width+dx]=src.pixels[sy*src.width+sx]; } } }
        }
    }
}
mod shapes {
    pub fn fill_rect(fb:&mut super::Surface,x:i32,y:i32,w:i32,h:i32,c:super::Color) {
        super::PRIMITIVES.with(|n|n.set(n.get()+1));
        for py in y..y+h { for px in x..x+w { if px>=0 && py>=0 && (px as usize)<fb.width && (py as usize)<fb.height { fb.pixels[py as usize*fb.width+px as usize]=c; } } }
    }
}
mod bitmap_font { pub fn draw_text(_:&mut super::Surface,_:&[u8],_:usize,_:usize,_:super::Color) {} }
struct Theme; impl Theme { fn dark()->Self { Self } }
struct MenuBar; impl MenuBar { fn draw(&self,_:&mut Surface,_:&Theme) {} }
struct Picker; impl Picker { fn draw(&self,_:&mut Surface,_:&Theme) {} }
struct App {
    width:usize,height:usize,tool:usize,size:usize,hue:u16,sat:u8,val:u8,selected_color:Option<[u8;3]>,recent:[[u8;3];8],
    stroke:Option<Vec<Point>>,history:History,message:String,picker:Option<Picker>,confirm:bool,
    palette:Option<(u16,Surface)>,canvas_surface:Option<(Canvas,Surface)>,
}
impl App { METHODS }
const TOP:i32=176;
const LABELS:[&[u8];7]=[b"Pen",b"Brsh",b"Line",b"Rect",b"Circ",b"Fill",b"Ers"];
mod node { MAPPING }
RENDER
#[test] fn noise_then_pointer_move() {
    let mut image=Canvas::new(800,424);
    let mut seed=1u32;
    for pixel in image.pixels_mut().chunks_exact_mut(3) {
        seed^=seed<<13; seed^=seed>>17; seed^=seed<<5;
        pixel.copy_from_slice(&seed.to_le_bytes()[..3]);
    }
    let mut app=App { width:800,height:424,tool:0,size:0,hue:0,sat:0,val:0,selected_color:None,recent:[[0;3];8],
        stroke:None,history:History::new(image),message:String::new(),picker:None,confirm:false,palette:None,canvas_surface:None };
    let mut fb=Surface::new(800,600,libgfx::surface::Format::BGRX).unwrap();
    render(&mut fb,&mut app,&MenuBar);
    assert!(PRIMITIVES.with(Cell::get)>300_000,"fixture must exercise a busy image");
    // Press, then deliver one more pointer sample to the pencil preview.
    app.stroke=Some(vec![(200,200)]); render(&mut fb,&mut app,&MenuBar);
    app.stroke.as_mut().unwrap().push((201,200));
    PRIMITIVES.with(|n|n.set(0)); BLITS.with(|n|n.set(0));
    render(&mut fb,&mut app,&MenuBar);
    let emitted=PRIMITIVES.with(Cell::get);
    assert!(emitted<=32,"one changed pixel plus fixed toolbar and canvas copies, emitted {emitted}");
    assert_eq!(BLITS.with(Cell::get),2,"one picker copy and one canvas copy");
    let expected=app.history.canvas().preview(app.stroke.as_ref().unwrap(),|c,p|app.draw_stroke(c,p));
    for y in 0..424 { for x in 0..800 { let i=(y*800+x)*3; assert_eq!(fb.pixels[(y+176)*800+x],Color(expected.pixels()[i..i+3].try_into().unwrap())); } }
    // Cancelling a preview must restore the pixels it covered on the retained surface.
    app.stroke=None; render(&mut fb,&mut app,&MenuBar);
    for y in 0..424 { for x in 0..800 { let i=(y*800+x)*3; assert_eq!(fb.pixels[(y+176)*800+x],Color(app.history.canvas().pixels()[i..i+3].try_into().unwrap())); } }
}
}
'''.replace('CANVAS', str(ROOT / 'userspace/programs/src/guskit/canvas.rs'))
   .replace('METHODS', methods).replace('MAPPING', functions).replace('RENDER', item(source, 'fn render('))
   # Before retention lands, these future cache fields are absent from render.
   .replace('struct App {', '#[allow(dead_code)] struct App {'))


if __name__ == '__main__':
    unittest.main()
