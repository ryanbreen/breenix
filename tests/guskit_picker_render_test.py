"""Exercise Gus Kit's renderer with recorded GPU primitives and exact picker pixels."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class PickerRender(unittest.TestCase):
    def test_unchanged_picker_reuses_pixels_and_hue_refreshes_them(self):
        source = (ROOT / 'userspace/programs/src/guskit.rs').read_text()
        mapping = (ROOT / 'userspace/programs/src/guskit/node.rs').read_text()
        functions = '\n'.join(item(mapping, f'pub fn {name}(')
                              for name in ['hue_at', 'sv_at', 'hsv_to_rgb'])
        run_rust(self, r'''
use std::cell::Cell;
thread_local! { static PIXELS: Cell<usize> = const { Cell::new(0) }; }
#[derive(Clone, Copy, Debug, PartialEq)] struct Color([u8; 3]);
impl Color {
    const WHITE: Self = Self([255; 3]);
    fn rgb(r: u8, g: u8, b: u8) -> Self { Self([r,g,b]) }
}
struct Surface { width: usize, height: usize, pixels: Vec<Color> }
impl Surface {
    fn new(width: usize, height: usize, _: libgfx::surface::Format) -> Result<Self, String> {
        Ok(Self { width, height, pixels: vec![Color::WHITE; width*height] })
    }
    fn batch(&mut self, f: impl FnOnce(&mut Self)) { f(self); }
    fn clear(&mut self, c: Color) { self.pixels.fill(c); }
    fn put_pixel(&mut self, x: usize, y: usize, c: Color) {
        PIXELS.with(|n| n.set(n.get()+1));
        if x < self.width && y < self.height { self.pixels[y*self.width+x] = c; }
    }
    fn wait(&self) -> Result<(), String> { Ok(()) }
}
mod libgfx {
    pub mod surface { pub struct Format; impl Format { pub const BGRX: Self = Self; } }
    pub mod blit {
        pub fn blit(dst: &mut super::super::Surface, src: &super::super::Surface, x: i32, y: i32) {
            for sy in 0..src.height { for sx in 0..src.width {
                let (dx,dy)=(sx+x as usize,sy+y as usize);
                if dx < dst.width && dy < dst.height { dst.pixels[dy*dst.width+dx]=src.pixels[sy*src.width+sx]; }
            }}
        }
    }
}
mod shapes {
    pub fn fill_rect(fb: &mut super::Surface, x: i32, y: i32, w: i32, h: i32, c: super::Color) {
        for py in y..y+h { for px in x..x+w {
            if px >= 0 && py >= 0 && (px as usize) < fb.width && (py as usize) < fb.height {
                fb.pixels[py as usize*fb.width+px as usize]=c;
            }
        }}
    }
}
mod bitmap_font { pub fn draw_text(_: &mut super::Surface, _: &[u8], _: usize, _: usize, _: super::Color) {} }
struct Theme;
impl Theme { fn dark() -> Self { Self } }
struct MenuBar;
impl MenuBar { fn draw(&self, _: &mut Surface, _: &Theme) {} }
struct Picker;
impl Picker { fn draw(&self, _: &mut Surface, _: &Theme) {} }
struct Canvas;
impl Canvas {
    fn preview(&self, p: &[(i32,i32)], f: impl FnOnce(&mut Self, &[(i32,i32)])) -> Self { let mut c=Self; f(&mut c,p); c }
    fn render_runs(&self, _: impl FnMut(usize,usize,usize,[u8;3])) {}
}
struct History;
impl History { fn canvas(&self) -> &Canvas { &Canvas } }
struct App {
    tool: usize, size: usize, hue: u16, recent: [[u8;3];8], stroke: Option<Vec<(i32,i32)>>,
    history: History, message: String, picker: Option<Picker>, confirm: bool,
    palette: Option<(u16,Surface)>,
}
impl App { fn draw_stroke(&self, _: &mut Canvas, _: &[(i32,i32)]) {} }
const LABELS: [&[u8];7]=[b"Pen",b"Brsh",b"Line",b"Rect",b"Circ",b"Fill",b"Ers"];
const TOP: i32=176;
mod node { MAPPING }
RENDER
#[test] fn picker_pixels_and_reuse() {
    let mut app=App { tool:0,size:0,hue:0,recent:[[0;3];8],stroke:None,history:History,
        message:String::new(),picker:None,confirm:false,palette:None };
    assert!(app.palette.is_none());
    let mut fb=Surface::new(800,600,libgfx::surface::Format::BGRX).unwrap();
    for hue in [0,120,241] {
        app.hue=hue;
        render(&mut fb,&mut app,&MenuBar);
        fb.wait().unwrap();
        // Independent integer HSV formula, checking every bar and square pixel.
        let rgb=|h:u16,s:u32,v:u32| {
            let h=u32::from(h%360); let sector=h/60; let f=(h%60)*6;
            let p=v*(255-s)/255; let q=v*(255-s*f/360)/255; let t=v*(255-s*(360-f)/360)/255;
            let c=match sector { 0=>[v,t,p],1=>[q,v,p],2=>[p,v,t],3=>[p,q,v],4=>[t,p,v],_=>[v,p,q] };
            Color(c.map(|n| n as u8))
        };
        for y in 58..76 { for x in 0..780 { assert_eq!(fb.pixels[y*800+x],rgb((x*360/780) as u16,255,255)); }}
        for y in 0..100 { for x in 0..100 { assert_eq!(fb.pixels[(y+76)*800+x],rgb(hue,(x*255/100) as u32,((99-y)*255/100) as u32)); }}
        let mut saved=Surface::new(800,600,libgfx::surface::Format::BGRX).unwrap();
        libgfx::blit::blit(&mut saved,&fb,0,0);
        PIXELS.with(|n| n.set(0));
        render(&mut fb,&mut app,&MenuBar);
        assert_eq!(fb.pixels,saved.pixels,"cached redraw changed the picture");
        assert_eq!(PIXELS.with(Cell::get),0,"unchanged picker was rasterized again before the input commit");
    }
}
'''.replace('MAPPING', functions).replace('RENDER', item(source, 'fn render(')))


if __name__ == '__main__':
    unittest.main()
