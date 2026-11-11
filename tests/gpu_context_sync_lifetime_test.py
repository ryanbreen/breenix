"""Keep a dropped context alive through its final synchronous GPU request."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT = Path(__file__).resolve().parents[1]
class ContextSyncLifetime(unittest.TestCase):
 def test_dropped_context_waits_for_sync_completion(self):
  body = item((ROOT/'kernel/src/gpu/mod.rs').read_text(), '    fn settle(')
  run_rust(self, r'''
use std::collections::BTreeMap;
#[derive(PartialEq)] enum CtxState { Live, Creating, Quarantined }
struct Arena { id:u32 }
struct Context { busy:bool,inflight:Vec<u64>,state:CtxState,device_alive:bool,dropped:bool,vertex_arena:Option<Arena>,private_scratch:Vec<Arena>,vb:u32,scratch:u32,scratch_texture:u32 }
struct Resource { dropped:bool }
struct SyncReq { ctx:u32 }
struct Core { contexts:BTreeMap<u32,Context>,resources:BTreeMap<u32,Resource>,syncs:BTreeMap<u64,SyncReq>,destroyed:Vec<u32> }
enum Tag { Background }
struct Request(u32);
mod dev { use super::*;#[cfg(target_arch="x86_64")]pub fn backend()->u32{0}pub fn ctx_destroy(_:Tag,id:u32)->Option<Request>{Some(Request(id))} }
#[cfg(target_arch="x86_64")]mod abi { pub mod backend { pub const GK208:u32=6; } }
static GPU_CONTEXTS:()=();fn gauge(_: &(),_:i32){}
impl Core {
@BODY@
fn unhold(&mut self,_:u32){}fn free_if_unused(&mut self,_:u32){}
fn background(&mut self,r:Request){self.destroyed.push(r.0);}
}
#[test] fn a_dying_compositor_keeps_its_device_context_until_its_sync_drain_returns(){
 let mut c=Core {contexts:BTreeMap::from([(7,Context {busy:false,inflight:vec![],state:CtxState::Live,device_alive:true,dropped:true,vertex_arena:None,private_scratch:vec![],vb:0,scratch:0,scratch_texture:0})]),resources:BTreeMap::new(),syncs:BTreeMap::from([(81,SyncReq{ctx:7})]),destroyed:vec![]};
 assert_eq!(c.syncs[&81].ctx,7);c.settle(7);
 assert!(c.destroyed.is_empty(),"device context destroyed while its synchronous drain still owns it");
 assert!(c.contexts[&7].device_alive);
 c.syncs.remove(&81);c.settle(7);
 assert_eq!(c.destroyed,vec![7]);assert!(!c.contexts.contains_key(&7));
}
'''.replace('@BODY@',body))
if __name__=='__main__':unittest.main()
