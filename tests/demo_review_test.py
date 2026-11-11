"""Run the demos' actual publication/render/error bodies without a GPU."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / 'userspace/programs/src'


def source(name):
    return (SRC / f'{name}.rs').read_text()


STUBS = '''
#[derive(Clone,Copy)] pub struct Color {pub r:u8,pub g:u8,pub b:u8}
impl Color {pub const BLACK:Self=Self::rgb(0,0,0);pub const WHITE:Self=Self::rgb(255,255,255);pub const fn rgb(r:u8,g:u8,b:u8)->Self{Self{r,g,b}}}
#[derive(Default)] pub struct Surface {pub submissions:usize,batched:bool}
impl Surface {
pub fn clear(&mut self,_c:Color){self.emit();}
pub fn put_pixel(&mut self,_x:usize,_y:usize,_c:Color){self.emit();}
pub fn emit(&mut self){if !self.batched {self.submissions+=1;}}
pub fn batch(&mut self,f:impl FnOnce(&mut Self)){self.batched=true;f(self);self.batched=false;self.submissions+=1;}
}
pub mod libgfx {pub use super::shapes;}
pub mod shapes {use super::*;
pub fn fill_circle(f:&mut Surface,_x:i32,_y:i32,_r:i32,_c:Color){f.emit();}
pub fn fill_rect(f:&mut Surface,_x:i32,_y:i32,_w:i32,_h:i32,_c:Color){f.emit();}
pub fn draw_rect(f:&mut Surface,_x:i32,_y:i32,_w:i32,_h:i32,_c:Color){f.emit();}
pub fn draw_line(f:&mut Surface,_x:i32,_y:i32,_xx:i32,_yy:i32,_c:Color){f.emit();}
}
'''


class Review(unittest.TestCase):
    def execute(self, code, test=True):
        host = subprocess.check_output(['rustc', '-vV'], text=True).split('host: ')[1].splitlines()[0]
        deps = ROOT / 'libs/libuitest/target' / host / 'debug/deps'
        args = []
        for name in ['breengel', 'serde_json']:
            lib = max(deps.glob(f'lib{name}-*.rlib'), key=lambda p: p.stat().st_mtime)
            args += ['--extern', f'{name}={lib}']
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'test.rs').write_text(code)
            result = subprocess.run((['taskpolicy', '-b'] if sys.platform == 'darwin' else []) +
                                    ['rustc', '--edition=2021', '-Dwarnings',
                                     *(['--test'] if test else []), str(path / 'test.rs'),
                                     '-L', f'dependency={deps}', *args, '-o', str(path / 'test')],
                                    capture_output=True, text=True, env=dict(os.environ, CARGO_BUILD_JOBS='4'))
            self.assertEqual(result.returncode, 0, result.stderr)
            return subprocess.run([str(path / 'test')], capture_output=True, text=True)

    def prefix(self, name):
        # Only app event loops and imports are omitted; helpers are the production bodies.
        return '\n'.join(line for line in source(name).split('fn main()')[0].splitlines()
                         if not line.startswith(('//!', 'use ', '#[path')))

    def test_demo_resize_publishes_every_ball_inside(self):
        text = source('demo')
        balls = text[text.index('let mut balls ='):text.index('let mut frame =')]
        publication = text[text.index('let width = win.width() as i32; let height'):text.index('let fb = win.surface()')]
        code = STUBS + f'''
#[path="{SRC}/demos/status.rs"] mod status;
struct Win {{tree:Option<breengel::tree::Tree>,w:u32,h:u32}}
impl Win {{fn width(&self)->u32{{self.w}} fn height(&self)->u32{{self.h}} fn set_tree(&mut self,t:breengel::tree::Tree){{self.tree=Some(t);}}}}
// Helper drawing functions are intentionally unused in this publication-only fixture.
#[allow(dead_code)] mod demo {{use super::*; {self.prefix('demo')}
pub fn snapshot(w:u32,h:u32)->breengel::tree::Tree {{let mut win=Win{{tree:None,w,h}};{balls}balls[0].x=w as i32+20;balls[0].y=h as i32+20;let frame=0;let epochs=0;{publication}win.tree.unwrap()}}
}}
#[test] fn resized_scene() {{for (w,h) in [(400,300),(300,200),(69,50)] {{
let t=demo::snapshot(w,h);assert_eq!(t.children.len(),21,"12 lines, 5 rectangles and all 4 balls must be published");
assert_eq!(t.root.value["inBounds"],true);
for n in t.children {{let [x,y,bw,bh]=n.bounds;assert!(x>=0 && y>=0 && x+bw<=w as i64 && y+bh<=h as i64,"{{:?}}",n.bounds);}}
}}}}
'''
        result = self.execute(code)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_confetti_frame_batches_200_particles(self):
        code = STUBS + f'''
// Simulation helpers are retained to instantiate a real, full particle population.
#[allow(dead_code)] mod confetti {{use super::*; {self.prefix('confetti')}
#[test] fn bounded_submissions() {{let mut s=ConfettiSystem::new(400,300,1);s.spawn_burst(150,100,MAX_PARTICLES);let mut fb=Surface::default();s.render(&mut fb);assert!(fb.submissions<=2,"{{}} submits for 200 particles",fb.submissions);}}
}}
'''
        result = self.execute(code)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_present_error_reports_failure_without_panicking(self):
        for name in ['biconkit', 'confetti', 'demo', 'particles', 'rectangles']:
            with self.subTest(app=name):
                text = source(name)
                # Run the exact terminal present/error block in a separate process.
                tail = text[text.rfind('\n', 0, text.rindex('win.present()')) + 1:]
                tail = tail[:tail.rfind('}')].rstrip()
                tail = tail[:tail.rfind('}')].rstrip()
                code = 'struct Win;impl Win{fn present(&mut self)->Result<(), &str>{Err("latched GPU error")}}\nfn main(){let mut win=Win;\n' + tail + '\n}'
                result = self.execute(code, test=False)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertIn('latched GPU error', result.stderr)
                self.assertNotIn('panicked', result.stderr)


if __name__ == '__main__':
    unittest.main()
