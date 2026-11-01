"""Execute upload CPU loops and CE plans with a 16 MiB traffic/launch budget.

The fixture substitutes page allocation, uaccess and device dispatch; the loops,
band geometry and packet encoder come from production. No device timing is claimed.
"""
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
PRIORITY = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []


class Transfers(unittest.TestCase):
    def run_rust(self, source):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'test.rs').write_text(source.replace('ROOT', str(ROOT)))
            subprocess.run(PRIORITY + ['rustc', '--edition=2021', '-Dwarnings',
                '--crate-name', 'gpu_abi', '--crate-type', 'rlib',
                str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(path / 'libgpu_abi.rlib')], check=True)
            subprocess.run(PRIORITY + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                '--extern', f'gpu_abi={path / "libgpu_abi.rlib"}', str(path / 'test.rs'),
                '-o', str(path / 'test')], check=True)
            subprocess.run([str(path / 'test')], check=True)

    def test_upload_cpu_passes_and_exact_bytes(self):
        core = (ROOT / 'kernel/src/gpu/mod.rs').read_text().split('fn op_upload(', 1)[1]
        stage = core[core.index('    // Stage the clipped source rows'):core.index('    // The backing is the upload')]
        convert = core[core.index('    // Rows into the backing'):core.index('    // Test point 30')]
        self.run_rust(r'''
#![allow(dead_code)] // The fixture supplies both the converting and identity paths.
use std::{cell::{Cell, UnsafeCell}, sync::Arc};
use gpu_abi::{format, Plain};
mod abi {
    pub use gpu_abi::{Upload, kind};
    pub mod upload_source {
        pub use gpu_abi::upload_source::{bytes, fits, B8G8R8A8, R8G8B8A8, R8};
        pub fn channels(source:u32,pixel:&[u8])->[u8;4] {
            crate::touch(bytes(source) as usize); gpu_abi::upload_source::channels(source,pixel)
        }
    }
}
const PAGE:usize=4096;
const UPLOAD_CHUNK:usize=768;
const EFAULT:i64=14;
thread_local! { static TRAFFIC:Cell<usize>=const{Cell::new(0)}; }
fn touch(n:usize) { TRAFFIC.with(|v| v.set(v.get()+n)); }
struct Pages(UnsafeCell<Vec<u8>>);
impl Pages {
    fn new(n:usize)->Self { Self(UnsafeCell::new(vec![0;n])) }
    fn read(&self,at:usize,out:&mut[u8]) { touch(out.len()); unsafe { out.copy_from_slice(&(&*self.0.get())[at..at+out.len()]); } }
    fn maintain(&self,_:usize,_:usize,cpu:bool) {assert!(!cpu);}
    fn write(&self,at:usize,src:&[u8]) { touch(src.len()); unsafe { (&mut *self.0.get())[at..at+src.len()].copy_from_slice(src); } }
    fn try_spans<E>(&self,at:usize,n:usize,mut f:impl FnMut(usize,*mut u8,usize)->Result<(),E>)->Result<(),E> {
        let mut done=0;
        while done<n { let k=(PAGE-(at+done)%PAGE).min(n-done); touch(k);
            f(done,unsafe{(*self.0.get()).as_mut_ptr().add(at+done)},k)?; done+=k; }
        Ok(())
    }
}
struct Staging { pages:Pages }
impl Staging { fn new(_:u64,n:usize)->Result<Self,i64>{Ok(Self{pages:Pages::new(n)})} }
mod syscall { pub mod userptr {
    pub fn read_user_bytes(dst:*mut u8,src:u64,n:usize)->Result<(),u64>{
        super::super::touch(n); unsafe { std::ptr::copy_nonoverlapping(src as *const u8,dst,n); } Ok(())
    }
} }
mod dev {pub fn backend()->u32 {6}}
mod gk208 {pub mod cost {
    pub enum Slot { UserCopy, UploadConvert }
    pub struct Guard;
    pub fn phase(_:Slot)->Option<Guard> {None}
}}
fn upload(source:u32,fmt:u32,cw:u64,ch:u64,stride:u64,x:u64,y:u64,pitch:u64,data:&[u8],backing:&Arc<Pages>)->Result<(),i64>{
    let tgid=1;
    let a=abi::Upload { pixels:data.as_ptr() as u64,source,..abi::Upload::ZERO };
    let bpp=abi::upload_source::bytes(source) as u64; let row=cw*bpp;
STAGE
CONVERT
    Ok(())
}
#[test] fn sixteen_mib_identity_upload_has_at_most_four_api_byte_visits() {
    let n=16<<20; let pixels:Vec<_>=(0..n).map(|i|(i^(i>>11)) as u8).collect();
    let backing=Arc::new(Pages::new(n)); TRAFFIC.with(|v|v.set(0));
    upload(abi::upload_source::B8G8R8A8,format::B8G8R8A8,2048,2048,8192,0,0,8192,&pixels,&backing).unwrap();
    let visits=TRAFFIC.with(Cell::get);
    // Include per-pixel conversion traffic, which is not a Pages method.
    assert!(visits<=4*n,"{visits} API byte visits for {n} bytes");
    assert_eq!(unsafe{&*backing.0.get()},&pixels);
}
#[test] fn clipped_rows_and_conversions_preserve_padding_and_channels() {
    for fmt in [format::B8G8R8A8,format::R8G8B8A8,format::R8] {
        for source in [abi::upload_source::B8G8R8A8,abi::upload_source::R8G8B8A8,abi::upload_source::R8] {
            if !abi::upload_source::fits(source,abi::kind::TEXTURE,fmt) {continue;}
            let (w,h)=(137u64,5u64); let bpp=abi::upload_source::bytes(source) as u64;
            let stride=w*bpp+17; let pitch=1024; let tbpp=format::bytes(fmt) as u64;
            let data:Vec<_>=(0..stride*h).map(|i|(i*13+7) as u8).collect();
            let backing=Arc::new(Pages::new((pitch*8) as usize));
            upload(source,fmt,w,h,stride,3,2,pitch,&data,&backing).unwrap();
            let mut want=vec![0;(pitch*8) as usize];
            for y in 0..h { for x in 0..w {
                let at=(y*stride+x*bpp) as usize;
                let [b,g,r,a]=abi::upload_source::channels(source,&data[at..]);
                let pixel=if fmt==format::R8 {[data[at],0,0,0]}else if fmt==format::R8G8B8A8 {[r,g,b,a]}else{[b,g,r,a]};
                let to=((y+2)*pitch+(x+3)*tbpp) as usize;
                want[to..to+tbpp as usize].copy_from_slice(&pixel[..tbpp as usize]);
            }}
            assert_eq!(unsafe{&*backing.0.get()},&want);
        }
    }
}
'''.replace('STAGE', stage).replace('CONVERT', convert))

    def test_full_image_uses_two_poison_launches_and_byte_mode(self):
        core = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        band = re.search(r'const READBACK_BAND: usize = ([^;]+);', core)[1]
        observe = core.split('fn observe(', 1)[1]
        geometry = observe[observe.index('    let svga = dev::is_svga();'):observe.index('    // Each in-flight SVGA band')]
        self.run_rust(r'''
#![allow(dead_code)]
extern crate alloc;
#[path="ROOT/kernel/src/gpu/gk208/copy.rs"] mod copy;
mod dev {pub fn is_svga()->bool{false} pub fn backend()->u32{6}}
#[allow(unused_imports)] mod abi {pub use gpu_abi::backend;}
const READBACK_BAND:usize=BAND;
fn packets(words:&[u32])->Vec<(u32,Vec<u32>)>{
    let mut out=Vec::new();let mut at=0;
    while at<words.len(){let h=words[at];let n=((h>>16)&0x1fff) as usize;
        out.push(((h&0x1fff)*4,words[at+1..at+1+n].to_vec()));at+=n+1;}
    out
}
#[test] fn full_image_launch_budget() {
    for (width,height) in [(2048,2048),(2560,1600)] {
        let row=width as usize*4;let pitch=width*4;let height=height as usize;
GEOMETRY
        let bands=height.div_ceil(band_rows);
        assert_eq!(bands*2,2,"each poison needs only one full-image CE launch");
        let image=copy::Image {address:0x100000000,width,height:height as u32,texel_bytes:4,pitch,
            block_height:4,bytes:row as u64*height as u64};
        let linear=copy::Linear {address:0x200000000,offset:0,stride:pitch,bytes:image.bytes};
        for plan in [copy::upload(image,linear,[0,0,width,height as u32]).unwrap(),
            copy::readback(image,linear,[0,0,width,height as u32]).unwrap()] {
            let p=packets(&copy::encode_plan(plan,0x800000,0x12345678abcdef01,77).unwrap());
            assert_eq!(p.iter().filter(|(m,_)|*m==0x400).count(),1);
            assert!(!p.iter().any(|(m,_)|*m==0x708),"identity full rows need no REMAP");
            assert_eq!(p.iter().find(|(m,_)|*m==0x400).unwrap().1[6],pitch);
            let tile=p.iter().find(|(m,_)|*m==0x70c||*m==0x728).unwrap();
            assert_eq!(tile.1[1],pitch);assert_eq!(tile.1[5],0);
            let launches:Vec<_>=p.iter().filter(|(m,_)|*m==0x300).map(|(_,v)|v[0]).collect();
            assert_eq!(launches.len(),4);assert_eq!(launches[0]&0x400,0);
            assert_eq!(&launches[1..],&[0x0c;3]);
            let releases:Vec<_>=p.iter().filter(|(m,_)|*m==0x240).map(|(_,v)|v.clone()).collect();
            assert_eq!(releases,vec![vec![0,0x800004,0xabcdef01],vec![0,0x800008,0x12345678],vec![0,0x800000,77]]);
        }
    }
}
'''.replace('=BAND;', '=' + band + ';').replace('GEOMETRY', geometry))

    def test_cost_fields_cover_transfer_phases(self):
        cost = (ROOT / 'kernel/src/gpu/gk208/cost.rs').read_text()
        abi = (ROOT / 'libs/gpu-abi/src/heap_trace.rs').read_text()
        scorer = (ROOT / 'scripts/metal/gt710-suite').read_text().split('heap_fields = {', 1)[1].split('}', 1)[0]
        for name in ['user_copy', 'read_prepare', 'upload_convert', 'poison',
                     'transfer_map', 'ce', 'compare', 'copy_out']:
            for suffix in ['_n', '_ns']:
                for source in [cost, abi, scorer]:
                    self.assertIn(name + suffix, source)


if __name__ == '__main__':
    unittest.main()
