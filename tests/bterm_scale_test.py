"""Check UI text size selection and bterm's production cell layout on the host."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


def source(path):
    baseline = os.environ.get('BTERM_SCALE_BASELINE')
    if baseline:
        return subprocess.check_output(['git', 'show', f'{baseline}:{path}'], cwd=ROOT, text=True)
    return (ROOT / path).read_text()


class TerminalScale(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.directory = tempfile.TemporaryDirectory(prefix='bterm-scale-')
        cls.addClassCleanup(cls.directory.cleanup)
        cls.path = Path(cls.directory.name)
        cls.priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        cls.compile(['--crate-name=libfont', '--crate-type=rlib',
                     str(ROOT / 'libs/libfont/src/lib.rs'), '-o', str(cls.path / 'libfont.rlib')])

    @classmethod
    def compile(cls, args):
        subprocess.run(cls.priority + ['rustc', '--edition=2021', '-Dwarnings', *args],
                       check=True, env=dict(os.environ, CARGO_BUILD_JOBS='4'))

    def run_rust(self, code):
        path = self.path / self._testMethodName
        path.with_suffix('.rs').write_text(code)
        self.compile(['--extern', f'libfont={self.path / "libfont.rlib"}',
                      str(path.with_suffix('.rs')), '-o', str(path)])
        subprocess.run([str(path)], check=True)

    def test_helper_matches_draw_text_at_scales_one_and_two(self):
        text = source('libs/libbui/src/text.rs')
        self.assertIn('pub fn scaled_font(', text, 'libbui must expose the draw_text size rule')
        code = item(text, 'pub fn scaled_font(') + '\n' + item(text, 'pub fn draw_text(')
        self.run_rust(r'''
#[derive(Clone, Copy)] pub struct Color;
pub struct Theme { use_bitmap_font: bool }
pub struct Surface { scale: u32, size: f32, raster: Option<bitmap_font::RasterHeight> }
impl Surface { fn ui_scale(&self) -> u32 { self.scale } }
mod bitmap_font {
    use super::*;
    #[derive(Clone, Copy, Debug, PartialEq)] pub enum RasterHeight { Size16, Size32 }
    pub struct Metrics { pub char_width: usize }
    pub fn metrics_at(h: RasterHeight) -> Metrics { Metrics { char_width: if h == RasterHeight::Size32 {14} else {7} } }
    pub fn draw_char_at(f: &mut Surface, _: char, _: usize, _: usize, _: Color, h: RasterHeight) { f.raster = Some(h); }
}
mod ttf_font {
    use super::*;
    pub fn draw_text_bytes(f: &mut Surface, _: &mut (), _: &[u8], _: i32, _: i32, size: f32, _: Color) { f.size = size; }
}
fn with_font(f: impl FnOnce(&mut ())) { f(&mut ()); }
CODE
fn main() {
    for scale in [1, 2] {
        let (size, raster) = scaled_font(16.0, scale);
        assert_eq!(size, 16.0 * scale as f32);
        assert_eq!(raster, if scale == 2 {bitmap_font::RasterHeight::Size32} else {bitmap_font::RasterHeight::Size16});
        let mut fb = Surface {scale, size: 0.0, raster: None};
        draw_text(&mut fb, b"M", 0, 0, Color, &Theme {use_bitmap_font: false});
        assert_eq!(fb.size, size);
        draw_text(&mut fb, b"M", 0, 0, Color, &Theme {use_bitmap_font: true});
        assert_eq!(fb.raster, Some(raster));
    }
}
'''.replace('CODE', code))

    def test_terminal_cells_double_at_scale_two(self):
        terminal = source('userspace/programs/src/bterm.rs')
        text = source('libs/libbui/src/text.rs')
        if 'mod cells;' in terminal:
            cells = source('userspace/programs/src/bterm/cells.rs')
            cells = '\n'.join(line for line in cells.splitlines() if not line.startswith('//!'))
            code = 'mod libbui { pub mod text { use crate::bitmap_font;\n' + item(text, 'pub fn scaled_font(') + '\n}}\nmod cells { use crate::libbui;\n' + cells.replace('use libgfx::', 'use crate::') + '\n}\nuse cells::cell_dims;'
        else:
            # Execute the original startup calculation, rather than a model of its defect.
            begin = terminal.index('let (mut cell_w, mut cell_h) =')
            end = terminal.index('// Widgets use logical UI coordinates', begin)
            startup = terminal[begin:end].replace('mut cell_w, mut cell_h', 'cell_w, cell_h')
            code = item(terminal, 'fn ttf_cell_dims(') + '''
const BITMAP_CELL_W: usize = 7;
const BITMAP_CELL_H: usize = 18;
fn cell_dims(mut ttf_font: Option<&mut CachedFont>, font_size: f32, ui_scale: u32) -> (usize, usize) {
    let _ = ui_scale;
''' + startup + '\n(cell_w, cell_h)\n}'
        self.run_rust(r'''
mod ttf_font { pub type CachedFont = libfont::Font; }
mod bitmap_font {
    #[derive(Clone, Copy, PartialEq)] pub enum RasterHeight { Size16, Size32 }
    pub struct Metrics { pub char_width: usize, pub char_height: usize }
    impl Metrics { pub fn line_height(&self) -> usize { self.char_height + self.char_height / 8 } }
    pub fn metrics_at(h: RasterHeight) -> Metrics { let scale = if h == RasterHeight::Size32 {2} else {1}; Metrics {char_width: 7 * scale, char_height: 16 * scale} }
}
use ttf_font::CachedFont;
CODE
fn main() {
    let mut font = CachedFont::parse(include_bytes!("FONT")).unwrap();
    for size in [14.0, 18.0] {
        let one = cell_dims(Some(&mut font), size, 1);
        let two = cell_dims(Some(&mut font), size, 2);
        assert_eq!(two, (2 * one.0, 2 * one.1), "terminal cells must follow UI scale at size {size}");
    }
    for scale in [1, 2] {
        let dims = cell_dims(None, 14.0, scale);
        let raster = if scale == 2 {bitmap_font::RasterHeight::Size32} else {bitmap_font::RasterHeight::Size16};
        let metrics = bitmap_font::metrics_at(raster);
        assert_eq!(dims, (metrics.char_width, metrics.line_height()));
    }
}
'''.replace('CODE', code).replace('FONT', str(ROOT / 'fonts/DejaVuSansMono.ttf')))


if __name__ == '__main__':
    unittest.main()
