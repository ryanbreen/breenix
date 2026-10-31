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
        retire = item((ROOT / 'kernel/src/gpu/mod.rs').read_text(), '    fn retire_if_unused(').replace('#[cfg(target_arch = "x86_64")]', '')
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


    def test_retired_native_context_drops_registry_after_reset(self):
        settle=item((ROOT/'kernel/src/gpu/mod.rs').read_text(),'    fn settle(').replace('#[cfg(target_arch = "x86_64")]','')
        harness=r'''
use std::{collections::{BTreeMap,BTreeSet},sync::{Mutex,atomic::{AtomicU32,Ordering}}};
static BACKEND:AtomicU32=AtomicU32::new(6);static REGISTRY:Mutex<BTreeSet<u32>>=Mutex::new(BTreeSet::new());
mod abi {pub mod backend {pub const GK208:u32=6;}}
static GPU_CONTEXTS:()=();fn gauge(_: &(),_:i64){}
enum Tag{Background}struct Request;
mod dev {
 pub fn backend()->u32{super::BACKEND.load(super::Ordering::Relaxed)}
 pub fn ctx_destroy(_:super::Tag,id:u32)->Option<super::Request>{super::REGISTRY.lock().unwrap().remove(&id);Some(super::Request)}
}
#[derive(PartialEq)]enum CtxState{Creating,Live,Quarantined,Reset}
struct Arena{id:u32}
struct Ctx{busy:bool,inflight:Vec<u64>,state:CtxState,device_alive:bool,dropped:bool,vertex_arena:Option<Arena>,private_scratch:Vec<Arena>,vb:u32,scratch:u32,scratch_texture:u32}
struct Resource{dropped:bool}
struct Core{contexts:BTreeMap<u32,Ctx>,resources:BTreeMap<u32,Resource>,queued:usize}
impl Core{
 fn background(&mut self,_:Request){self.queued+=1;}
 fn unhold(&mut self,_:u32){} fn free_if_unused(&mut self,_:u32){}
 SETTLE
}
fn main(){
 let _=CtxState::Live;assert_eq!(abi::backend::GK208,6);
 for (backend,alive,inflight) in [(6,false,false),(6,true,false),(6,false,true),(4,false,false)]{
  BACKEND.store(backend,Ordering::Relaxed);assert_eq!(dev::backend(),backend);*REGISTRY.lock().unwrap()=if backend==6{BTreeSet::from([7])}else{BTreeSet::new()};
  let mut c=Core{contexts:BTreeMap::from([(7,Ctx{busy:false,inflight:if inflight{vec![1]}else{vec![]},state:CtxState::Reset,device_alive:alive,dropped:true,vertex_arena:None,private_scratch:vec![],vb:1,scratch:2,scratch_texture:3})]),resources:BTreeMap::new(),queued:0};
  c.settle(7);
  assert_eq!(c.contexts.contains_key(&7),inflight);
  assert_eq!(REGISTRY.lock().unwrap().contains(&7),backend==6&&inflight,"retired context leaked a native registry ID after reset");
  assert_eq!(c.queued,usize::from(alive&&!inflight));
 }
}
'''.replace('SETTLE',settle)
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'context.rs').write_text(harness)
            subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',str(p/'context.rs'),'-o',str(p/'context')],check=True)
            subprocess.run([str(p/'context')],check=True)

if __name__ == '__main__':
    unittest.main()
