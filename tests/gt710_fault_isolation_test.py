"""Exercise Corrected G19 logical-context survival and error precedence in the production shared core."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item

class FaultIsolation(unittest.TestCase):
    def test_logical_context_survives_and_mmu_error_survives_chunk_reset(self):
        core = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        methods = '\n'.join(item(core, marker) for marker in (
            '    fn returned(', '    fn ended(', '    fn ended_tag('))
        methods = methods.replace('#[cfg(target_arch = "x86_64")]', '')
        methods = methods.replace('#[cfg(not(target_arch = "x86_64"))]\n                let mmu: Option<u32> = None;', '')
        source = r'''
use std::collections::{BTreeMap,VecDeque};
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
enum Tag {Background,Sync(u64),Submit{ctx:u32,serial:u64,last:bool}}
struct Done {tag:Tag,answer:u32}
struct Request(Tag);
impl Request {fn tag(&self)->Tag{self.0}}
#[derive(Debug,PartialEq,Eq)] enum CtxState {Live,Quarantined}
struct Ctx {state:CtxState,depth:Option<()>}
struct Sync {ctx:u32,warm:Option<u64>}
struct Sub {requests:u32,error:Option<u32>}
mod abi {pub mod backend {pub const GK208:u32=6;}}
mod fence_status {pub const DEVICE_RESET:u32=3;pub const DEVICE_ERROR:u32=6;}
mod dev {pub fn backend()->u32{6} pub fn ok(a:u32)->bool{a==1}}
fn now_ns()->u64{42}
struct Core {
 contexts:BTreeMap<u32,Ctx>,syncs:BTreeMap<u64,Sync>,subs:BTreeMap<u64,Sub>,backlog:VecDeque<(u64,Request)>,
 traffic_ns:u64,faults:u32,settled:Vec<u32>,results:Vec<(u64,u32,u32)>,epoch:u64,resources:BTreeMap<u32,u64>,
}
impl Core {
 // METHODS
 fn submit_returned(&mut self,serial:u64,_:bool,d:Done){
  let s=self.subs.get_mut(&serial).unwrap();s.requests-=1;
  if !dev::ok(d.answer){s.error=Some(d.answer);}
 }
 fn sync_ended(&mut self,serial:u64,_:Option<Done>){self.syncs.remove(&serial);}
 fn fault(&mut self){self.faults+=1;}
 fn settle(&mut self,id:u32){self.settled.push(id);}
 fn publish(&mut self,serial:u64,status:u32,answer:u32){self.results.push((serial,status,answer));}
 fn retire_if_done(&mut self,serial:u64){if self.subs[&serial].requests==0{self.subs.remove(&serial);}}
}
fn main(){
 let mut c=Core{contexts:BTreeMap::from([(7,Ctx{state:CtxState::Live,depth:Some(())}),(8,Ctx{state:CtxState::Live,depth:Some(())})]),
  syncs:BTreeMap::new(),subs:BTreeMap::from([(9001,Sub{requests:2,error:None})]),
  backlog:VecDeque::from([(17,Request(Tag::Submit{ctx:7,serial:9001,last:false})),(17,Request(Tag::Submit{ctx:8,serial:23,last:true}))]),
  traffic_ns:0,faults:0,settled:vec![],results:vec![],epoch:17,resources:BTreeMap::from([(7,123),(8,456)])};
 c.returned(Done{tag:Tag::Submit{ctx:7,serial:9001,last:true},answer:0x60010002});
 assert_eq!(c.contexts[&7].state,CtxState::Live);assert_eq!(c.contexts[&7].depth,Some(()));
 assert_eq!(c.contexts[&8].state,CtxState::Live);assert_eq!(c.contexts[&8].depth,Some(()));
 assert_eq!(c.backlog.len(),2,"logical-context queued requests survive channel recovery");
 let (_, request)=c.backlog.pop_front().unwrap();c.ended(request);
 assert_eq!(c.results,[(9001,fence_status::DEVICE_ERROR,0x60010002)]);
 assert!(c.settled.is_empty());assert_eq!(c.epoch,17);assert_eq!(c.resources,BTreeMap::from([(7,123),(8,456)]));
 assert_eq!(c.faults,0,"an attributed fault must not request whole-device reset");assert_eq!(c.traffic_ns,42);
 c.syncs.insert(24,Sync{ctx:8,warm:None});c.returned(Done{tag:Tag::Sync(24),answer:1});
 c.returned(Done{tag:Tag::Background,answer:1});
 assert_eq!(c.contexts[&8].state,CtxState::Live);assert_eq!(c.faults,0);
}
'''.replace('// METHODS', methods)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p / 'isolation.rs').write_text(source)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(p / 'isolation.rs'), '-o', str(p / 'isolation')], check=True)
            subprocess.run([str(p / 'isolation')], check=True)

if __name__ == '__main__':
    unittest.main()
