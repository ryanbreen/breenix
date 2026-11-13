"""Execute each app's text/layout calculation at UI scales one and two.

APPS_SCALE_BASELINE reads the pre-fix calculations from that revision, so the
same five checks can demonstrate the reported defect at f18a7974.
"""
from pathlib import Path
import os
import re
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


def source(path):
    revision = os.environ.get('APPS_SCALE_BASELINE')
    if revision:
        return subprocess.check_output(['git', 'show', f'{revision}:{path}'], cwd=ROOT, text=True)
    return (ROOT / path).read_text()


class AppsTextScale(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix='apps-text-scale-')
        cls.addClassCleanup(cls.tmp.cleanup)
        cls.path = Path(cls.tmp.name)
        cls.priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        cls.compile(['--crate-name=libfont', '--crate-type=rlib', str(ROOT / 'libs/libfont/src/lib.rs'),
                     '-o', str(cls.path / 'libfont.rlib')])

    @classmethod
    def compile(cls, args):
        subprocess.run(cls.priority + ['rustc', '--edition=2021', '-Dwarnings', *args], check=True)

    def check_app(self, app, projection):
        app_source = source(f'userspace/programs/src/{app}.rs')
        if 'mod layout;' in app_source:
            metrics = source('libs/libbui/src/text/metrics.rs')
            layout = source(f'userspace/programs/src/{app}/layout.rs')
            helper = item(source('libs/libbui/src/text.rs'), 'pub fn scaled_font(')
            code = 'pub mod text { use crate::{bitmap_font, ttf_font::CachedFont};\n' + helper + '\n' + metrics.replace('use super::scaled_font;', '').replace('use libgfx::{bitmap_font, ttf_font::CachedFont};', '') + '\n}\n'
            code += 'mod layout {\n' + layout + '\n}\n'
            code += '''fn sample(font: &libfont::Font, scale: u32) -> (f32, Vec<i32>) {
let l = layout::Layout::new(Some(font), 14.0, scale, 600 * scale as i32, 540 * scale as i32);
''' + projection + '\n}'
        else:
            # Execute original production expressions (not a replacement scale model).
            constants = '\n'.join(re.findall(r'^const (?:TAB_BAR_H|STATUS_BAR_H|LIST_TOP|ITEM_H|PREVIEW_HEIGHT|LIST_START_Y|ROW_H|FOOTER_H|PADDING|SEARCH_H|PANEL_W|MAX_VISIBLE):[^\n]+', app_source, re.M))
            if app in ('blog', 'bcheck', 'blauncher'):
                size = re.search(r'let font_size = .*?;', app_source).group().replace('win.mono_size()', '14.0_f32').replace('win.display_size()', '14.0_f32')
                if app == 'blog':
                    start = app_source.index('let metrics = font.metrics(font_size);')
                    end = app_source.index('println!("[blog] font loaded', start)
                    calc = app_source[start:end]
                    body = calc + '(font_size, vec![char_w, line_h, TAB_BAR_H * scale as i32, STATUS_BAR_H])'
                elif app == 'bcheck':
                    body = '(font_size, vec![ROW_H, LIST_START_Y, FOOTER_H])'
                else:
                    code = constants + '\n' + item(app_source, 'struct PanelLayout {') + '\n' + item(app_source, 'fn compute_layout(')
                    code += '\nfn sample(_font: &libfont::Font, scale: u32) -> (f32, Vec<i32>) {\n' + size
                    code += '\nlet l=compute_layout(600 * scale as usize,540 * scale as usize,3); let _=(l.y,l.search_x,l.search_y,l.search_w,l.visible_count); (font_size,vec![l.x,l.w,l.h,l.list_y,ITEM_H])\n}'
                    constants = ''
                    body = None
                if body:
                    code = constants + '\nfn sample(font: &libfont::Font, scale: u32) -> (f32, Vec<i32>) {\nlet _=(font,scale);\n' + size + '\n' + body + '\n}'
            elif app == 'bfontpicker':
                draw = re.search(r'bitmap_font::draw_text\(fb, b"System Font Picker".*?;', app_source).group()
                self.assertIn(', 10, 8,', draw)
                # The original draw_text is the 16px raster; list and hit-test share these constants.
                code = constants + '\nfn sample(_: &libfont::Font, _: u32)->(f32,Vec<i32>){ let _=PREVIEW_HEIGHT; (16.0,vec![LIST_TOP as i32,ITEM_H as i32]) }'
            else:
                draw = re.search(r'font::draw_text\(fb,b"ICON LIBRARY".*?;', app_source).group()
                bitmap_scale = int(re.search(r'Color::WHITE,(\d+)\)', draw).group(1))
                code = item(source('userspace/programs/src/biconkit/input.rs'), 'pub fn grid_center(')
                code += f'\nfn sample(_: &libfont::Font,_:u32)->(f32,Vec<i32>){{let (x,y)=grid_center(1); ({7 * bitmap_scale}.0,vec![x,y,30])}}'
            # Baseline extraction retains surrounding layout constants and fields.
            # They remain production inputs outside the extracted subset.
            code = '#[allow(dead_code)] mod original {\n' + code.replace('fn sample(', 'pub fn sample(') + '\n}\nuse original::sample;'
        path = self.path / app
        rust = '''extern crate self as libgfx;
extern crate self as libbui;
pub mod ttf_font { pub type CachedFont = libfont::Font; }
pub mod bitmap_font {
    #[derive(Clone, Copy, Debug)] pub enum RasterHeight { Size16, Size32 }
    pub struct Metrics { pub char_width: usize, pub char_height: usize }
    impl Metrics { pub fn line_height(&self)->usize {self.char_height+self.char_height/8} }
    pub fn metrics_at(r:RasterHeight)->Metrics { let s=match r {RasterHeight::Size16=>1,RasterHeight::Size32=>2}; Metrics {char_width:7*s,char_height:16*s} }
}
mod input { GRID }
CODE
fn main() {
 let font=libfont::Font::parse(include_bytes!("FONT")).unwrap();
 let one=sample(&font,1); let two=sample(&font,2);
 assert_eq!(two.0,2.0*one.0,"APP text size must double");
 assert_eq!(two.1,one.1.iter().map(|x|x*2).collect::<Vec<_>>(),"APP rows and hit rectangles must double");
}
'''.replace('GRID', item(source('userspace/programs/src/biconkit/input.rs'), 'pub fn grid_center(') if app == 'biconkit' and 'mod layout;' in app_source else '').replace('CODE', code).replace('FONT', str(ROOT / 'fonts/DejaVuSansMono.ttf')).replace('APP', app)
        path.with_suffix('.rs').write_text(rust)
        self.compile(['--extern', f'libfont={self.path / "libfont.rlib"}', str(path.with_suffix('.rs')), '-o', str(path)])
        subprocess.run([str(path)], check=True)

    def test_blog(self):
        self.check_app('blog', 'let r=l.row_rect(2); (l.text.size,vec![l.text.cell_w,l.text.line_h,l.content_y,l.status_h,r[0],r[1],r[2],r[3]])')

    def test_bcheck(self):
        self.check_app('bcheck', 'let r=l.scroll_rect(); (l.text.size,vec![l.row_h,l.list_y,l.footer_h,r[0],r[1],r[2],r[3]])')

    def test_bfontpicker(self):
        self.check_app('bfontpicker', 'let r=l.row_rect(2); assert_eq!(l.hit_row(r[0],r[1],0),Some(2)); assert_eq!(l.hit_row(-1,r[1],0),None); assert_eq!(l.hit_row(0,l.list_bottom,0),None); (l.text.size,vec![l.text.cell_w,l.text.line_h,l.list_top,l.row_h,l.preview_y,r[0],r[1],r[2],r[3]])')

    def test_blauncher(self):
        self.check_app('blauncher', 'let l=l.panel(3); let r=l.row_rect(2); assert_eq!(l.hit_row(r[0],r[1]),Some(2)); assert_eq!(l.hit_row(r[0]-1,r[1]),None); (l.text.size,vec![l.x,l.w,l.h,l.list_y,l.item_h,r[0],r[1],r[2],r[3]])')

    def test_biconkit(self):
        self.check_app('biconkit', 'let (x,y)=l.grid_center(1); let r=l.icon_rect(1); assert_eq!(l.logical_point(x,y),input::grid_center(1)); (l.text.size,vec![x,y,l.radius(),r[0],r[1],r[2],r[3]])')


if __name__ == '__main__':
    unittest.main()
