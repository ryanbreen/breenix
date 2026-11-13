"""Count production Gus Kit redraw work for the serialized collaboration brush input.

The GPU recorder honors Surface clips and counts destination pixel writes. The
allocator counts 200x120 RGB buffers, so a whole-canvas preview copy is observable.
GUSKIT_LAYOUT_BASELINE can run this same check against an earlier revision.
"""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item
from guskit_layout_test import ROOT, source


class MotionWork(unittest.TestCase):
    def test_each_brush_motion_is_bounded_by_its_damage(self):
        app = source('userspace/programs/src/guskit.rs')
        code = (ROOT / 'tests/fixtures/guskit_layout.rs').read_text().split('fn sample(')[0]
        start = code.index('#[derive(Clone)] struct Canvas;')
        end = code.index('struct Model', start)
        code = code[:start] + 'use canvas::{Canvas, Point, Radius};\nmod canvas {\n' + source('userspace/programs/src/guskit/canvas.rs') + '\n}\n' + code[end:]
        code = code.replace('type Point=(i32,i32);', '')
        code = code.replace('mod breengel { pub mod tree { pub use libbui::tree::*; } }', '')
        code = code.replace('fn canvas(&self)->&Canvas {&Canvas}', 'fn canvas(&self)->&Canvas {&self.image}')
        code = code.replace('fn draw_stroke(&self,_:&mut Canvas,_:&[Point]) {}', item(app, '    fn draw_stroke('))
        code = code.replace('// APP_LAYOUT_FIELD', 'layout: Layout, image: Canvas, damage: Option<Clip>,')
        code = code.replace('// APP_METHODS', '\n'.join(item(app, f'    fn {name}(') for name in ['stroke_bounds', 'content_damage'] if f'fn {name}(' in app))
        code = code.replace('// LAYOUT_MODULE', 'mod dimensions {\n' + source('userspace/programs/src/guskit/dimensions.rs') + '\n}\n' + 'mod layout {\n' + source('userspace/programs/src/guskit/layout.rs') + '\n}\nuse layout::Layout;')
        code = code.replace('// NODE_MODULE', source('userspace/programs/src/guskit/node.rs'))
        code = code.replace('// CONSTANTS', '\n'.join(line for line in app.splitlines() if line.startswith(('const NAMES:', 'const LABELS:'))))
        code = code.replace('// PRODUCTION', '\n'.join(item(app, f'fn {name}(') for name in ['draw_label', 'render', 'union']))
        code = code.replace('fn clear(&mut self,c:Color) {self.pixels.fill(c)}', '''fn clear(&mut self,c:Color) {
            for y in 0..self.height { for x in 0..self.width { self.put_pixel(x,y,c); } }
        }''')
        code = code.replace('self.points.push((x,y));if x<self.width && y<self.height {self.pixels[y*self.width+x]=c}', '''if x<self.width && y<self.height && self.clips.iter().all(|r| x as i32>=r.x && y as i32>=r.y && (x as i32)<r.x+r.w && (y as i32)<r.y+r.h) {
            WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.pixels[y*self.width+x]=c;
        }''')
        code = code.replace('f.pixels[py as usize*f.width+px as usize]=c', 'f.put_pixel(px as usize,py as usize,c)')
        code += r'''
use std::sync::atomic::{AtomicBool,AtomicUsize,Ordering};
static TEST_LOCK:std::sync::Mutex<()>=std::sync::Mutex::new(());
static COUNT:AtomicBool=AtomicBool::new(false);
static CANVAS_ALLOCS:AtomicUsize=AtomicUsize::new(0);
static WRITES:AtomicUsize=AtomicUsize::new(0);
struct CountingAllocator;
unsafe impl std::alloc::GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self,l:std::alloc::Layout)->*mut u8 {
        if COUNT.load(Ordering::Relaxed) && l.size()==200*120*3 { CANVAS_ALLOCS.fetch_add(1,Ordering::Relaxed); }
        std::alloc::System.alloc(l)
    }
    unsafe fn dealloc(&self,p:*mut u8,l:std::alloc::Layout) { std::alloc::System.dealloc(p,l) }
}
#[global_allocator] static ALLOCATOR:CountingAllocator=CountingAllocator;
#[test] fn brush_motions() {
    let _guard=TEST_LOCK.lock().unwrap();
    for scale in [1,2] {
        let mut app=App {width:200,height:120,tool:1,size:0,hue:0,sat:0,val:0,recent:[[0;3];8],stroke:None,selected_color:None,
            palette:None,canvas_surface:None,picker:None,confirm:false,session:None,session_dialog:None,message:String::new(),
            file:None,directory:PathBuf::from("."),save_as:false,layout:Layout::new(scale),image:Canvas::new(200,120),damage:None};
        let mut fb=Surface::new(800*scale as usize,600*scale as usize,libgfx::surface::Format::BGRX).unwrap();fb.scale=scale;
        render(&mut fb,&mut app,&MenuBar,&mut None,14.0);
        let original=fb.pixels.clone();
        app.stroke=Some(vec![(10,40)]);
        // This is the damage supplied for each actual growing brush prefix. The
        // renderer must respect it even when the surrounding window is larger.
        let mut total=0;
        for motion in 1..=59 {
            app.stroke.as_mut().unwrap().push((10+motion,40));
            app.damage=Some(Clip::new(8*scale as i32,app.layout.canvas_y+38*scale as i32,(motion+5)*scale as i32,5*scale as i32));
            WRITES.store(0,Ordering::Relaxed); CANVAS_ALLOCS.store(0,Ordering::Relaxed); COUNT.store(true,Ordering::Relaxed);
            render(&mut fb,&mut app,&MenuBar,&mut None,14.0);
            COUNT.store(false,Ordering::Relaxed);
            let writes=WRITES.load(Ordering::Relaxed);let copies=CANVAS_ALLOCS.load(Ordering::Relaxed);
            println!("scale={scale} motion={motion} pixel_writes={writes} full_canvas_allocations={copies}");
            assert!(writes<=3*(motion as usize+5)*5*(scale*scale) as usize && copies==0,
                "motion {motion}: {writes} pixel writes, {copies} full canvas allocations; work must follow the brush bounds");
            total+=writes;
        }
        let mut expected=app.image.clone();expected.brush(app.stroke.as_ref().unwrap(),Radius::Small,[0;3]);
        for y in 0..fb.height {for x in 0..fb.width {
            let want=if x<200*scale as usize && y>=app.layout.canvas_y as usize && y<app.layout.canvas_y as usize+120*scale as usize {
                let i=(((y-app.layout.canvas_y as usize)/scale as usize)*200+x/scale as usize)*3;
                Color::rgb(expected.pixels()[i],expected.pixels()[i+1],expected.pixels()[i+2])
            } else {original[y*fb.width+x]};
            assert_eq!(fb.pixels[y*fb.width+x],want,"scale={scale} pixel={x},{y}");
        }}
        // Full redraw after cancellation restores the committed image, including
        // every pixel covered by the temporary preview.
        app.stroke=None;app.damage=None;render(&mut fb,&mut app,&MenuBar,&mut None,14.0);
        assert_eq!(fb.pixels,original,"cancelled preview must leave no pixels behind");
        println!("scale={scale} motions=59 total_pixel_writes={total}");
    }
}
'''
        code += r'''
#[test] fn completed_stroke_apply_and_undo_pixels() {
    let _guard=TEST_LOCK.lock().unwrap();
    let mut app=App {width:1148,height:454,tool:1,size:0,hue:0,sat:0,val:0,recent:[[0;3];8],stroke:None,selected_color:None,
        palette:None,canvas_surface:None,picker:None,confirm:false,session:None,session_dialog:None,message:String::new(),
        file:None,directory:PathBuf::from("."),save_as:false,layout:Layout::new(2),image:Canvas::new(1148,454),damage:None};
    let mut fb=Surface::new(2296,1260,libgfx::surface::Format::BGRX).unwrap();fb.scale=2;
    render(&mut fb,&mut app,&MenuBar,&mut None,14.0);
    let original=fb.pixels.clone();
    for undo in [false,true] {
        if undo {app.image=Canvas::new(1148,454)} else {app.image.pencil(&[(10,80),(70,80)],[40,204,40])}
        EVENT_DAMAGE
        WRITES.store(0,Ordering::Relaxed);
        render(&mut fb,&mut app,&MenuBar,&mut None,14.0);
        let writes=WRITES.load(Ordering::Relaxed);
        println!("native completed gesture undo={undo} pixel_writes={writes} full_window_pixels={}",2296*1260);
        assert!(writes<=3*800_000,"completed gesture still draws the whole window: {writes} writes");
        for y in 0..fb.height {for x in 0..fb.width {
            let want=if y>=352 {
                let i=(((y-352)/2)*1148+x/2)*3;
                Color::rgb(app.image.pixels()[i],app.image.pixels()[i+1],app.image.pixels()[i+2])
            } else {original[y*fb.width+x]};
            assert_eq!(fb.pixels[y*fb.width+x],want,"undo={undo} pixel={x},{y}");
        }}
    }
}
'''.replace('EVENT_DAMAGE', 'app.damage=Some(app.content_damage());' if 'fn content_damage(' in app else 'app.damage=None;')
        with tempfile.TemporaryDirectory(prefix='guskit-motion-') as tmp:
            path = Path(tmp)
            (path / 'src').mkdir()
            (path / 'src/lib.rs').write_text(code)
            (path / 'Cargo.toml').write_text(f'''[package]
name="guskit-motion-check"
version="0.0.0"
edition="2021"
[workspace]
[dependencies]
libbui={{path="{ROOT / 'libs/libbui'}"}}
libgfx={{path="{ROOT / 'libs/libgfx'}"}}
libpixcheck={{path="{ROOT / 'libs/libpixcheck'}"}}
libbreenix={{path="{ROOT / 'libs/libbreenix'}"}}
serde_json="=1.0.154"
''')
            env = dict(os.environ, CARGO_BUILD_JOBS='4', RUSTFLAGS='-Dwarnings',
                       CARGO_TARGET_DIR=str(ROOT / 'tools/ui-host-tests/target'))
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            result = subprocess.run(priority + ['cargo', 'test', '--offline', '--manifest-path', str(path / 'Cargo.toml'), '--lib', '--', '--nocapture'], env=env)
            self.assertEqual(result.returncode, 0)


if __name__ == '__main__':
    unittest.main()
