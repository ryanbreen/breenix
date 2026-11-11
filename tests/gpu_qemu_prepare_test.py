"""Keep QEMU claim preparation within the per-pipeline warm-up contract (gpu-abi.md)."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class QemuPrepare(unittest.TestCase):
 def test_qemu_waits_for_the_prepared_catalogue(self):
  body=item((ROOT/'kernel/src/gpu/mod.rs').read_text(),'pub fn prepare_window_renderer(')
  run_rust(self,r'''
use std::{collections::BTreeMap,sync::{Mutex,atomic::{AtomicUsize,Ordering}}};
mod abi{pub mod pipeline{pub const SOLID:u32=0;pub const COPY:u32=1;}}
const ENODEV:i64=19;const EIO:i64=5;const EINTR:i64=4;
static PLATFORM:AtomicUsize=AtomicUsize::new(1);static CREATED:AtomicUsize=AtomicUsize::new(0);static CHECKS:AtomicUsize=AtomicUsize::new(0);
mod platform_config{pub fn is_qemu()->bool{crate::PLATFORM.load(crate::Ordering::Relaxed)==1}pub fn is_parallels()->bool{crate::PLATFORM.load(crate::Ordering::Relaxed)==2}}
mod limits{pub const CONTEXTS:usize=16;}
#[derive(Clone,Copy,PartialEq)]enum CtxState{Live}
struct Ctx{warm_catalogue:bool,state:CtxState,dropped:bool,warm:u32}
#[derive(Default)]struct Proc{contexts:BTreeMap<u64,u32>,prepared_context:Option<u64>}
#[derive(Default)]struct Core{procs:BTreeMap<u64,Proc>,contexts:BTreeMap<u32,Ctx>}
impl Core{fn proc_mut(&mut self,pid:u64)->&mut Proc{self.procs.entry(pid).or_default()}}
static CORE:Mutex<Core>=Mutex::new(Core{procs:BTreeMap::new(),contexts:BTreeMap::new()});
fn with_core<R>(f:impl FnOnce(&mut Core)->R)->Option<R>{Some(f(&mut CORE.lock().unwrap()))}
fn catalogue_mask()->u32{0xffff}
fn create_context_with_catalogue(pid:u64,warm:bool)->Result<(u64,u32),i64>{
 assert!(warm);CREATED.fetch_add(1,Ordering::Relaxed);
 with_core(|c|{c.proc_mut(pid).contexts.insert(11,1);c.contexts.insert(1,Ctx{warm_catalogue:true,state:CtxState::Live,dropped:false,warm:0});});Ok((11,0))
}
fn destroy_context(pid:u64,handle:u64){with_core(|c|{c.proc_mut(pid).contexts.remove(&handle);c.contexts.remove(&1);});}
fn wait_for<T>(_:Option<u64>,_:bool,mut f:impl FnMut(&mut Core)->Option<T>)->Result<T,()>{
 for _ in 0..2{CHECKS.fetch_add(1,Ordering::Relaxed);if let Some(found)=with_core(|c|f(c)).flatten(){return Ok(found);}with_core(|c|for(id,ctx)in c.contexts.iter_mut(){if *id!=9{ctx.warm=if platform_config::is_qemu(){3}else{catalogue_mask()};}});}Err(())
}
@BODY@
#[test]fn preparation_waits_and_native_stays_unchanged(){
 assert!(platform_config::is_qemu());let _=(abi::pipeline::SOLID,abi::pipeline::COPY);
 for platform in [1,2]{PLATFORM.store(platform,Ordering::Relaxed);*CORE.lock().unwrap()=Core::default();CREATED.store(0,Ordering::Relaxed);CHECKS.store(0,Ordering::Relaxed);
  assert_eq!(prepare_window_renderer(42,true),Ok(()));assert_eq!(CREATED.load(Ordering::Relaxed),1,"window setup did not reserve its renderer");assert_eq!(CHECKS.load(Ordering::Relaxed),2,"preparation did not wait for the actual catalogue state");
  with_core(|c|{assert_eq!(c.procs[&42].prepared_context,Some(11));assert_eq!(c.contexts[&1].warm,if platform_config::is_qemu(){3}else{catalogue_mask()});});
  assert_eq!(prepare_window_renderer(42,false),Ok(()));assert_eq!(CREATED.load(Ordering::Relaxed),1);
 }
 // A client CREATE must return to registration instead of synchronously
 // compiling the whole catalogue before its first window becomes visible.
 PLATFORM.store(1,Ordering::Relaxed);*CORE.lock().unwrap()=Core::default();CREATED.store(0,Ordering::Relaxed);CHECKS.store(0,Ordering::Relaxed);
 assert_eq!(prepare_window_renderer(43,false),Ok(()));
 assert_eq!((CREATED.load(Ordering::Relaxed),CHECKS.load(Ordering::Relaxed)),(0,0),"client registration waited on the compositor preparation path");
 PLATFORM.store(1,Ordering::Relaxed);*CORE.lock().unwrap()=Core::default();
 with_core(|c|{c.proc_mut(42).contexts.insert(99,9);c.contexts.insert(9,Ctx{warm_catalogue:true,state:CtxState::Live,dropped:false,warm:0});});
 assert_eq!(prepare_window_renderer(42,true),Ok(()),"an unrelated font renderer blocked the reserved compositor");
 with_core(|c|assert_eq!(c.contexts[&9].warm,0,"the test must leave the unrelated catalogue cold"));
 PLATFORM.store(0,Ordering::Relaxed);*CORE.lock().unwrap()=Core::default();CREATED.store(0,Ordering::Relaxed);CHECKS.store(0,Ordering::Relaxed);
 assert_eq!(prepare_window_renderer(42,true),Ok(()));assert_eq!((CREATED.load(Ordering::Relaxed),CHECKS.load(Ordering::Relaxed)),(0,0));
}
'''.replace('@BODY@',body))
if __name__=='__main__':unittest.main()
