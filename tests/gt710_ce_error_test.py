"""Execute the production completion-error diagnostic with distinct register words."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class CeError(unittest.TestCase):
    def test_timeout_records_raw_engine_transport_and_flush_state(self):
        source = (ROOT / 'kernel/src/gpu/gk208/ce.rs').read_text()
        begin = source.index('        if done.is_none() {', source.index('    fn execute('))
        end = source.index('        unsafe {self.context.finish_device', begin)
        harness = r'''
use std::cell::RefCell;
thread_local! {static LINE:RefCell<String>=RefCell::new(String::new());}
#[macro_export] macro_rules! serial_println {($($a:tt)*)=>{{let line=format!($($a)*);LINE.with(|s|*s.borrow_mut()=line);}};}
struct Registers;
impl Registers {fn read(&mut self,r:usize)->u32 {r as u32 ^ 0x5a000000}}
fn read(_: &mut Registers,_:u64,address:u64)->Result<u32,&'static str>{Ok(address as u32)}
struct Range {start:u64}
struct Allocation {range:Range}
pub struct Channel {id:u32,commands:Allocation}
pub struct Engine {pub base:usize,pub pbdma:usize,pub engine:usize,pub run:usize}
impl Channel {
    pub fn release_address(&self,seq:u32)->u64{self.commands.range.start+8192+u64::from((seq-1)%4)*16}
    fn execute(&self,regs:&mut Registers,total:u64,e:&Engine,seq:u32)->Result<(),&'static str>{
        let done:Option<()>=None;
BLOCK
        Ok(())
    }
}
fn main(){
    let c=Channel{id:3,commands:Allocation{range:Range{start:0x200000}}};
    let e=Engine{base:0x104000,pbdma:2,engine:5,run:4};
    assert_eq!(c.execute(&mut Registers,1<<24,&e,3),Err("CE completion failed; backing retained"));
    let line=LINE.with(|s|s.borrow().clone());
    for (name,register) in [("pmc",0x200),("engine_status",0x2668),("pbdma_status",0x3088),
                          ("preempt",0x2634),("runlist",0x22a4),("mmu_flush",0x100c80)] {
        assert!(line.split_whitespace().any(|field|field==format!("{name}={:#x}",register^0x5a000000)),
                "missing raw register {register:#x}: {line}");
    }
    assert!(line.contains("sem=2105376"),"semaphore diagnostic names another request slot: {line}");
}
'''.replace('BLOCK', source[begin:end])
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'error.rs').write_text(harness)
            priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(path / 'error.rs'),
                            '-o', str(path / 'error')], check=True, cwd=ROOT)
            subprocess.run([str(path / 'error')], check=True)


if __name__ == '__main__':
    unittest.main()
