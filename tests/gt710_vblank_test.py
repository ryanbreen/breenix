"""Sample the native head's documented 16-bit vblank counter without writes."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item

class Vblank(unittest.TestCase):
    def test_active_head_counter_and_timestamp(self):
        source = (ROOT / 'kernel/src/gpu/gk208/native.rs').read_text()
        method = (item(source, 'pub(in crate::gk208) fn vblank_counter(')
                  if 'pub(in crate::gk208) fn vblank_counter(' in source else
                  'fn vblank_counter(_: &Registers, _:u32)->Result<(u64,u16), &\'static str>{Err("counter unavailable")}')
        method = method.replace('pub(in crate::gk208)', 'pub(crate)')
        code = r'''
use std::cell::RefCell;
mod signal {pub fn monotonic_nanos()->u64{123456789}}
struct Registers {reads:RefCell<Vec<usize>>}
impl Registers {fn read(&self, a:usize)->u32{self.reads.borrow_mut().push(a);match a {0x616340=>0xffff_0123,0x617b40=>0x0002_0456,_=>panic!("foreign register")}}}
METHOD
#[test] fn samples_only_the_selected_heads_read_only_counter(){
 let _fixture_read = Registers::read as fn(&Registers,usize)->u32;
 assert_eq!(signal::monotonic_nanos(),123456789);
 let regs=Registers{reads:RefCell::new(vec![])};
 assert_eq!(vblank_counter(&regs,0),Ok((123456789,65535)));
 assert_eq!(vblank_counter(&regs,3),Ok((123456789,2)));
 assert!(vblank_counter(&regs,4).is_err());
 assert_eq!(*regs.reads.borrow(),[0x616340,0x617b40]);
 assert_eq!(2u16.wrapping_sub(65535),3);
}
'''.replace('METHOD', method)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp); (p/'counter.rs').write_text(code)
            subprocess.run(priority+['rustc','--edition=2021','--test','-Dwarnings',str(p/'counter.rs'),'-o',str(p/'counter')],check=True)
            subprocess.run([str(p/'counter')],check=True)
    def test_proc_samples_survive_suppressed_case_stdout(self):
        source = (ROOT / 'kernel/src/gpu/gk208/mod.rs').read_text()
        method = item(source, 'pub(crate) fn proc_vblank(')
        code = r'''#![allow(dead_code)] // Mocks include the export API before the producer uses it.
use std::cell::RefCell;
thread_local!{static ROWS:RefCell<Vec<String>>=const{RefCell::new(vec![])}}
struct Registers;struct Evo{head:u32}
mod ce{pub fn with_clock_owner<R>(f:impl FnOnce(&mut super::Registers,&mut super::Evo,())->Result<R,&'static str>)->Result<R,&'static str>{f(&mut super::Registers,&mut super::Evo{head:2},())}}
mod native{pub fn vblank_counter(_: &super::Registers,h:u32)->Result<(u64,u16),&'static str>{assert_eq!(h,2);Ok((123456789,65535))}}
mod bulk{pub fn line(_:String)->Result<(),&'static str>{Ok(())}}
mod timing{pub fn vblank_sample(at:u64,count:u16){super::ROWS.with(|r|r.borrow_mut().push(format!("{at} {count}")));}}
fn hash(_: &[u8])->String{"digest".into()}
#[macro_export]macro_rules! serial_println{($($a:tt)*)=>{let _=format!($($a)*);}}
METHOD
#[test]fn retains_read_only_samples_in_the_digest_bound_bulk(){
 assert_eq!(proc_vblank().unwrap(),b"123456789 65535\n");
 ROWS.with(|r|assert_eq!(*r.borrow(),["123456789 65535"]));
}
'''.replace('METHOD', method)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p / 'sample.rs').write_text(code)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings', str(p / 'sample.rs'), '-o', str(p / 'sample')], check=True)
            subprocess.run([str(p / 'sample')], check=True)
if __name__=='__main__':unittest.main()
