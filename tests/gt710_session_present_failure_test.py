"""Exercise session presentation recovery with failing EVO/evidence boundaries."""
import unittest
from gt710_gr_live_unwind_test import item
import gt710_receipt_timing_test as timing


class SessionPresentationFailure(unittest.TestCase):
    run_fixture = timing.ReceiptTiming.run_fixture

    def test_failed_submission_completion_and_drain_allow_a_replacement(self):
        process = timing.source('kernel/src/gpu/gk208/ce/process.rs')
        functions = '\n'.join(item(process, marker) for marker in [
            'pub(in crate::gk208) fn present(',
            'pub(in crate::gk208) fn present_complete(',
            'pub(crate) fn set_mode(',
        ]).replace('pub(in crate::gk208)', 'pub(crate)').replace('super::super::', '')
        fixture = r'''
use std::{cell::Cell, sync::Mutex};
thread_local! {static FAIL:Cell<u8>=const{Cell::new(0)};}
fn fail(at:u8)->bool{FAIL.with(|f|if f.get()==at{f.set(0);true}else{false})}
mod present {
 #[derive(Clone,Copy,Debug,PartialEq)]
 pub struct Surface {pub resource:u32,pub address:u64,pub width:u32,pub height:u32,pub pitch:u32,pub block_height:u32,pub format:u32,pub dma:u32}
 #[derive(Clone,Copy)]pub struct Receipt {pub notifier:u32,pub active:[u32;5],pub armed:[u32;5]}
}
struct Registers(usize);
#[derive(Clone,Copy)]struct Range {start:u64,size:u64}
struct Object {name:&'static str,range:Range}
struct Layout {objects:Vec<Object>}
#[derive(Clone,Copy)]struct Console {vram:u64,bytes:usize}
struct Resources;
impl Resources {
 fn hold(&self,id:u32)->Result<u32,&'static str>{Ok(id)}
 fn scanout_allocation(&self,_:u32)->Option<Range>{Some(Range{start:100,size:100})}
}
#[derive(Clone,Copy)] struct Mode {width:u32,height:u32}
const MODES:[Mode;2]=[Mode{width:1280,height:800},Mode{width:2560,height:1600}];
struct Session {modes:[Mode;2],pending_present:Option<present::Surface>,ready_present:Option<present::Receipt>,
 scanout:Vec<u32>,evo:native::Evo,registers:usize,resources:Resources,mode:u32,console:Console,layout:Layout}
struct Owner(Mutex<Option<Session>>);
impl Owner {fn lock(&self)->std::sync::MutexGuard<'_,Option<Session>>{self.0.lock().unwrap()}}
static SESSION:Owner=Owner(Mutex::new(None));
mod native {
 use super::*;
 pub struct Evo {pub head:u32,pub pending:bool}
 fn receipt()->present::Receipt{present::Receipt{notifier:1,active:[7;5],armed:[7;5]}}
 pub fn present_accepted(e:&mut Evo,r:&mut Registers,_:present::Surface,_:u32)->Result<Option<present::Receipt>,&'static str>{
  assert_eq!(r.0,0);assert!(!e.pending,"replacement reused outstanding EVO work");
  // Both pre-arm and post-publication failures leave EVO's slot retired.
  if fail(1)||fail(2){return Err("submission failed")}
  e.pending=true;Ok(None)
 }
 pub fn present_latched(e:&mut Evo,_:&mut Registers,_:present::Surface,_:u32)->Result<Option<present::Receipt>,&'static str>{
  if fail(3){e.pending=false;return Err("completion failed")}
  if !e.pending{return Err("native presentation not armed")}
  if fail(5){return Ok(None)}
  e.pending=false;Ok(Some(receipt()))
 }
 pub fn wait_present(e:&mut Evo,r:&mut Registers,s:present::Surface,mode:u32)->Result<present::Receipt,&'static str>{
  if fail(6){e.pending=false;return Err("drain failed")}
  present_latched(e,r,s,mode)?.ok_or("did not latch")
 }
 pub fn present(e:&mut Evo,r:&mut Registers,s:present::Surface,mode:u32)->Result<present::Receipt,&'static str>{
  present_accepted(e,r,s,mode)?;wait_present(e,r,s,mode)
 }
}
mod presentation_trace {
 pub fn snapshot() {}
 pub fn export(_:()) {}
}
mod display_evidence {
 pub fn transition(_: &mut super::Registers,_:u32,_:super::present::Surface,_:&super::present::Receipt,_:[u64;2],_:[u64;2],_:&str,_:u32)->Result<(),&'static str>{
  if super::fail(4){Err("evidence failed")}else{Ok(())}
 }
}
FUNCTIONS
fn surface(resource:u32)->present::Surface{present::Surface{resource,address:100,width:1280,height:800,pitch:5120,block_height:0,format:0,dma:1}}
fn setup(){*SESSION.lock()=Some(Session{modes:MODES,pending_present:None,ready_present:None,scanout:vec![6],evo:native::Evo{head:0,pending:false},registers:0,resources:Resources,mode:1,console:Console{vram:100,bytes:100},layout:Layout{objects:vec![Object{name:"logical-mode",range:Range{start:100,size:100}}]}});}
fn pinned(ids:&[u32]){assert_eq!(SESSION.lock().as_ref().unwrap().scanout,ids);}
fn replacement(){present(surface(8)).unwrap();pinned(&[6,7,8]);assert!(present_complete(surface(8)).unwrap());pinned(&[8]);}
#[test]fn recover(){
 for fault in 1..=4 {
  for mode in [false,true] {
   setup();FAIL.with(|f|f.set(fault));
   if fault<=2 {assert!(present(surface(7)).is_err());}
   else {present(surface(7)).unwrap();assert!(present_complete(surface(7)).is_err());}
   pinned(&[6,7]); // uncertainty never releases possible display readers
   assert!(SESSION.lock().as_ref().unwrap().pending_present.is_none(),"failed session slot survived EVO retirement");
   if mode {set_mode(0).unwrap();pinned(&[]);}else{replacement();}
  }
 }
 // An incomplete poll retains the slot, and refuses a foreign completion.
 setup();present(surface(7)).unwrap();FAIL.with(|f|f.set(5));
 assert!(!present_complete(surface(7)).unwrap());
 assert!(present_complete(surface(8)).is_err());pinned(&[6,7]);
 assert!(present_complete(surface(7)).unwrap());pinned(&[7]);
 // Failure while draining a previous flip is reported once, then recoverable.
 for mode in [false,true] {
  setup();present(surface(7)).unwrap();FAIL.with(|f|f.set(6));
  if mode {assert!(set_mode(0).is_err());}else{assert!(present(surface(8)).is_err());}
  pinned(&[6,7]);
  if mode {set_mode(0).unwrap();pinned(&[]);}else{replacement();}
 }
}
'''.replace('FUNCTIONS', functions)
        self.run_fixture(fixture)


if __name__ == '__main__':
    unittest.main()
