//! Exercise the production adapter with a controlled native engine boundary.
extern crate alloc;
use std::{cell::RefCell, rc::Rc};
#[path = "../kernel/src/gpu/completion.rs"] pub mod gpu_completion;
pub mod gpu {
    pub mod dma { pub struct Pages; }
    pub mod dev {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Tag { Background, Sync(u64) }
        pub struct Done {pub tag:Tag,pub fence:Option<u64>,pub native_fence:Option<u64>,pub answer:u32,pub echoed:u64,pub fenced:bool}
        pub enum Reap {Empty,Done(Done),Fault}
    }
}
pub mod gk208 {
    #[path = "../../kernel/src/gpu/gk208/queue.rs"] pub mod queue;
    pub mod copy {
        #[derive(Clone, Copy)] pub struct Plan;
        #[derive(Clone, Copy)] pub struct Rect {pub src:u64,pub dst:u64,pub width:u32,pub height:u32,pub src_pitch:u32,pub dst_pitch:u32}
    }
    pub mod gr {
        pub mod draw {#[derive(Clone, Copy)] pub struct Draw;}
        pub mod channel {
            use crate::{State,gpu_completion::CeSample};
            use std::{cell::RefCell,rc::Rc};
            pub struct NativeChannel(pub Rc<RefCell<State>>);
            impl NativeChannel {
                pub fn identity(&self)->(u64,u64) {(0x123000,self.0.borrow().epoch)}
                pub fn publish(&mut self,sequence:u32,_:&crate::gk208::adapter::Operation)->Result<(),&'static str> {
                    self.0.borrow_mut().published.push(sequence);Ok(())
                }
                pub fn publish_draw(&mut self,sequence:u32,draw:&crate::gk208::adapter::EncodedDraw)->Result<(),&'static str> {
                    assert_eq!(draw.target,19);
                    assert_eq!(draw.words,[0x20010367,0]);
                    self.0.borrow_mut().published.push(sequence);Ok(())
                }
                pub fn sample(&mut self,sequence:u32)->Result<CeSample,&'static str> {
                    let s=self.0.borrow();Ok(CeSample{context:s.channel,epoch:s.release_epoch,sequence:s.release,consumed:sequence,healthy:!s.fault})
                }
                pub fn healthy(&self)->bool {!self.0.borrow().fault}
                pub fn reset(&mut self)->Result<(),&'static str> {
                    let mut s=self.0.borrow_mut();s.events.push("reset");
                    if s.fail_reset {return Err("reset failed");}
                    s.events.extend(["confirm","remove","unbind"]);
                    if s.fail_unbind {return Err("unbind failed");}
                    s.events.push("restore");s.stopped=true;Ok(())
                }
                pub fn restart(&mut self)->Result<(),&'static str> {
                    let mut s=self.0.borrow_mut();assert!(s.stopped);s.epoch+=1;s.release=0;s.fault=false;s.stopped=false;Ok(())
                }
            }
        }
    }
    #[path = "../../kernel/src/gpu/gk208/caps.rs"] pub mod caps;
    #[path = "../../kernel/src/gpu/gk208/adapter.rs"] pub mod adapter;
}
use gpu::dev::{Reap,Tag};
use gk208::{adapter::{Device,Request},copy::Rect,gr::channel::NativeChannel};
pub struct State {
    epoch:u64,channel:u64,release_epoch:u64,release:u32,fault:bool,
    fail_reset:bool,fail_unbind:bool,stopped:bool,published:Vec<u32>,events:Vec<&'static str>,
}
fn device()->(Device,Rc<RefCell<State>>) {
    let s=Rc::new(RefCell::new(State{epoch:17,channel:0x123000,release_epoch:17,release:0,fault:false,
        fail_reset:false,fail_unbind:false,stopped:false,published:vec![],events:vec![]}));
    (Device::new(NativeChannel(s.clone()),gk208::caps::BoardState::empty()),s)
}
fn request(caller:u64)->Request {
    Request::copy(Tag::Sync(caller),caller,0x456000,9,
        Rect{src:0x100000,dst:0x200000,width:4,height:1,src_pitch:4,dst_pitch:4})
}
fn send(d:&mut Device,caller:u64) {assert!(d.send(request(caller)).is_ok());}
#[test] fn withheld_release_never_returns_done() {
    let (mut d,_)=device();send(&mut d,9001);assert!(matches!(d.reap(),Reap::Empty));assert_eq!(d.outstanding(),1);
}
#[test] fn stale_epoch_never_returns_done() {
    let (mut d,s)=device();send(&mut d,9001);s.borrow_mut().release=1;s.borrow_mut().release_epoch=16;
    assert!(!matches!(d.reap(),Reap::Done(_)));assert_eq!(d.outstanding(),1);
}
#[test] fn wrong_channel_never_returns_done() {
    let (mut d,s)=device();send(&mut d,9001);s.borrow_mut().release=1;s.borrow_mut().channel=0x124000;
    assert!(!matches!(d.reap(),Reap::Done(_)));assert_eq!(d.outstanding(),1);
}
#[test] fn faulted_engine_never_returns_success() {
    let (mut d,s)=device();send(&mut d,9001);s.borrow_mut().release=1;s.borrow_mut().fault=true;
    assert!(d.needs_reset());assert!(matches!(d.reap(),Reap::Fault));assert_eq!(d.outstanding(),1);
}
#[test] fn four_in_flight_leave_no_room_and_caller_is_not_native_sequence() {
    let (mut d,s)=device();
    for caller in [9001,23,42,2] {assert!(d.has_room(1));send(&mut d,caller);}
    assert!(!d.has_room(1));assert!(!d.has_room(5));assert_eq!(d.queue_size(),4);
    assert!(d.send(request(5)).is_err());assert_eq!(s.borrow().published,vec![1,2,3,4]);
    s.borrow_mut().release=1;
    let Reap::Done(done)=d.reap() else {panic!("matching release not reaped")};
    assert_eq!(done.tag,Tag::Sync(9001));assert_eq!(done.answer,gpu_abi::gk208::SUCCESS);
    assert_eq!(done.fence,Some(9001));assert_eq!(done.native_fence,Some(1));assert_eq!(done.echoed,1);assert!(done.fenced);
    assert!(gpu_completion::matches_echo(done.fence,done.native_fence,done.echoed,done.fenced,9001));
    assert_eq!(d.outstanding(),3);assert!(d.has_room(1));assert!(!d.has_room(2));
}
#[test] fn failed_reset_or_unbind_keeps_requests_and_forbids_restart() {
    for unbind in [false,true] {
        let (mut d,s)=device();send(&mut d,9001);
        s.borrow_mut().fail_reset=!unbind;s.borrow_mut().fail_unbind=unbind;
        d.begin_reset();assert!(!d.reset_done());assert!(!d.has_room(1));
        let (held,result)=d.restart();assert!(held.is_empty());assert!(result.is_err());assert_eq!(d.outstanding(),1);
        assert!(!s.borrow().events.contains(&"restore"));
    }
}
#[test] fn confirmed_unwind_returns_old_requests_and_restarts_with_new_epoch() {
    let (mut d,s)=device();send(&mut d,9001);d.begin_reset();assert!(d.reset_done());
    assert_eq!(s.borrow().events,vec!["reset","confirm","remove","unbind","restore"]);
    let (held,result)=d.restart();assert!(result.is_ok());assert_eq!(held.len(),1);assert_eq!(held[0].fence,Some(9001));
    assert_eq!(d.outstanding(),0);send(&mut d,23);
    // An old release after restart cannot complete the new request.
    s.borrow_mut().release=1;assert!(!matches!(d.reap(),Reap::Done(_)));
    s.borrow_mut().release_epoch=18;assert!(matches!(d.reap(),Reap::Done(_)));
}

#[test] fn registered_channel_resumes_after_startup_receipts() {
    let (old,s)=device();drop(old);
    let mut d=Device::registered(NativeChannel(s.clone()),gk208::caps::BoardState::empty(),8);
    send(&mut d,9001);
    assert_eq!(s.borrow().published,vec![9]);
    s.borrow_mut().release=8;
    assert!(!matches!(d.reap(),Reap::Done(_)));
    s.borrow_mut().release=9;
    assert!(matches!(d.reap(),Reap::Done(_)));
}

#[test] fn storage_ack_is_unfenced_ordered_and_retains_backing_until_reaped() {
    use std::sync::Arc;
    let (mut d,s)=device();send(&mut d,9001);
    let pages=Arc::new(gpu::dma::Pages);
    let storage=Request::storage(Tag::Sync(99),Some(pages.clone()));
    assert!(d.send(storage).is_ok());
    assert_eq!(Arc::strong_count(&pages),2);
    assert!(matches!(d.reap(),Reap::Empty));
    assert_eq!(s.borrow().published,vec![1],"storage is not an engine packet");
    s.borrow_mut().release=1;
    let Reap::Done(draw)=d.reap() else {panic!("draw receipt absent")};
    assert_eq!(draw.fence,Some(9001));assert_eq!(Arc::strong_count(&pages),2);
    let Reap::Done(storage)=d.reap() else {panic!("storage receipt absent")};
    assert_eq!(storage.tag,Tag::Sync(99));assert_eq!(storage.answer,gpu_abi::gk208::SUCCESS);
    assert_eq!((storage.fence,storage.native_fence,storage.echoed,storage.fenced),(None,None,0,false));
    assert_eq!(Arc::strong_count(&pages),1);
}

#[test]
fn encoded_draw_requires_its_engine_release_and_epoch_before_caller_fence() {
    let (mut d,s)=device();
    let draw=gk208::adapter::EncodedDraw {target:19,words:vec![0x20010367,0]};
    assert!(d.send(Request::draw(Tag::Sync(9001),9001,19,1,draw)).is_ok());
    assert_eq!(s.borrow().published,[1]);
    assert!(matches!(d.reap(),Reap::Empty));
    s.borrow_mut().release=1;s.borrow_mut().release_epoch=16;
    assert!(matches!(d.reap(),Reap::Empty));
    s.borrow_mut().release_epoch=17;
    let Reap::Done(done)=d.reap() else {panic!("released draw did not complete")};
    assert_eq!(done.fence,Some(9001));assert_eq!(done.native_fence,Some(1));
    assert_eq!(done.echoed,1);assert!(done.fenced);
}
