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
if __name__=='__main__':unittest.main()
