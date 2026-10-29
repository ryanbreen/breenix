"""Exercise shared-core retirement after confirmed reset with native backing pins."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class ResetRetirement(unittest.TestCase):
    def test_lost_native_resource_releases_its_registry_pin(self):
        retire = item((ROOT / 'kernel/src/gpu/mod.rs').read_text(), '    fn retire_if_unused(')
        harness = r'''
use std::{collections::BTreeMap,sync::{Arc,Mutex,atomic::{AtomicU32,Ordering}}};
mod abi {pub mod backend {pub const GK208:u32=6;}}
static BACKEND:AtomicU32=AtomicU32::new(6);
static PIN:Mutex<Option<Arc<Vec<u8>>>>=Mutex::new(None);
static GPU_RESOURCES:()=();
fn gauge(_: &(),_:i64){}
enum Tag {Background}
struct Request;
mod dev {
 pub fn backend()->u32{super::BACKEND.load(super::Ordering::Relaxed)}
 pub fn resource_unref(_:super::Tag,_:u32,_:Option<super::Arc<Vec<u8>>>)->Option<super::Request>{
  super::PIN.lock().unwrap().take();Some(super::Request)
 }
}
struct Resource {dropped:bool,holds:u32,writers:u32,reading:bool,lost:bool,epoch:u64,backing:Arc<Vec<u8>>}
struct Core {resources:BTreeMap<u32,Resource>,queued:usize}
impl Core {
 fn background_in(&mut self,_:u64,_:Request)->Option<u64>{self.queued+=1;Some(17)}
 // RETIRE
}
fn main(){
 for (backend,lost,holds) in [(6,true,0),(6,false,0),(6,true,1),(4,true,0)] {
  BACKEND.store(backend,Ordering::Relaxed);assert_eq!(dev::backend(),backend);assert_eq!(abi::backend::GK208,6);
  let backing=Arc::new(vec![0;16*1024*1024]);
  *PIN.lock().unwrap()=if backend==6{Some(backing.clone())}else{None};
  let mut c=Core{resources:BTreeMap::from([(7,Resource{dropped:true,holds,writers:0,reading:false,lost,epoch:1,backing:backing.clone()})]),queued:0};
  let result=c.retire_if_unused(7);
  if holds>0 {
   assert!(result.is_none());assert!(c.resources.contains_key(&7));
   assert_eq!(Arc::strong_count(&backing),3,"in-flight native backing was released");
  } else {
   assert!(!c.resources.contains_key(&7));
   assert_eq!(Arc::strong_count(&backing),1,"confirmed-reset resource leaked its native registry pin");
   assert_eq!(c.queued,usize::from(!lost));
  }
  PIN.lock().unwrap().take();
 }
}
'''.replace('// RETIRE', retire)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p / 'retire.rs').write_text(harness)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(p / 'retire.rs'), '-o', str(p / 'retire')], check=True)
            subprocess.run([str(p / 'retire')], check=True)


if __name__ == '__main__':
    unittest.main()
