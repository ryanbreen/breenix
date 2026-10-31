"""Count aperture transactions made by the production VM publisher."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]
PRIORITY = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []


class VmBatch(unittest.TestCase):
    def test_table_clear_and_pte_writes_use_bounded_aperture_spans(self):
        channel = (ROOT / 'kernel/src/gpu/gk208/channel.rs').read_text()
        inventory = (ROOT / 'kernel/src/gpu/gk208/inventory.rs').read_text()
        source = r'''
#![allow(dead_code)] // The fixture imports the complete memory and VM builders.
extern crate alloc;
mod gk208 {
    #[path="ROOT/kernel/src/gpu/gk208/memory.rs"] pub mod memory;
    #[path="ROOT/kernel/src/gpu/gk208/vm.rs"] pub mod vm;
    pub mod modeset {
        pub fn bar_flush(r:&mut super::Registers)->Result<(),&'static str>{r.flushes+=1;Ok(())}
    }
    pub mod vm_control {
        pub fn invalidate(r:&mut super::Registers,_:u64)->Result<u32,&'static str>{r.invalidates+=1;Ok(0)}
    }
    #[derive(Default)]
    pub struct Registers {
        pub bank:u32,pub reads:usize,pub spans:usize,pub max_span:usize,
        pub flushes:usize,pub invalidates:usize,pub data:std::collections::BTreeMap<u64,u32>,
    }
    impl Registers {
        pub fn read(&mut self,r:usize)->u32 {
            self.reads+=1;
            if r==0x1700 {self.bank} else {*self.data.get(&((u64::from(self.bank)<<16)+(r-0x700000) as u64)).unwrap_or(&0)}
        }
    }
    impl memory::Access for Registers {
        fn read(&mut self,r:usize)->u32 {self.read(r)}
        fn write(&mut self,r:usize,v:u32) {
            if r==0x1700 {self.bank=v;} else {self.data.insert((u64::from(self.bank)<<16)+(r-0x700000) as u64,v);}
        }
        fn write_words(&mut self,r:usize,words:&[u32]) {
            self.spans+=1;self.max_span=self.max_span.max(words.len()*4);
            for (i,&v) in words.iter().enumerate(){self.write(r+i*4,v);}
        }
    }
    WORDS
    pub mod channel {
        use super::{memory,vm,modeset,vm_control,Registers,words};
        STRUCT
        IMPL
        pub fn check() {
            let mut a=memory::allocator::Allocator::new(1<<30,&[]).unwrap();
            let mut regs=Registers::default();
            let mut io=VmIo{regs:&mut regs,total:1<<30};
            let mut space=vm::Space::new(&mut a,&mut io).unwrap();
            let va=(128<<20)-128*4096;
            let pages:Vec<_>=(0..256).map(|i|0x812345000+i*8192).collect();
            space.map_system(&mut a,&mut io,va,&pages).unwrap();
            for (i,&p) in pages.iter().enumerate() {
                assert_eq!(vm::Io::read(&mut io,space.pte_address(va+i as u64*4096).unwrap()).unwrap(),
                    (p>>8)|1|(1<<32)|(2<<33));
            }
            // Empty tables and their guard PTEs are still entirely zero.
            for (_,range) in space.table_ranges() {
                for at in (range.start..range.end().unwrap()).step_by(8) {
                    if !(0..256).any(|i|space.pte_address(va+i*4096)==Some(at)) {
                        assert_eq!(*io.regs.data.get(&at).unwrap(),0);
                        assert_eq!(*io.regs.data.get(&(at+4)).unwrap(),0);
                    }
                }
            }
            let regs=io.regs;
            assert!(regs.reads<=3000,"{} selector/data reads for 256 mapped pages",regs.reads);
            assert!(regs.spans<=400,"{} aperture spans for two tables and 256 PTEs",regs.spans);
            assert!(regs.max_span<=16384,"aperture hold exceeds existing 16 KiB bound");
            assert!(regs.flushes>=3 && regs.invalidates>=2,"mapping must be published");
        }
    }
}
#[test]fn bounded_mapping_transactions(){gk208::channel::check();}
'''.replace('ROOT', str(ROOT)).replace('WORDS', item(inventory, 'pub(super) fn words(')) \
    .replace('STRUCT', item(channel, "pub(super) struct VmIo<'a>")) \
    .replace('IMPL', item(channel, "impl vm::Io for VmIo<'_>"))
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p / 'check.rs').write_text(source)
            subprocess.run(PRIORITY + ['rustc', '--edition=2021', '-Dwarnings', '--test',
                str(p / 'check.rs'), '-o', str(p / 'check')], check=True)
            subprocess.run([str(p / 'check'), '--nocapture'], check=True)


if __name__ == '__main__':
    unittest.main()
