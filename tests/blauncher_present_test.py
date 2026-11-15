"""Run the launcher's scaled background paint through the production GPU validator.

Only the GPU syscall boundary is substituted; layout, font metrics, Surface,
BGL encoding, shape geometry and stream validation execute production code.
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


class LauncherPresent(unittest.TestCase):
    def test_scaled_first_frame_is_admitted(self):
        with tempfile.TemporaryDirectory(prefix='blauncher-present-') as directory:
            path = Path(directory)
            for name in ('libgfx', 'bgl', 'libbreenix', 'app'):
                (path / name).mkdir()
            # Build the unchanged libraries against a host GPU syscall boundary.
            for name in ('libgfx', 'bgl'):
                manifest = (ROOT / f'libs/{name}/Cargo.toml').read_text()
                def dependency(match):
                    dep = match[1]
                    target = path / dep if dep in ('libbreenix', 'bgl') else ROOT / 'libs' / dep
                    return f'path = "{target}"'
                manifest = re.sub(r'path = "../([^\"]+)"', dependency, manifest)
                library = f'path = "{ROOT}/libs/{name}/src/lib.rs"'
                manifest = (manifest.replace('[lib]', f'[lib]\n{library}') if '[lib]' in manifest
                            else manifest + f'\n[lib]\n{library}\n')
                (path / name / 'Cargo.toml').write_text(manifest)
            (path / 'libbreenix/Cargo.toml').write_text(f'''[package]
name="libbreenix"
version="0.1.0"
edition="2021"
[workspace]
[lib]
path="lib.rs"
[dependencies]
gpu-abi={{path="{ROOT}/libs/gpu-abi",features=["alloc"]}}
[features]
suite=[]
''')
            (path / 'libbreenix/lib.rs').write_text(f'''
#[path="{ROOT}/libs/libbreenix/src/gpu.rs"] pub mod gpu;
#[path="{ROOT}/libs/libbreenix/src/errno.rs"] pub mod errno;
#[path="{ROOT}/tests/fixtures/blauncher_gpu_admission.rs"] mod host;
pub use host::SCALE;
pub mod image {{
    pub fn read_only(_: &[u8])->bool {{true}}
    pub fn static_rows(_: &[&[u8]])->bool {{true}}
}}
pub mod error {{ #[derive(Debug)] pub enum Error {{ Os(u8) }} }}
pub mod process {{
    pub struct Pid;
    impl Pid {{pub fn raw(&self)->u64 {{1}}}}
    pub fn getpid()->Result<Pid,super::error::Error> {{Ok(Pid)}}
    pub fn yield_now()->Result<(),super::error::Error> {{Ok(())}}
}}
mod syscall {{ pub mod raw {{
    pub unsafe fn syscall3(n:u64,op:u64,p:u64,_len:u64)->u64 {{
        assert_eq!(n,416); crate::host::dispatch(op as u32,p) as u64
    }}
}} }}
''')
            (path / 'app/Cargo.toml').write_text('''[package]
name="blauncher-present-check"
version="0.1.0"
edition="2021"
[workspace]
[[bin]]
name="check"
path="main.rs"
[dependencies]
libgfx={path="../libgfx"}
libbreenix={path="../libbreenix"}
''')
            app = (ROOT / 'userspace/programs/src/blauncher.rs').read_text()
            helpers = '\n'.join(item(app, name) for name in (
                'fn abs_f32(', 'fn floor_f32(', 'fn fmod_pos(', 'fn lerp_color(',
                'fn gradient_color(', 'fn border_perimeter_pos(', 'fn color_of(',
                'fn draw_sinuous_border('))
            constants = '\n'.join(re.findall(
                r'^const (?:PANEL_W|PADDING|SEARCH_H|ITEM_H|MAX_VISIBLE|PANEL_BG|'
                r'CORNER_RADIUS|BORDER_WIDTH|INNER_RADIUS|BORDER_SEGMENT):[^\n]+', app, re.M))
            sizes = app[app.index('    let panel_w ='):app.index('    // Create chromeless window')]
            sizes += app[app.index('    let device_w ='):app.index('    if win.width()')]
            render = item(app, 'fn render(')
            background = render[render.index('    // Panel background color'):render.index('    // Search field border')]
            metrics = (ROOT / 'libs/libbui/src/text/metrics.rs').read_text().replace('use super::scaled_font;', '')
            scaled_font = item((ROOT / 'libs/libbui/src/text.rs').read_text(), 'pub fn scaled_font(')
            (path / 'app/main.rs').write_text(f'''
use libgfx::{{ttf_font::{{self,CachedFont}},surface::{{Surface,Rect as Clip}},color::Color,shapes}};
extern crate self as libbui;
pub mod text {{ {scaled_font} {metrics} }}
#[path="{ROOT}/userspace/programs/src/blauncher/layout.rs"] pub mod layout;
use layout::Layout;
{constants}
{helpers}
struct State {{frame_count:u32}}
fn background(fb:&mut Surface,w:usize,h:usize) {{
    let state=State {{frame_count:0}};
    let scale=fb.ui_scale() as i32;
    let px=|logical| logical*scale;
    {background}
}}
fn main() {{
    let font=ttf_font::Font::parse(include_bytes!("{ROOT}/fonts/DejaVuSans.ttf")).unwrap();
    let ttf_font=Some(CachedFont::with_default_cache(font));
    let font_size=14.; let errors: &[()]=&[];
    for ui_scale in [1,2] {{
        libbreenix::SCALE.store(ui_scale,std::sync::atomic::Ordering::Relaxed);
        {sizes}
        let mut fb=Surface::new(device_w as usize,device_h as usize,libgfx::surface::Format::BGRX).unwrap();
        background(&mut fb,device_w as usize,device_h as usize);
        let mapped=fb.error().map(|e|libbreenix::errno::Errno::from_raw(-e));
        assert_eq!(fb.error(),None,"launcher first frame at scale {{ui_scale}} ({{device_w}}x{{device_h}}): raw GPU error {{:?}}, Window maps it to {{mapped:?}}",fb.error());
    }}
}}
''')
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['cargo', 'run', '--offline', '--quiet', '--manifest-path',
                           str(path / 'app/Cargo.toml'), '--bin', 'check'], check=True,
                           env=dict(os.environ, CARGO_BUILD_JOBS='4', RUSTFLAGS='-Dwarnings'))


if __name__ == '__main__':
    unittest.main()
