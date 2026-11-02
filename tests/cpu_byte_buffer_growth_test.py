"""Exercise production byte staging growth with its CpuBox allocation contract."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ByteGrowth(unittest.TestCase):
    def test_subpage_growth_uses_the_owned_page_and_preserves_failure(self):
        source = r'''
extern crate alloc;
use std::sync::atomic::{AtomicBool,AtomicUsize,Ordering};
static ALLOCS:AtomicUsize=AtomicUsize::new(0);static FAIL:AtomicBool=AtomicBool::new(false);
mod gpu{
 pub mod dma{
  pub const PAGE:usize=4096;
  pub struct CpuBox(Vec<u8>);
  impl CpuBox{
   pub fn try_new(n:usize)->Option<Self>{crate::ALLOCS.fetch_add(1,crate::Ordering::Relaxed);if crate::FAIL.load(crate::Ordering::Relaxed){None}else{Some(Self(vec![0;n]))}}
   pub fn len(&self)->usize{self.0.len()}
   pub fn as_ref(&self)->&[u8]{&self.0}
   pub fn as_mut(&mut self)->&mut[u8]{&mut self.0}
  }
 }
 #[path="BYTES"]pub mod byte_buffer;
}
use gpu::byte_buffer::Bytes;
fn main(){
 let mut b=Bytes::new(0).unwrap();assert_eq!(b.len(),0);
 for size in[1,256,512,768,1024,1536,2048,3072,4096]{
  let old=b.len();b.resize(size).unwrap();assert!(b[old..].iter().all(|v|*v==0));b[old..].fill(0xa5);
 }
 assert_eq!(b.len(),gpu::dma::PAGE);assert!(b.iter().all(|v|*v==0xa5));
 assert_eq!(ALLOCS.load(Ordering::Relaxed),1,"subpage growth remaps CPU staging instead of using its owned physical page");
 b.resize(8192).unwrap();assert_eq!(ALLOCS.load(Ordering::Relaxed),2);assert!(b[..4096].iter().all(|v|*v==0xa5));assert!(b[4096..].iter().all(|v|*v==0));
 b.remove_prefix(1024);assert_eq!(b.len(),7168);assert!(b[..3072].iter().all(|v|*v==0xa5));
 FAIL.store(true,Ordering::Relaxed);assert!(b.resize(16384).is_err());assert_eq!(b.len(),7168);assert!(b[..3072].iter().all(|v|*v==0xa5));FAIL.store(false,Ordering::Relaxed);
 b.resize(16384).unwrap();assert!(b[..3072].iter().all(|v|*v==0xa5));assert!(b[3072..].iter().all(|v|*v==0));
 b.resize(1).unwrap();b.resize(16384).unwrap();assert!(b[1..].iter().all(|v|*v==0));
 assert!(b.resize(usize::MAX).is_err());assert_eq!(b.len(),16384);
}
'''.replace('BYTES', str(ROOT / 'kernel/src/gpu/byte_buffer.rs'))
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='gt710-byte-growth-') as directory:
            p = Path(directory)
            (p / 'check.rs').write_text(source)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(p / 'check.rs'), '-o', str(p / 'check')], check=True)
            subprocess.run([str(p / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
