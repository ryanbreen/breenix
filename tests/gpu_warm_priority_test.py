"""A reserved compositor can warm while ordinary font work is in flight."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class WarmPriority(unittest.TestCase):
 def test_reserved_renderer_has_finite_priority_on_qemu(self):
  s=(ROOT/'kernel/src/gpu/mod.rs').read_text()
  body=item(s,'fn warm_context(') if 'fn warm_context(' in s else 'fn warm_context(c:&Core,warming:impl Fn(&Ctx)->bool)->Option<u32>{c.contexts.iter().find(|(_,x)|warming(x)&&!x.busy).map(|(&id,_)|id)}'
  run_rust(self,r'''
use std::{collections::BTreeMap,sync::atomic::{AtomicBool,Ordering}};
static QEMU:AtomicBool=AtomicBool::new(true);mod platform_config{pub fn is_qemu()->bool{super::QEMU.load(super::Ordering::Relaxed)}}
struct Ctx{owner:u64,busy:bool,cold:bool}struct Proc{contexts:BTreeMap<u64,u32>,prepared_context:Option<u64>}
struct Core{contexts:BTreeMap<u32,Ctx>,procs:BTreeMap<u64,Proc>}
@BODY@
#[test]fn priority_ends_when_the_reserved_catalogue_is_ready(){
 let mut c=Core{contexts:BTreeMap::from([(1,Ctx{owner:42,busy:false,cold:true}),(2,Ctx{owner:42,busy:false,cold:true})]),procs:BTreeMap::from([(42,Proc{contexts:BTreeMap::from([(11,1),(22,2)]),prepared_context:Some(22)})])};
 assert_eq!(c.contexts[&1].owner,42);assert_eq!(c.procs[&42].contexts[&22],2);assert_eq!(c.procs[&42].prepared_context,Some(22));assert!(platform_config::is_qemu());
 assert_eq!(warm_context(&c,|x|x.cold),Some(2),"font catalogue ran before the compositor reserved for startup");
 c.contexts.get_mut(&2).unwrap().cold=false;assert_eq!(warm_context(&c,|x|x.cold),Some(1),"ordinary work must resume after the finite reserved catalogue");
 c.contexts.get_mut(&2).unwrap().cold=true;QEMU.store(false,Ordering::Relaxed);assert_eq!(warm_context(&c,|x|x.cold),Some(1),"other backends retain context order");
}
'''.replace('@BODY@',body))
  self.assertIn('warm_context(c, &warming)',s)
if __name__=='__main__':unittest.main()
