"""Run isolation allocation/display setup with the native and virtual catalogues."""
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class IsolationModes(unittest.TestCase):
    def test_isolation_resources_match_native_and_irq_keeps_measured_extent(self):
        source = (ROOT / 'userspace/programs/src/suite_gpu/isolation.rs').read_text()
        performance = (ROOT / 'userspace/programs/src/suite_gpu/performance.rs').read_text()
        helper = item(source, 'fn take_native_display(') if 'fn take_native_display(' in source else ''
        setups = []
        for name in ['display_context_unreachable', 'no_addresses', 'client_cannot_scanout']:
            body = item(source, f'pub fn {name}(')
            allocation = re.search(r'    let t = scanout_target[^;]+;', body).group()
            start = body.find('    let mode = take_native_display()?;')
            setup = body[start:body.index(allocation) + len(allocation)] if start >= 0 else allocation
            setups.append(f'fn {name}() -> R<()> {{ {setup} assert_target(t, EXPECTED.with(|e| e.get())); Ok(()) }}')
        irq = item(source, 'pub fn irq_masked_bound(')
        start = irq.index('    let (w, h) =')
        setup = irq[start:irq.index('    for i in 0..10u32')]
        child = re.search(r'let own = child\((scanout_target\([^;]+)\)\?;', item(source, 'pub fn client_cannot_scanout(')).group(1)
        code = r'''extern crate gpu_abi;
use gpu_abi::{self as abi, Mode, Modes, Plain};
use std::cell::{Cell,RefCell};
type R<T> = Result<T,String>;
fn fail_msg(s:String)->String{s}
fn errno(r:i64)->String{r.to_string()}
fn ensure(ok:bool,msg:impl FnOnce()->String)->R<()>{if ok{Ok(())}else{Err(msg())}}
mod format{pub const B8G8R8A8:u32=1;}
thread_local! {
 static MODES:RefCell<Modes>=const{RefCell::new(Modes::ZERO)};
 static SELECTED:Cell<Option<u32>>=const{Cell::new(None)};
 static EXPECTED:Cell<(u32,u32)>=const{Cell::new((0,0))};
 static TARGETS:RefCell<Vec<(u32,u32)>>=const{RefCell::new(Vec::new())};
}
pub mod graphics{pub fn take_over_display()->Result<(),i64>{Ok(())}}
mod libbreenix{pub use crate::graphics;}
pub mod gpu{
 use super::*;
 pub fn modes()->(i64,Modes){(0,MODES.with(|m|*m.borrow()))}
 pub fn set_mode(i:u32)->i64{SELECTED.with(|s|s.set(Some(i)));0}
 pub fn texture_create(_:u32,w:u32,h:u32)->Result<u64,i64>{assert_eq!((w,h),(2560,1600));Ok(9)}
 pub fn upload(_:u64,_:u64,rect:[u32;4],pixels:&[u8],stride:u32,_:u32,_:u32)->(i64,()){
  assert_eq!(rect,[0,0,2560,1600]);assert_eq!(pixels.len(),2560*1600*4);assert_eq!(stride,10240);(0,())
 }
}
fn scanout_target(_:u32,w:u32,h:u32)->R<u64>{TARGETS.with(|t|{let mut t=t.borrow_mut();t.push((w,h));Ok(t.len() as u64)})}
fn assert_target(t:u64,size:(u32,u32)){TARGETS.with(|v|assert_eq!(v.borrow()[t as usize-1],size));}
// HELPERS
// SETUPS
fn irq()->R<()> {let ctx=1; // IRQ
 assert_eq!((w,h),(2560,1600));assert_target(t,(2560,1600));Ok(())}
fn catalogue(entries:&[(u32,u32,u32)]){
 let mut m=Modes::ZERO;m.count=entries.len() as u32;m.current=1;
 for (slot,&(width,height,scale)) in m.modes.iter_mut().chain(m.extra_modes.iter_mut()).zip(entries){*slot=Mode{width,height,scale,reserved:0};}
 MODES.with(|v|*v.borrow_mut()=m);SELECTED.with(|v|v.set(None));
}
fn main(){
 let which=std::env::args().nth(1).unwrap();
 for (entries,native,native_index,containing) in [
  (vec![(1024,768,1),(1280,1024,1),(2560,2048,2)],(1280,1024),1,2),
  (vec![(1280,800,1),(2560,1600,2)],(2560,1600),1,1),
 ]{
  catalogue(&entries);EXPECTED.with(|e|e.set(native));
  if which!="irq" {
   let case=match which.as_str(){"context"=>display_context_unreachable,"addresses"=>no_addresses,_=>client_cannot_scanout};
   SELECTED.with(|s|s.set(None));case().unwrap();assert_eq!(SELECTED.with(Cell::get),Some(native_index));
  let mode=MODES.with(|m|m.borrow().current_mode().unwrap());
  assert_eq!((mode.width,mode.height),native);let own=CHILD.unwrap();assert_target(own,native);
  } else {SELECTED.with(|s|s.set(None));irq().unwrap();assert_eq!(SELECTED.with(Cell::get),Some(containing));}
 }
 if which=="irq" {catalogue(&[(1280,1024,1)]);assert!(irq().unwrap_err().contains("2560"));}
}
'''.replace('// HELPERS', helper + '\npub mod performance { use super::*; const W:u32=2560; const H:u32=1600; ' + item(performance, 'fn take_display_2560(').replace('fn take_display_2560(', 'pub fn take_display_2560(') + ' }')
        code = code.replace('// SETUPS', '\n'.join(setups)).replace('// IRQ', setup.replace('super::performance::', 'performance::')).replace('CHILD', child)
        priority = ['taskpolicy','-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='isolation-modes-') as directory:
            p = Path(directory)
            (p/'check.rs').write_text(code)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings','--crate-name','gpu_abi','--crate-type','rlib',str(ROOT/'libs/gpu-abi/src/lib.rs'),'-o',str(p/'libgpu_abi.rlib')],check=True)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'check.rs'),'--extern',f'gpu_abi={p}/libgpu_abi.rlib','-o',str(p/'check')],check=True)
            for case in ['context', 'addresses', 'client', 'irq']:
                with self.subTest(case=case):
                    subprocess.run([str(p/'check'), case],check=True)
        self.assertIn('present_view(ctx, t)?', irq)
        self.assertNotIn('gpu::present(ctx, t)', irq)
        self.assertIn('bound <= 500', irq)
        self.assertIn('(0..2_000)', irq)
        self.assertIn('[0, 0, 1280, 800], t, [640, 400]', irq)


if __name__ == '__main__':
    unittest.main()
