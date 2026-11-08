#![allow(dead_code)] // Unrelated resource-write and warm-up branches of extracted core methods.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
mod abi {pub mod backend {pub const GK208:u32=6;}}
mod fence_status {pub const OK:u32=0;pub const DEVICE_RESET:u32=4;pub const DEVICE_ERROR:u32=2;pub const FENCE_MISMATCH:u32=3;}
mod dev {pub fn backend()->u32{6}pub fn success()->u32{0x60000000}pub fn ok(a:u32)->bool{a==success()}pub fn context_id(id:u32)->u64{id as u64}pub fn draw_kind()->u32{1}}
mod test_point {pub const STALE_FENCE:u32=20;pub const WITHHOLD_FENCE:u32=21;pub const WITHHOLD_DEPTH:u32=1;pub const WITHHOLD_ALL:u32=2;}
mod test_control {pub fn take(_:u32,_:u32)->Option<u32>{None}pub fn take_if(_:u32,_:u32,_:impl Fn(u32)->bool)->Option<u32>{None}}
mod completion {pub fn finish_write(_: &mut bool,_:&mut Option<u64>,_:u64,_:bool,_:bool){}}
fn notify(){}
fn now_ns()->u64{1}
#[derive(Clone,Copy)]struct Binding{id:u32,fence:u64,answer:u32}
#[derive(Clone,Copy)]struct Presenting{id:u32,seq:u64,owner:u64}
struct Owner{group:u64,last:Option<Binding>}
struct Display{binding:Option<Binding>,accepted:u64,sent:u64,known:bool,outstanding:u32,latest_ok:bool,holds:BTreeSet<u32>,stack:Vec<Owner>}
#[derive(Default)]struct Totals{device_errors:u32}
struct Proc{presents:u64,totals:Totals}
#[derive(PartialEq)]enum CtxState{Live,Quarantined}
struct Arena{ranges:Ranges}struct Ranges;impl Ranges{fn release(&mut self,_:u64){}}
struct Writer;impl Writer{fn publish(&self,_:u32){}fn end_access(&self){}}
struct Depth{kept:BTreeSet<u64>}
#[derive(Default)]struct Answers{values:BTreeMap<u32,u32>}
impl Answers{fn complete(&mut self,n:u32,a:u32){self.values.insert(n,a);}fn take_ready(&mut self,n:u32)->Option<u32>{self.values.remove(&n)}fn take(&mut self,n:u32)->Option<u32>{self.values.remove(&n)}}
struct Context{ns:u32,done_upto:u32,early:BTreeSet<u32>,outcomes:BTreeMap<u32,(u32,u32)>,errors_in_row:u32,inflight:VecDeque<u64>,owner:u64,totals:Totals,state:CtxState,flushes:Answers,native_probe_mask:u32,vertex_arena:Option<Arena>,depth:Option<Depth>}
struct LastWrite{ctx:u32,device_ctx:u64,fence:u64,serial:u64,answer:u32,kind:u32}
struct Resource{holds:u32,writers:u32,invalid:bool,invalidated_by:Option<u64>,generation:u64,last:Option<LastWrite>}
struct Sub{sampled:Vec<()>,sent_ns:Option<u64>,progress_ns:Option<u64>,ctx:u32,n:u32,fence:u64,native_fence:u64,present:Option<Presenting>,published:bool,requests:u32,result:Option<(u32,u32)>,error:Option<u32>,mismatch:Option<u32>,writer:Option<Writer>,written:Vec<u32>,whole:Vec<u32>,held:Vec<u32>,native_probe_mask:u32}
struct Done{answer:u32,fence:Option<u64>,native_fence:Option<u64>,echoed:u64}
impl Done{fn matches_fence(&self,f:u64)->bool{self.fence==Some(f)}}
struct Core{retired_pins:Vec<Vec<()>>,display:Display,contexts:BTreeMap<u32,Context>,procs:BTreeMap<u64,Proc>,resources:BTreeMap<u32,Resource>,subs:BTreeMap<u64,Sub>,stalled:bool,released:Vec<u32>}
impl Core{
 fn unhold(&mut self,id:u32){self.resources.get_mut(&id).unwrap().holds-=1;self.released.push(id);}
 fn pending_watchdog(&self)->Vec<(u64,u32,u64)>{let now:u64=3_000_000_000;mod limits{pub const WATCHDOG_NS:u64=2_000_000_000;}
 // WATCHDOG
 hung
 }
 fn free_if_unused(&mut self,_:u32){}fn settle(&mut self,_:u32){}
 // METHODS
}
fn fresh()->Core{
 let old=Binding{id:71,fence:7<<32|1,answer:dev::success()};
 Core{retired_pins:vec![],display:Display{binding:Some(old),accepted:1,sent:2,known:true,outstanding:1,latest_ok:true,holds:[71,72].into_iter().collect(),stack:vec![Owner{group:1,last:Some(old)}]},
 contexts:BTreeMap::from([(1,Context{ns:7,done_upto:1,early:BTreeSet::new(),outcomes:BTreeMap::new(),errors_in_row:0,inflight:VecDeque::from([17]),owner:1,totals:Totals::default(),state:CtxState::Live,flushes:Answers::default(),native_probe_mask:0,vertex_arena:None,depth:None})]),
 procs:BTreeMap::from([(1,Proc{presents:0,totals:Totals::default()})]),
 resources:[(71,2),(72,2)].into_iter().map(|(id,holds)|(id,Resource{holds,writers:0,invalid:false,invalidated_by:None,generation:0,last:None})).collect(),
 subs:BTreeMap::from([(17,Sub{sampled:vec![],sent_ns:Some(1),progress_ns:None,ctx:1,n:2,fence:123,native_fence:0,present:Some(Presenting{id:72,seq:2,owner:1}),published:false,requests:1,result:None,error:None,mismatch:None,writer:None,written:vec![],whole:vec![],held:vec![72],native_probe_mask:0})]),stalled:false,released:vec![]}
}
fn wait_for<T>(_:Option<u64>,_:bool,f:impl FnOnce(&mut Core)->Option<T>)->Result<T,()>{CORE.with(|c|f(&mut c.borrow_mut()).ok_or(()))}
fn wait_for_traced<T>(deadline:Option<u64>,interruptible:bool,f:impl FnOnce(&mut Core)->Option<T>,_:Option<()>,_:bool,_:Option<()>)->Result<T,()>{wait_for(deadline,interruptible,f)}
thread_local!{static CORE:std::cell::RefCell<Core>=std::cell::RefCell::new(fresh());}
fn present_answer(n:u32)->Result<(u32,u32),()>{let ctx_id=1;
 // PRESENT_ANSWER
 outcome
}
fn wait_poll(n:u32)->Result<Result<(u32,u32),i64>,()>{let id=1;let deadline=0;const EINVAL:i64=22;
 // WAIT_POLL
 outcome
}
#[test]fn present_returns_with_a_completed_wait_fence_but_live_latch_holds(){
 assert!(wait_poll(2).is_err());
 CORE.with(|c|c.borrow_mut().present_accepted(17));
 assert_eq!(present_answer(2),Ok((fence_status::OK,dev::success())));
 assert_eq!(wait_poll(2),Ok(Ok((fence_status::OK,0))),"PRESENT returned but timeout-zero WAIT did not complete");
 CORE.with(|c|{let mut c=c.borrow_mut();
  assert!(c.subs[&17].published);assert_eq!(c.subs[&17].requests,1);
  assert!(c.contexts[&1].inflight.contains(&17),"accepted flip lost its context lifetime");
  assert_eq!(c.pending_watchdog(),[(17,1,1)],"unlatched accepted flip escaped the watchdog");
  assert_eq!(c.display.outstanding,1);assert!(c.released.is_empty());
  assert_eq!(c.display.binding.unwrap().id,72);assert!(c.display.known);
  assert_eq!(c.resources[&71].holds,2);assert_eq!(c.resources[&72].holds,2);
  c.submit_returned(17,true,Done{answer:dev::success(),fence:Some(123),native_fence:Some(9),echoed:9});
  assert_eq!(c.display.outstanding,0);assert_eq!(c.procs[&1].presents,1);
  assert_eq!(c.contexts[&1].done_upto,2);assert!(c.contexts[&1].early.is_empty());
  assert!(c.subs.is_empty());assert!(c.contexts[&1].inflight.is_empty());assert_eq!(c.resources[&71].holds,0);assert_eq!(c.resources[&72].holds,2);
 });
 assert_eq!(wait_poll(2),Ok(Ok((fence_status::OK,0))));
}
#[test]fn acceptance_preserves_earlier_context_fence_order(){
 CORE.with(|c|{let mut c=c.borrow_mut();c.subs.get_mut(&17).unwrap().n=3;c.present_accepted(17);assert_eq!(c.contexts[&1].done_upto,1);assert!(c.contexts[&1].early.contains(&3));});
 assert!(present_answer(3).is_err(),"PRESENT returned ahead of an earlier context fence");
 assert!(wait_poll(3).is_err());
 CORE.with(|c|{let mut c=c.borrow_mut();let ctx=c.contexts.get_mut(&1).unwrap();ctx.early.insert(2);while ctx.early.remove(&(ctx.done_upto+1)){ctx.done_upto+=1;}});
 assert_eq!(present_answer(3),Ok((fence_status::OK,dev::success())));
 assert_eq!(wait_poll(3),Ok(Ok((fence_status::OK,0))));
}
