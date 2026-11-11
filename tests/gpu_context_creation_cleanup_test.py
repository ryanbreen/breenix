"""Interrupted context creation must not unref internal resources never created."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class CreationCleanup(unittest.TestCase):
 def test_partial_creation_retires_only_acknowledged_resources(self):
  source=(ROOT/'kernel/src/gpu/mod.rs').read_text()
  creation=item(source,'fn create_context_with_catalogue(')
  start=creation.index('        c.insert_res(\n            vb,')
  registration=creation[start:creation.index('        c.insert_ctx(',start)]
  retire=item(source,'    fn retire_if_unused(')
  run_rust(self,r'''
use std::{collections::BTreeMap,sync::Arc};
struct Pages;impl Pages{fn none()->Self{Self}}struct Res{dropped:bool,holds:u32,writers:u32,reading:bool,cpu_reading:bool,lost:bool,epoch:u64,backing:Arc<Pages>}
mod kind{pub const BUFFER:u32=1;pub const TARGET:u32=2;pub const TEXTURE:u32=3;}
mod format{pub const B8G8R8A8:u32=1;}
mod encode{pub const VERTEX_BUFFER_BYTES:usize=4096;}
#[cfg(target_arch="x86_64")]mod abi{pub mod backend{pub const GK208:u32=6;}}
enum Tag{Background}struct Request(u32);
mod dev{use super::*;#[cfg(target_arch="x86_64")]pub fn backend()->u32{0}pub fn resource_unref(_:Tag,id:u32,_:Option<Arc<Pages>>)->Option<Request>{Some(Request(id))}}
static GPU_RESOURCES:()=();fn gauge(_:&(),_:i32){}
#[allow(clippy::too_many_arguments)]fn new_res(_:u64,_:u32,_:u32,_:u32,_:u32,_:u64,_:u32,backing:Arc<Pages>,epoch:u64)->Res{Res{dropped:false,holds:0,writers:0,reading:false,cpu_reading:false,lost:false,epoch,backing}}
struct Core{resources:BTreeMap<u32,Res>,unrefs:Vec<u32>}
impl Core{
@RETIRE@
fn insert_res(&mut self,id:u32,r:Res){self.resources.insert(id,r);}
fn background_in(&mut self,_:u64,r:Request)->Option<u64>{self.unrefs.push(r.0);Some(1)}
}
#[test]fn kill_after_first_resource_ack_does_not_reset_surviving_fronts(){
 let mut c=Core{resources:BTreeMap::new(),unrefs:vec![]};let(vb,scratch,scratch_texture,epoch)=(11,12,13,1);
 let vb_pages=Arc::new(Pages);let scratch_pages=Arc::new(Pages);
 @REGISTER@
 // Only the first resource creation returned before SIGKILL interrupted setup.
 c.resources.get_mut(&vb).unwrap().lost=false;
 for id in [vb,scratch,scratch_texture]{c.resources.get_mut(&id).unwrap().dropped=true;c.retire_if_unused(id);}
 assert_eq!(c.unrefs,vec![vb],"cleanup unreferences device resources whose creation never ran");
 assert!(c.resources.is_empty());
}
'''.replace('@REGISTER@',registration).replace('@RETIRE@',retire))
if __name__=='__main__':unittest.main()
