"""Exercise native console DMA reuse before the suite's baseline sample."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from gt710_gr_live_unwind_test import item

class ConsoleBacking(unittest.TestCase):
    def test_readback_reuses_boot_prepared_dma_and_checks_extent(self):
        source=(ROOT/'kernel/src/gpu/mod.rs').read_text()
        function=item(source,'fn native_console_backing(')
        install=item(source,'pub(crate) fn install_native(')
        self.assertLess(install.index('native_console_backing('),install.index('with_core('))
        readback=item(source,'pub(crate) fn native_console_readback(')
        self.assertNotIn('Pages::new(',readback)
        self.assertIn('native_console_backing(surface)',readback)
        harness=r'''
use std::{cell::Cell,sync::{Arc,Mutex,MutexGuard}};
const PAGE:usize=4096;
thread_local! {static ALLOCS:Cell<usize>=const{Cell::new(0)};}
struct Storage(Mutex<Option<Arc<Pages>>>);
impl Storage {fn lock(&self)->MutexGuard<'_,Option<Arc<Pages>>>{self.0.lock().unwrap()}}
static NATIVE_CONSOLE_BACKING:Storage=Storage(Mutex::new(None));
struct Pages(usize);
impl Pages {fn new(n:usize)->Option<Self>{ALLOCS.with(|v|v.set(v.get()+1));Some(Self(n))}fn bytes(&self)->usize{self.0*PAGE}}
mod gk208 {pub struct ConsoleSurface {pub width:usize,pub height:usize}}
// FUNCTION
fn main(){
 let s=gk208::ConsoleSurface{width:1280,height:1024};
 let baseline=native_console_backing(&s).unwrap();
 assert_eq!(baseline.bytes(),1280*4096);
 for _ in 0..10 {let next=native_console_backing(&s).unwrap();assert!(Arc::ptr_eq(&baseline,&next));}
 assert_eq!(ALLOCS.with(Cell::get),1);
 assert!(native_console_backing(&gk208::ConsoleSurface{width:2560,height:1600}).is_err());
 assert_eq!(ALLOCS.with(Cell::get),1);
}
'''.replace('// FUNCTION',function)
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'backing.rs').write_text(harness)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'backing.rs'),'-o',str(p/'backing')],check=True)
            subprocess.run([str(p/'backing')],check=True)
if __name__=='__main__':unittest.main()
