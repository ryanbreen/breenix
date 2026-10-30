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
#[path = "../kernel/src/gpu/gk208"]
pub mod gk208 {
    #[path = "queue.rs"] pub mod queue;
    #[allow(dead_code)] // Only transfer request construction is needed by this fixture.
    #[path = "copy.rs"] pub mod copy;
    pub fn transfer_request(caller: u64) -> adapter::Request {
        let image=copy::Image {address:1<<32,width:16,height:8,texel_bytes:4,pitch:64,block_height:2,bytes:1024};
        let backing=copy::Linear {address:2<<32,bytes:4096,offset:0,stride:64};
        let plan=copy::readback(image,backing,[0,0,16,8]).unwrap();
        adapter::Request::transfer(crate::Tag::Sync(caller),caller,0x456000,plan,false,None)
    }
    pub mod gr {
        #[path = "error.rs"] pub mod error;
        pub mod draw {#[derive(Clone, Copy)] pub struct Draw;}
        pub mod channel {
            use crate::{State,gpu_completion::CeSample};
            use std::{cell::RefCell,rc::Rc};
            pub struct NativeChannel(pub Rc<RefCell<State>>);
            impl NativeChannel {
                pub fn identity(&self)->(u64,u64) {(0x123000,self.0.borrow().epoch)}
                pub fn publish(&mut self,sequence:u32,operation:&crate::gk208::adapter::Operation)->Result<(),&'static str> {
                    self.0.borrow_mut().operations.push(match operation {crate::gk208::adapter::Operation::Transfer(_)=>"transfer",_=>"other"});
                    self.0.borrow_mut().published.push(sequence);Ok(())
                }
                pub fn publish_draw(&mut self,sequence:u32,draw:&crate::gk208::adapter::EncodedDraw)->Result<(),&'static str> {
                    assert_eq!(draw.target,19);
                    assert_eq!(draw.words,self.0.borrow().expected_draw);
                    if self.0.borrow().refuse_publish {return Err("injected prepublication refusal");}
                    if self.0.borrow().fail_publish {
                        self.0.borrow_mut().published.push(sequence);
                        return Err("injected native publication failure");
                    }
                    self.0.borrow_mut().published.push(sequence);Ok(())
                }
                pub fn sample(&mut self,sequence:u32)->Result<CeSample,&'static str> {
                    let s=self.0.borrow();Ok(CeSample{context:s.channel,epoch:s.release_epoch,sequence:s.release,consumed:sequence,healthy:!s.fault})
                }
                pub fn error_trap(&self)->Result<Option<crate::gk208::gr::error::Trap>,&'static str> {
                    crate::gk208::gr::error::capture(&mut *self.0.borrow_mut())
                }
                pub fn acknowledge_error(&mut self,trap:crate::gk208::gr::error::Trap)->Result<(),&'static str> {
                    crate::gk208::gr::error::acknowledge(&mut *self.0.borrow_mut(),trap)
                }
                pub fn transfer_drain(&mut self,sequence:u32)->Result<CeSample,&'static str> {
                    let mut sample=self.sample(sequence)?;sample.sequence=self.0.borrow().drain;Ok(sample)
                }
                pub fn published(&self,sequence:u32)->bool {self.0.borrow().published.contains(&sequence)}
                pub fn healthy(&self)->Result<(), &'static str> {if self.0.borrow().fault {Err("injected engine fault")} else {Ok(())}}
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
    #[path = "caps.rs"] pub mod caps;
    #[path = "adapter.rs"] pub mod adapter;
}
use gpu::dev::{Reap,Tag};
use gk208::{adapter::{Device,Request},copy::Rect,gr::channel::NativeChannel};
pub struct State {
    epoch:u64,channel:u64,release_epoch:u64,release:u32,drain:u32,fault:bool,
    registers:std::collections::BTreeMap<usize,u32>,writes:Vec<(usize,u32)>,
    fail_reset:bool,fail_unbind:bool,fail_publish:bool,refuse_publish:bool,stopped:bool,operations:Vec<&'static str>,published:Vec<u32>,events:Vec<&'static str>,expected_draw:Vec<u32>,
}
fn device()->(Device,Rc<RefCell<State>>) {
    let s=Rc::new(RefCell::new(State{epoch:17,channel:0x123000,release_epoch:17,release:0,drain:0,fault:false,
        registers:Default::default(),writes:vec![],fail_reset:false,fail_unbind:false,fail_publish:false,refuse_publish:false,stopped:false,operations:vec![],published:vec![],events:vec![],expected_draw:vec![0x20010367,0]}));
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
    assert_eq!(d.fault_reason(),Some("injected engine fault"),
        "the health fast path must retain the producing error before reprobe reports it");
    s.borrow_mut().fault=false;
    assert!(d.needs_reset(),"a transient healthy read cannot clear the captured fault");
    assert!(matches!(d.reap(),Reap::Fault));
    assert_eq!(d.fault_reason(),Some("injected engine fault"));
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
    let draw=gk208::adapter::EncodedDraw {transfer:None,target:19,words:vec![0x20010367,0]};
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

#[test]
fn stale_transfer_returns_device_echo_only_after_terminal_drain_then_next_request_is_clean() {
    let (mut d,s)=device();
    let mut request=gk208::transfer_request(9001);
    request.transfer_controls(gk208::copy::Extras {skip:false,stale_fence:true});
    assert!(d.send(request).is_ok());
    s.borrow_mut().release=2;
    assert!(matches!(d.reap(),Reap::Empty));assert_eq!(d.outstanding(),1);
    s.borrow_mut().drain=1;s.borrow_mut().release_epoch=16;
    assert!(matches!(d.reap(),Reap::Empty));assert_eq!(d.outstanding(),1);
    s.borrow_mut().release_epoch=17;s.borrow_mut().channel=0x124000;
    assert!(matches!(d.reap(),Reap::Empty));assert_eq!(d.outstanding(),1);
    s.borrow_mut().channel=0x123000;
    let Reap::Done(done)=d.reap() else {panic!("terminal drain not reaped")};
    assert_eq!((done.fence,done.native_fence,done.echoed),(Some(9001),Some(1),2));
    assert!(!gpu_completion::matches_echo(done.fence,done.native_fence,done.echoed,done.fenced,9001));
    assert_eq!(d.outstanding(),0);
    assert!(d.send(gk208::transfer_request(23)).is_ok());
    let Reap::Done(done)=d.reap() else {panic!("clean next transfer not reaped")};
    assert!(gpu_completion::matches_echo(done.fence,done.native_fence,done.echoed,done.fenced,23));
    assert_eq!(s.borrow().epoch,17);assert!(s.borrow().events.is_empty());
}

#[test]
fn uncertain_draw_publication_preserves_the_first_native_error_and_backing() {
    let (mut d,s)=device();s.borrow_mut().fail_publish=true;
    let draw=gk208::adapter::EncodedDraw {transfer:None,target:19,words:vec![0x20010367,0]};
    assert!(d.send(Request::draw(Tag::Sync(9001),9001,19,1,draw)).is_ok());
    assert!(d.needs_reset());
    assert_eq!(d.fault_reason(),Some("injected native publication failure"));
    assert!(matches!(d.reap(),Reap::Fault));assert_eq!(d.outstanding(),1);
    assert_eq!(d.fault_reason(),Some("injected native publication failure"));
    s.borrow_mut().fail_reset=true;d.begin_reset();
    assert!(!d.reset_done());assert_eq!(d.outstanding(),1);
}

#[test]
fn empty_draw_still_waits_for_a_fresh_engine_release() {
    let (mut d,s)=device();s.borrow_mut().expected_draw.clear();
    let draw=gk208::adapter::EncodedDraw {transfer:None,target:19,words:vec![]};
    assert!(d.send(Request::draw(Tag::Sync(9001),9001,19,1,draw)).is_ok());
    assert!(matches!(d.reap(),Reap::Empty));assert_eq!(d.outstanding(),1);
    s.borrow_mut().release=1;s.borrow_mut().release_epoch=16;
    assert!(matches!(d.reap(),Reap::Empty));
    s.borrow_mut().release_epoch=17;
    let Reap::Done(done)=d.reap() else {panic!("empty draw has no fresh release")};
    assert_eq!((done.fence,done.native_fence,done.echoed),(Some(9001),Some(1),1));
    assert!(done.fenced);
}

#[test]
fn refusal_before_publication_returns_request_and_keeps_the_next_sequence_clean() {
    let (mut d,s)=device();s.borrow_mut().refuse_publish=true;
    let draw=gk208::adapter::EncodedDraw {transfer:None,target:19,words:vec![0x20010367,0]};
    let returned=d.send(Request::draw(Tag::Sync(9001),9001,19,1,draw));
    let Err(request)=returned else {panic!("software refusal escalated instead of returning the request")};
    assert_eq!(request.tag,Tag::Sync(9001));assert_eq!(request.fence,Some(9001));
    assert!(!d.needs_reset());assert_eq!(d.fault_reason(),None);
    assert_eq!(d.outstanding(),0);assert!(matches!(d.reap(),Reap::Empty));
    assert!(s.borrow().published.is_empty());assert!(s.borrow().events.is_empty());
    s.borrow_mut().refuse_publish=false;
    send(&mut d,23);assert_eq!(s.borrow().published,[1]);
    s.borrow_mut().release=1;
    assert!(matches!(d.reap(),Reap::Done(_)));
}

#[test]
fn encoded_stream_copy_uses_ce_completion_and_keeps_the_caller_fence() {
    let (mut d,s)=device();
    let image=gk208::copy::Image {address:1<<32,width:16,height:8,texel_bytes:4,pitch:64,block_height:2,bytes:1024};
    let plan=gk208::copy::region(image,gk208::copy::Image {address:2<<32,..image},[0,0,8,4,2,1],None).unwrap();
    let chunk=gk208::adapter::EncodedDraw {target:19,words:vec![],transfer:Some(plan)};
    assert!(d.send(Request::draw(Tag::Sync(8123),8123,19,1,chunk)).is_ok());
    assert_eq!(s.borrow().operations,["transfer"]);
    assert!(matches!(d.reap(),Reap::Empty));
    s.borrow_mut().release=1;
    let Reap::Done(done)=d.reap() else {panic!("CE release must retire the copy");};
    assert_eq!(done.fence,Some(8123));assert_eq!(done.native_fence,Some(1));
}

// The model exposes actual PGRAPH status/identity/method registers to the
// production decoder. Acknowledgement does not invent the engine's release.
impl gk208::gr::error::Io for State {
    fn read(&mut self,address:usize)->u32 {*self.registers.get(&address).unwrap_or(&0)}
    fn write(&mut self,address:usize,value:u32) {
        self.writes.push((address,value));
        if address==0x400100 { *self.registers.entry(address).or_default() &= !value; }
        else {self.registers.insert(address,value);}
    }
}
fn illegal_method(s:&Rc<RefCell<State>>,sequence:u32) {
    s.borrow_mut().registers.extend([(0x400100,0x10),(0x409b00,0x123),
        (0x400704,0x148),(0x400708,sequence),(0x404200,0xa197)]);
}
#[test]
fn undefined_method_is_not_a197_and_release_follows_the_trap() {
    let words=gk208::gr::error::commands(0x100000000,17,3).unwrap();
    assert_eq!(&words[..6],&[0x20010000,0xa197,0x20010044,0,0x20010052,3]);
    // Ordered A197 semaphore writes come after the illegal method, so ack
    // alone cannot certify DMA cessation or release a held request.
    assert_eq!(words.len(),21);
    assert_eq!(words[6],0x200406c0);
    assert_eq!(&words[17..],&[1,0,3,0x1000f010]);
}
#[test]
fn illegal_method_retires_only_its_fence_after_release_and_keeps_other_context_live() {
    use std::sync::Arc;
    let (mut d,s)=device();
    let bad=Request::undefined_command(Tag::Sync(9001),7,9001).unwrap();
    assert!(d.send(bad).is_ok());
    send(&mut d,23); // another core context's request on the shared native channel
    let pages=Arc::new(gpu::dma::Pages);
    assert!(d.send(Request::storage(Tag::Sync(24),Some(pages.clone()))).is_ok());
    illegal_method(&s,1);
    assert!(matches!(d.reap(),Reap::Empty));
    assert_eq!(s.borrow().writes,[(0x400100,0x10),(0x400500,0x10001)]);
    assert_eq!(d.outstanding(),3);assert_eq!(Arc::strong_count(&pages),2);
    assert!(!d.needs_reset());
    s.borrow_mut().release=1;
    let Reap::Done(done)=d.reap() else {panic!("illegal-method receipt absent")};
    assert_eq!((done.tag,done.fence,done.native_fence,done.answer,done.echoed,done.fenced),
        (Tag::Sync(9001),Some(9001),Some(1),0x10,1,true));
    assert_ne!(done.answer,gpu_abi::gk208::SUCCESS); // shared core maps this to DEVICE_ERROR
    assert_eq!(d.outstanding(),2);assert_eq!(Arc::strong_count(&pages),2);
    assert!(matches!(d.reap(),Reap::Empty));
    s.borrow_mut().release=2;
    let Reap::Done(done)=d.reap() else {panic!("surviving context did not finish")};
    assert_eq!((done.fence,done.answer),(Some(23),gpu_abi::gk208::SUCCESS));
    assert!(matches!(d.reap(),Reap::Done(_)));assert_eq!(Arc::strong_count(&pages),1);
    assert!(s.borrow().events.is_empty(),"a recoverable illegal method must not reset peers");
}
#[test]
fn foreign_or_unexpected_trap_never_retires_or_acknowledges_and_failed_reset_retains_holds() {
    use std::sync::Arc;
    for (address,value) in [(0x409b00,0x124),(0x400708,2),(0x400704,0x150),
        (0x400704,0x10148),(0x404200,0xa097),(0x400100,0x100010),(0x400108,1)] {
        let (mut d,s)=device();
        assert!(d.send(Request::undefined_command(Tag::Sync(9001),7,9001).unwrap()).is_ok());
        let pages=Arc::new(gpu::dma::Pages);
        assert!(d.send(Request::storage(Tag::Sync(23),Some(pages.clone()))).is_ok());
        illegal_method(&s,1);s.borrow_mut().registers.insert(address,value);
        s.borrow_mut().release=1;
        assert!(matches!(d.reap(),Reap::Fault),"foreign trap {address:x}={value:x}");
        assert_eq!(d.outstanding(),2);assert!(s.borrow().writes.is_empty());
        s.borrow_mut().fail_reset=true;d.begin_reset();assert!(!d.reset_done());
        let (held,result)=d.restart();assert!(held.is_empty());assert!(result.is_err());
        assert_eq!(Arc::strong_count(&pages),2);assert_eq!(d.outstanding(),2);
    }
}
#[test]
fn later_illegal_method_does_not_mark_an_earlier_clean_request_failed() {
    let (mut d,s)=device();send(&mut d,23);
    assert!(d.send(Request::undefined_command(Tag::Sync(9001),7,9001).unwrap()).is_ok());
    illegal_method(&s,2);s.borrow_mut().release=1;
    let Reap::Done(done)=d.reap() else {panic!("earlier clean receipt absent")};
    assert_eq!((done.fence,done.answer),(Some(23),gpu_abi::gk208::SUCCESS));
    assert!(matches!(d.reap(),Reap::Empty));
    s.borrow_mut().release=2;
    let Reap::Done(done)=d.reap() else {panic!("error receipt absent")};
    assert_eq!((done.fence,done.answer),(Some(9001),0x10));
}
