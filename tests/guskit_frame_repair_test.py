"""Exercise production rendering/damage across a join's document-size transition.

The recorder models GPU pixels and the kernel's ACQUIRE replay (already copied
before return). Production render, content_damage and present_frame run unchanged.
"""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from guskit_layout_test import ROOT, source
from gt710_gr_live_unwind_test import item


class FrameRepair(unittest.TestCase):
    def test_join_front_and_composed_pixels_agree_after_replay_and_raise(self):
        app = source('userspace/programs/src/guskit.rs')
        code = (ROOT / 'tests/fixtures/guskit_layout.rs').read_text().split('fn sample(')[0]
        code = code.replace('mod breengel { pub mod tree { pub use libbui::tree::*; } }', '')
        start = code.index('#[derive(Clone)] struct Canvas;')
        end = code.index('struct Model ', start)
        code = code[:start] + ('#[path="' + str(ROOT / 'userspace/programs/src/guskit/canvas.rs')
                                 + '"] mod canvas;\nuse canvas::Canvas;\n') + code[end:]
        code = code.replace('width:usize,height:usize,tool:', 'image:Canvas,width:usize,height:usize,tool:')
        code = code.replace('fn canvas(&self)->&Canvas {&Canvas}', 'fn canvas(&self)->&Canvas {&self.image}')
        start = code.index('struct Window ')
        end = code.index('mod node {', start)
        code = code[:start] + code[end:]
        code = code.replace('// NODE_MODULE', source('userspace/programs/src/guskit/node.rs'))
        code = code.replace('// LAYOUT_MODULE', 'mod dimensions {\n' + source('userspace/programs/src/guskit/dimensions.rs')
                            + '\n}\nmod layout {\n' + source('userspace/programs/src/guskit/layout.rs') + '\n}\nuse layout::Layout;')
        code = code.replace('// APP_LAYOUT_FIELD', 'layout:Layout,damage:Option<Clip>,')
        code = code.replace('// APP_METHODS', '\n'.join(item(app, f'    fn {name}(')
                            for name in ['stroke_bounds', 'content_damage']))
        code = code.replace('// CONSTANTS', '\n'.join(line for line in app.splitlines()
                            if line.startswith(('const NAMES:', 'const LABELS:'))))
        code = code.replace('// PRODUCTION', '\n'.join(item(app, f'fn {name}(')
                            for name in ['union', 'draw_label', 'render', 'present_frame']))
        # Match real GPU initialization and clipping, including clear under replay.
        code = code.replace('pixels:vec![Color::WHITE;', 'pixels:vec![Color::rgb(0,0,0);')
        code = code.replace('fn clear(&mut self,c:Color) {self.pixels.fill(c)}', '''fn clear(&mut self,c:Color) {
            for y in 0..self.height {for x in 0..self.width {self.put_pixel(x,y,c)}}
        }''')
        code = code.replace('if x<self.width && y<self.height {', '''if x<self.width && y<self.height
            && self.clips.iter().all(|r| (x as i32)>=r.x && (y as i32)>=r.y
                && (x as i32)<r.x+r.w && (y as i32)<r.y+r.h) {''')
        code = code.replace('f.pixels[py as usize*f.width+px as usize]=c', 'f.put_pixel(px as usize,py as usize,c)')
        code += (ROOT / 'tests/fixtures/guskit_frame_repair.rs').read_text()
        with tempfile.TemporaryDirectory(prefix='guskit-frame-repair-') as tmp:
            path = Path(tmp)
            (path / 'src').mkdir()
            (path / 'src/lib.rs').write_text(code)
            deps = '\n'.join(f'{name} = {{ path = "{ROOT / "libs" / name}" }}'
                             for name in ['libbui', 'libgfx', 'libbreenix', 'libpixcheck'])
            (path / 'Cargo.toml').write_text('[package]\nname="guskit-frame-repair-check"\nversion="0.0.0"\nedition="2021"\n[workspace]\n[dependencies]\n'
                                           + deps + '\nserde_json="=1.0.154"\n')
            env = dict(os.environ, CARGO_BUILD_JOBS='4', RUSTFLAGS='-Dwarnings',
                       CARGO_TARGET_DIR=str(ROOT / 'tools/ui-host-tests/target'))
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            result = subprocess.run(priority + ['cargo', 'test', '--offline', '--manifest-path', str(path / 'Cargo.toml'), '--lib', '--', '--nocapture'], env=env)
            self.assertEqual(result.returncode, 0)


if __name__ == '__main__':
    unittest.main()
