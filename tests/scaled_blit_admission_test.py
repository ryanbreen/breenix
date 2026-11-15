"""Admit Gus Kit's startup canvases through Surface, BGL and the GPU validator.

Only GPU syscalls are replaced. Accepted draws also prove exact scaled source
coordinates and once-only coverage, including a clip cutting through a pixel.
"""
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ScaledBlit(unittest.TestCase):
    def test_startup_canvases_and_clipped_damage(self):
        with tempfile.TemporaryDirectory(prefix='scaled-blit-') as directory:
            path = Path(directory)
            for name in ('libgfx', 'bgl', 'libbreenix', 'app'):
                (path / name).mkdir()
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
breengel-abi={{path="{ROOT}/libs/breengel-abi"}}
[features]
suite=[]
''')
            boundary = (ROOT / 'tests/fixtures/blauncher_gpu_admission.rs').read_text()
            boundary = boundary.replace('Ok(_) => {', 'Ok(_) => {\n crate::record(words);')
            (path / 'libbreenix/host.rs').write_text(boundary)
            (path / 'libbreenix/lib.rs').write_text(f'''
#[path="{ROOT}/libs/libbreenix/src/gpu.rs"] pub mod gpu;
#[path="{ROOT}/libs/libbreenix/src/errno.rs"] pub mod errno;
mod host;
pub mod window {{pub use breengel_abi as abi;}}
pub use host::SCALE;
thread_local! {{pub static DRAWS:std::cell::RefCell<Vec<([i32;4],[i32;16])>>=const {{std::cell::RefCell::new(Vec::new())}};}}
fn record(words:&[u32]) {{
    let mut at=0;
    while at<words.len() {{
        if words[at]==gpu_abi::draw::COMMAND {{
            assert_eq!(words[at+2],gpu_abi::pipeline::COPY_SCALED);
            for n in 0..words[at+9] as usize {{
                let i=at+gpu_abi::draw::HEADER+n*gpu_abi::draw::INSTANCE;
                DRAWS.with(|d|d.borrow_mut().push((std::array::from_fn(|k|words[i+k] as i32),std::array::from_fn(|k|words[i+4+k] as i32))));
            }}
        }}
        at+=words[at+1] as usize;
    }}
}}
pub mod image {{pub fn read_only(_: &[u8])->bool {{true}} pub fn static_rows(_: &[&[u8]])->bool {{true}}}}
pub mod error {{#[derive(Debug)] pub enum Error {{Os(u8)}}}}
pub mod process {{
    pub struct Pid; impl Pid {{pub fn raw(&self)->u64 {{1}}}}
    pub fn getpid()->Result<Pid,super::error::Error> {{Ok(Pid)}}
    pub fn yield_now()->Result<(),super::error::Error> {{Ok(())}}
}}
mod syscall {{pub mod raw {{pub unsafe fn syscall3(n:u64,op:u64,p:u64,_len:u64)->u64 {{
    assert_eq!(n,416);crate::host::dispatch(op as u32,p) as u64
}}}}}}
''')
            (path / 'app/Cargo.toml').write_text('''[package]
name="scaled-blit-check"
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
            (path / 'app/main.rs').write_text(f'''
use libgfx::{{surface::{{Surface,Format,Rect}},blit::blit_scaled}};
// The app's actual work-area sizing, not a duplicated expected size.
#[path="{ROOT}/userspace/programs/src/guskit/dimensions.rs"] mod dimensions;
fn main() {{
    for scale in [2,1] {{
        libbreenix::SCALE.store(scale,std::sync::atomic::Ordering::Relaxed);
        let (w,h)=dimensions::initial_window(2560,1600,scale);
        let top=dimensions::CANVAS_TOP as u32*scale;
        let size=(w/scale,(h-top)/scale);
        assert!(size.0 as usize<=dimensions::MAX_WIDTH && size.1 as usize<=dimensions::MAX_HEIGHT);
        // First frame: default desktop and --join (before its first session poll),
        // then the explicit 200x120 host document after synchronization.
        for (sw,sh) in [size,(200,120)] {{
            let src=Surface::new(sw as usize,sh as usize,Format::BGRX).unwrap();
            for clipped in [false,true] {{
                let mut dst=Surface::new(w as usize,h as usize,Format::BGRX).unwrap();
                let (x,y)=if clipped {{(-3,top as i32-1)}} else {{(0,top as i32)}};
                let clip=if clipped {{Rect::new(5,top as i32+3,w as i32-14,h as i32-top as i32-11)}} else {{Rect::new(0,0,w as i32,h as i32)}};
                dst.push_clip(clip);
                libbreenix::DRAWS.with(|d|d.borrow_mut().clear());
                blit_scaled(&mut dst,&src,x,y,scale as i32);
                println!("scale={{scale}} canvas={{sw}}x{{sh}} clipped={{clipped}} error={{:?}}",dst.error());
                assert_eq!(dst.error(),None,"scaled startup must be admitted without changing the GPU work cap");
                let mut coverage=vec![0u8;(w*h) as usize];
                libbreenix::DRAWS.with(|d|for (r,a) in d.borrow().iter() {{
                    for py in r[1]..r[3] {{for px in r[0]..r[2] {{
                        assert_eq!(a[2]+(px-a[0])/a[4],(px-x)/scale as i32);
                        assert_eq!(a[3]+(py-a[1])/a[4],(py-y)/scale as i32);
                        coverage[(py as u32*w+px as u32) as usize]+=1;
                    }}}}
                }});
                for py in 0..h as i32 {{for px in 0..w as i32 {{
                    let visible=px>=x && py>=y && px<x+(sw*scale) as i32 && py<y+(sh*scale) as i32
                        && px>=clip.x && py>=clip.y && px<clip.x+clip.w && py<clip.y+clip.h;
                    assert_eq!(coverage[(py as u32*w+px as u32) as usize],u8::from(visible),"coverage at {{px}},{{py}}");
                }}}}
            }}
        }}
    }}
}}
''')
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['cargo', 'run', '--offline', '--quiet', '--manifest-path',
                           str(path / 'app/Cargo.toml'), '--bin', 'check'], check=True,
                           env=dict(os.environ, CARGO_BUILD_JOBS='4', RUSTFLAGS='-Dwarnings'))


if __name__ == '__main__':
    unittest.main()
