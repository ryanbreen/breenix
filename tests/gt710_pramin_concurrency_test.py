"""Interleave VM and command aperture accesses through the production PRAMIN code."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from gt710_gr_live_unwind_test import item

class PraminConcurrency(unittest.TestCase):
    def test_another_owner_cannot_change_the_bank_during_a_pte_read(self):
        registers=(ROOT/'kernel/src/gpu/gk208/mod.rs').read_text()
        impl=item(registers,'impl memory::Access for Registers')
        locking=item(impl,'    fn with_pramin') if '    fn with_pramin' in impl else ''
        harness=r'''
extern crate alloc;
#[path="MEMORY"] mod memory;
use std::sync::{Arc,Mutex,mpsc};
use memory::Access;
static PRAMIN_LOCK:Mutex<()>=Mutex::new(());
struct Registers { bank:Arc<Mutex<u32>>, trigger:Option<mpsc::Sender<()>> }
impl Access for Registers {
 fn read(&mut self,reg:usize)->u32 {
  if reg==0x1700 {return *self.bank.lock().unwrap();}
  assert_eq!(reg,0x700000);
  if let Some(trigger)=self.trigger.take() {
   trigger.send(()).unwrap();
   std::thread::sleep(std::time::Duration::from_millis(100));
  }
  // The VM bank's PTE is empty; the command bank contains nonzero words.
  if *self.bank.lock().unwrap()==0x10 {0} else {0xbeef}
 }
 fn write(&mut self,reg:usize,value:u32) {
  if reg==0x1700 {*self.bank.lock().unwrap()=value;} else {assert_eq!(reg,0x700000);}
 }
 // LOCKING
}
fn main() {
 let bank=Arc::new(Mutex::new(0));let (trigger,started)=mpsc::channel();
 let (done,finished)=mpsc::channel();let other=bank.clone();
 let writer=std::thread::spawn(move|| {
  started.recv().unwrap();
  let mut registers=Registers {bank:other,trigger:None};
  let mut p=memory::Pramin::new(&mut registers,4<<20);
  p.write(2<<20,0xbeef).unwrap();
  finished.recv().unwrap();
 });
 let mut registers=Registers {bank:bank.clone(),trigger:Some(trigger)};
 let mut p=memory::Pramin::new(&mut registers,4<<20);
 let pte=p.read(1<<20).unwrap();drop(p);done.send(()).unwrap();writer.join().unwrap();
 assert_eq!(pte,0,"a command publisher must not cause VM mapping presence mismatch");
 assert_eq!(*bank.lock().unwrap(),0,"restore the pre-transaction bank");
}
'''.replace('"MEMORY"','"'+str(ROOT/'kernel/src/gpu/gk208/memory.rs')+'"').replace('// LOCKING',locking)
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp);(path/'pramin.rs').write_text(harness)
            subprocess.run(priority+['rustc','--edition=2021','-Adead_code',str(path/'pramin.rs'),'-o',str(path/'pramin')],check=True)
            subprocess.run([str(path/'pramin')],check=True)

if __name__=='__main__': unittest.main()
