"""Count production quad command-storage allocations after frame reservation."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]


class QuadStorage(unittest.TestCase):
    def test_quad_methods_and_relocations_allocate_once_each(self):
        encoder=(ROOT/'kernel/src/gpu/gk208/encode.rs').read_text().replace(
            'include_bytes!("caps/integer.inputs")','include_bytes!("'+str(ROOT/'kernel/src/gpu/gk208/caps/integer.inputs')+'")')
        fixture=r'''
pub fn allocation_check(){
 let t=Target{id:1,address:0x400000,width:256,height:256,format:gpu_abi::format::B8G8R8A8,tile_mode:0x30};
 let source=Target{id:2,address:0x500000,..t};let scratch=Target{id:3,address:0x600000,..t};let data=Target{id:4,address:0x700000,..t};
 for (pipe,src,dst,table)in[
  (gpu_abi::pipeline::SOLID,None,None,None),(gpu_abi::pipeline::LINE,None,None,None),
  (gpu_abi::pipeline::COPY,Some(source),None,None),(gpu_abi::pipeline::OVER,Some(source),Some(scratch),None),
  (gpu_abi::pipeline::COVERAGE_SPANS,None,Some(scratch),Some(data))]{
  let mut f=Frame{probe_mask:0,chunks:Vec::with_capacity(1),data:Vec::with_capacity(32768),removed_prefix:DATA_START};
  crate::ALLOCATIONS.store(0,std::sync::atomic::Ordering::Relaxed);crate::WATCH.store(true,std::sync::atomic::Ordering::Relaxed);
  let result=quads(&mut f,t,[[2,4,18,20]].into_iter(),[0,0,256,256],&[0;32],pipe,src,dst,table);
  crate::WATCH.store(false,std::sync::atomic::Ordering::Relaxed);result.unwrap();
  let count=crate::ALLOCATIONS.load(std::sync::atomic::Ordering::Relaxed);
  assert_eq!(count,2,"pipeline {pipe}: command words and relocations should each allocate once, got {count}");
  assert!(f.chunks[0].words.len()<=256);assert!(f.chunks[0].relocations.len()<=8);
 }
}
'''
        source=r'''
extern crate alloc;
use std::{alloc::{GlobalAlloc,Layout,System},sync::atomic::{AtomicBool,AtomicUsize,Ordering}};
static WATCH:AtomicBool=AtomicBool::new(false);static ALLOCATIONS:AtomicUsize=AtomicUsize::new(0);
struct Counting;unsafe impl GlobalAlloc for Counting{
 unsafe fn alloc(&self,l:Layout)->*mut u8{if WATCH.load(Ordering::Relaxed){ALLOCATIONS.fetch_add(1,Ordering::Relaxed);}System.alloc(l)}
 unsafe fn dealloc(&self,p:*mut u8,l:Layout){System.dealloc(p,l)}
 unsafe fn realloc(&self,p:*mut u8,l:Layout,n:usize)->*mut u8{if WATCH.load(Ordering::Relaxed){ALLOCATIONS.fetch_add(1,Ordering::Relaxed);}System.realloc(p,l,n)}
}
#[global_allocator]static GLOBAL:Counting=Counting;
#[allow(dead_code)]#[path="SHADERS"]pub mod shaders;
#[allow(dead_code)]#[path="CAPS"]pub mod caps;
#[allow(dead_code)]mod encode;
fn main(){encode::allocation_check();}
'''.replace('SHADERS',str(ROOT/'kernel/src/gpu/gk208/gr/shaders.rs')).replace('CAPS',str(ROOT/'kernel/src/gpu/gk208/caps.rs'))
        priority=['taskpolicy','-b']if sys.platform=='darwin'else[]
        with tempfile.TemporaryDirectory(prefix='gt710-quad-storage-')as directory:
            p=Path(directory);(p/'encode.rs').write_text(encoder+fixture);(p/'check.rs').write_text(source)
            subprocess.run(priority+['rustc','--edition=2021','--crate-name','gpu_abi','--crate-type','rlib','--cfg','feature="alloc"','-Dwarnings',str(ROOT/'libs/gpu-abi/src/lib.rs'),'-o',str(p/'libgpu_abi.rlib')],check=True)
            subprocess.run(priority+['rustc','--edition=2021','--extern',f'gpu_abi={p/"libgpu_abi.rlib"}','-Dwarnings',str(p/'check.rs'),'-o',str(p/'check')],check=True)
            subprocess.run([str(p/'check')],check=True)


if __name__=='__main__':
    unittest.main()
