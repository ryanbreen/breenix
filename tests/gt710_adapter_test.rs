//! Exercise the production adapter with a controlled native engine boundary.
extern crate alloc;
use std::{cell::RefCell, rc::Rc};
#[path = "../kernel/src/gpu/completion.rs"] pub mod gpu_completion;
pub mod gpu {
    pub mod dma { pub struct Pages; }
    pub mod dev {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Tag { Background, Sync(u64), Submit { ctx:u32, serial:u64, last:bool } }
        pub struct Done {pub tag:Tag,pub fence:Option<u64>,pub native_fence:Option<u64>,pub answer:u32,pub echoed:u64,pub fenced:bool}
        pub enum Reap {Empty,Done(Done),Fault,Reset(Tag)}
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
                    let mut s=self.0.borrow_mut();
                    if s.trap_after_samples>0 {s.trap_after_samples-=1;}
                    if s.trap_at_sample || s.trap_after_samples==0 && s.delayed_trap_sequence!=0 {
                        s.trap_at_sample=false;let sequence=s.delayed_trap_sequence.max(1);
                        s.delayed_trap_sequence=0;s.plant_mmu(sequence);return Err("MMU fault during sample");
                    }
                    Ok(CeSample{context:s.channel,epoch:s.release_epoch,sequence:s.release,consumed:sequence,healthy:!s.fault})
                }
                pub fn error_trap(&self)->Result<Option<crate::gk208::gr::error::Trap>,&'static str> {
                    crate::gk208::gr::error::capture(&mut *self.0.borrow_mut())
                }
                pub fn acknowledge_error(&mut self,trap:crate::gk208::gr::error::Trap,sequence:u32)->Result<(),&'static str> {
                    assert_eq!(trap.page,sequence % 32768);
                    crate::gk208::gr::error::acknowledge(&mut *self.0.borrow_mut(),trap,0,0)
                }
                pub fn transfer_drain(&mut self,sequence:u32)->Result<CeSample,&'static str> {
                    let mut sample=self.sample(sequence)?;sample.sequence=self.0.borrow().drain;Ok(sample)
                }
                pub fn published(&self,sequence:u32)->bool {self.0.borrow().published.contains(&sequence)}
                pub fn healthy(&self)->Result<(), &'static str> {
                    let mut s=self.0.borrow_mut();
                    if s.trap_at_health {s.trap_at_health=false;s.plant_mmu(1);}
                    if s.fault {Err("injected engine fault")}
                    else if s.registers.get(&0x259c).copied().unwrap_or(0)!=0 {Err("MMU fault during health")}
                    else {Ok(())}
                }
                pub fn reset(&mut self)->Result<(),&'static str> {
                    let mut s=self.0.borrow_mut();s.events.push("reset");
                    if s.fail_reset {return Err("reset failed");}
                    // Native boundary: a confirmed GR stop, empty runlist,
                    // scheduler-error clear and unblock. No engine release.
                    gk208_reset(&mut s);
                    s.events.extend(["confirm","remove","unbind"]);
                    if s.fail_unbind {return Err("unbind failed");}
                    s.events.push("restore");s.stopped=true;Ok(())
                }
                pub fn restart(&mut self)->Result<(),&'static str> {
                    let mut s=self.0.borrow_mut();assert!(s.stopped);s.epoch+=1;s.release=0;s.fault=false;s.stopped=false;Ok(())
                }
            }
            fn gk208_reset(s:&mut State) {
                use crate::gk208::gr::error::Io;
                s.write(0x400500,0);
                s.write(0x200,0);
                s.write(0x200,0x1000);
                s.write(0x2270,0x456);
                s.write(0x2274,0);
                s.write(0x262c,1);
                s.write(0x2630,0);
                s.registers.insert(0x2640,0);
            }
        }
    }
    #[path = "caps.rs"] pub mod caps;
    #[path = "adapter.rs"] pub mod adapter;
}
use gpu::dev::{Reap,Tag};
use gk208::{adapter::{Device,Request},copy::Rect,gr::channel::NativeChannel};
pub struct State {
    now:u64,epoch:u64,channel:u64,release_epoch:u64,release:u32,drain:u32,fault:bool,
    transient_trap:bool,refusals:Vec<[u32;4]>,registers:std::collections::BTreeMap<usize,u32>,writes:Vec<(usize,u32)>,
    trap_at_health:bool,trap_at_sample:bool,fail_ack:bool,trap_after_samples:u32,delayed_trap_sequence:u32,
    fail_reset:bool,fail_unbind:bool,fail_publish:bool,refuse_publish:bool,stopped:bool,operations:Vec<&'static str>,published:Vec<u32>,events:Vec<&'static str>,expected_draw:Vec<u32>,
}
fn device()->(Device,Rc<RefCell<State>>) {
    let s=Rc::new(RefCell::new(State{now:0,epoch:17,channel:0x123000,release_epoch:17,release:0,drain:0,fault:false,
        transient_trap:false,refusals:vec![],registers:Default::default(),writes:vec![],trap_at_health:false,trap_at_sample:false,fail_ack:false,trap_after_samples:0,delayed_trap_sequence:0,fail_reset:false,fail_unbind:false,fail_publish:false,refuse_publish:false,stopped:false,operations:vec![],published:vec![],events:vec![],expected_draw:vec![0x20010367,0]}));
    (Device::new(NativeChannel(s.clone()),gk208::caps::BoardState::empty()),s)
}
// The model exposes actual PFIFO MMU status/instance/address registers to the
// production decoder. Acknowledgement does not invent the engine's release.
impl gk208::gr::error::Io for State {
    fn now_us(&mut self)->u64 {self.now}
    fn delay_us(&mut self,us:u64) {self.now+=us;}
    fn refuse_trap(&mut self, words:[u32;4])->Result<(),&'static str> {
        assert_ne!(words,[0;4]);self.refusals.push(words);self.events.push("trap-record");Ok(())
    }
    fn read(&mut self,address:usize)->u32 {
        let value=*self.registers.get(&address).unwrap_or(&0);
        if self.transient_trap && [0x400100,0x400108,0x400110,0x400118].contains(&address) {self.registers.remove(&address);}
        value
    }
    fn write(&mut self,address:usize,value:u32) {
        self.writes.push((address,value));
        if address==0x259c { if !self.fail_ack {*self.registers.entry(address).or_default() &= !value;} }
        else {self.registers.insert(address,value);}
    }
}
impl State {
    fn plant_mmu(&mut self,sequence:u32) {
        let address=0xfff8000000u64+u64::from(sequence % 32768)*4096;
        self.registers.extend([(0x259c,1),(0x2800,0x123),
            (0x2804,address as u32),(0x2808,(address>>32) as u32),(0x280c,0xc0),(0x262c,1)]);
    }
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

fn mmu_fault(s:&Rc<RefCell<State>>,sequence:u32) {
    s.borrow_mut().plant_mmu(sequence);
}
fn recover(d:&mut Device) {
    assert!(d.recovery_pending());
    d.begin_reset();assert!(d.reset_done());
    let (held,result)=d.restart();assert!(result.is_ok());assert!(held.is_empty());
}

#[test]
fn g19_mmu_fault_without_release_stops_offender_and_retires_inside_adapter() {
    use std::sync::Arc;
    let (mut d,s)=device();
    let bad_tag=Tag::Submit {ctx:7,serial:9001,last:true};
    assert!(d.send(Request::undefined_command(bad_tag,7,9001).unwrap()).is_ok());
    let peer_tag=Tag::Submit {ctx:8,serial:23,last:true};
    let mut peer=request(23);peer.tag=peer_tag;
    assert!(d.send(peer).is_ok());
    let pages=Arc::new(gpu::dma::Pages);
    assert!(d.send(Request::storage(Tag::Sync(24),Some(pages.clone()))).is_ok());
    mmu_fault(&s,1);
    assert!(matches!(d.reap(),Reap::Empty));
    // kgpu drives the adapter's existing reset/restart boundary outside CORE.
    // An MMU acknowledgement is followed by confirmed reset, not a post-fault release.
    assert_eq!(s.borrow().release,0);
    d.begin_reset();assert!(d.reset_done());
    let (held,result)=d.restart();assert!(result.is_ok());
    assert!(held.is_empty(),"attributed faults must retire locally, without whole-device reset");
    assert_eq!(&s.borrow().writes[..3],&[(0x2630,1),(0x259c,1),(0x800004,0x800)],
        "block, acknowledge, disable before GR reset");
    {let state=s.borrow();let writes=&state.writes;
    let reset=writes.iter().position(|w| *w==(0x200,0)).unwrap();
    let commit=writes.iter().position(|w| *w==(0x2274,0)).unwrap();
    let clear=writes.iter().position(|w| *w==(0x262c,1)).unwrap();
    let unblock=writes.iter().position(|w| *w==(0x2630,0)).unwrap();
    assert!(reset<commit && commit<clear && clear<unblock);}
    let Reap::Done(done)=d.reap() else {panic!("fault must retire without a later engine release")};
    assert_eq!((done.tag,done.answer),(bad_tag,0x60010000));
    assert_ne!(done.answer,gpu_abi::gk208::SUCCESS);
    assert!(matches!(d.reap(),Reap::Reset(tag) if tag==peer_tag),
        "unconfirmed peer work must report the reset epoch, never fabricated success");
    assert!(matches!(d.reap(),Reap::Done(_)));assert_eq!(Arc::strong_count(&pages),1);
    assert_eq!(d.outstanding(),0);assert!(!d.needs_reset());
    // The peer keeps its resources/VM; fresh work requires the new epoch.
    send(&mut d,25);s.borrow_mut().release=1;
    assert!(matches!(d.reap(),Reap::Empty));
    s.borrow_mut().release_epoch=18;
    assert!(matches!(d.reap(),Reap::Done(done) if done.answer==gpu_abi::gk208::SUCCESS));
}
#[test]
fn unmapped_write_and_release_follows_the_fault() {
    let words=gk208::gr::error::commands(0x100000000,17,3).unwrap();
    // The first engine-ordered write is to the absent final 128 MiB PDE.
    assert_eq!(&words[..9],&[0x20010000,0xa197,0x20010044,0,
        0x200406c0,0xff,0xf8003000,3,0x1000f010]);
    // The three valid receipt words follow; a fault flag alone cannot retire backing.
    assert_eq!(words.len(),24);
    assert_eq!(&words[9..14],&[0x200406c0,1,4,17,0x1000f010]);
    assert_eq!(&words[19..],&[0x200406c0,1,0,3,0x1000f010]);
}
#[test]
fn g19_failed_ack_reset_or_unbind_retains_all_requests_and_holds() {
    use std::sync::Arc;
    for failure in 0..3 {
        let (mut d,s)=device();
        assert!(d.send(Request::undefined_command(Tag::Submit {ctx:7,serial:9001,last:true},7,9001).unwrap()).is_ok());
        let pages=Arc::new(gpu::dma::Pages);
        assert!(d.send(Request::storage(Tag::Sync(24),Some(pages.clone()))).is_ok());
        mmu_fault(&s,1);
        assert!(matches!(d.reap(),Reap::Empty));
        {let mut s=s.borrow_mut();s.fail_ack=failure==0;s.fail_reset=failure==1;s.fail_unbind=failure==2;}
        d.begin_reset();assert!(!d.reset_done());
        let (held,result)=d.restart();assert!(result.is_err());assert!(held.is_empty());
        assert_eq!(d.outstanding(),2);assert_eq!(Arc::strong_count(&pages),2);
        assert!(matches!(d.reap(),Reap::Empty));assert!(!d.has_room(1));
        assert_eq!(s.borrow().release,0);
    }
}

#[test]
fn foreign_or_unexpected_trap_never_retires_or_acknowledges_and_failed_reset_retains_holds() {
    use std::sync::Arc;
    for (address,value) in [(0x2800,0x124),(0x2804,0xf8000008),(0x2808,0xfe),
        (0x280c,0x40),(0x280c,0xc5),(0x259c,3),(0x400100,0x10)] {
        let (mut d,s)=device();
        assert!(d.send(Request::undefined_command(Tag::Sync(9001),7,9001).unwrap()).is_ok());
        let pages=Arc::new(gpu::dma::Pages);
        assert!(d.send(Request::storage(Tag::Sync(23),Some(pages.clone()))).is_ok());
        mmu_fault(&s,1);s.borrow_mut().registers.insert(address,value);
        s.borrow_mut().release=1;
        assert!(matches!(d.reap(),Reap::Fault),"foreign trap {address:x}={value:x}");
        assert_eq!(d.outstanding(),2);assert!(s.borrow().writes.is_empty());
        s.borrow_mut().fail_reset=true;d.begin_reset();assert!(!d.reset_done());
        let (held,result)=d.restart();assert!(held.is_empty());assert!(result.is_err());
        assert_eq!(Arc::strong_count(&pages),2);assert_eq!(d.outstanding(),2);
    }
}
#[test]
fn g19_uncertain_earlier_peer_reports_reset_instead_of_replaying() {
    let (mut d,s)=device();send(&mut d,23);
    assert!(d.send(Request::undefined_command(Tag::Sync(9001),7,9001).unwrap()).is_ok());
    mmu_fault(&s,2);
    assert!(matches!(d.reap(),Reap::Empty));recover(&mut d);
    assert!(matches!(d.reap(),Reap::Done(done) if done.answer==0x60010000 && done.tag==Tag::Sync(9001)));
    assert!(matches!(d.reap(),Reap::Reset(Tag::Sync(23))));
    assert_eq!(d.outstanding(),0);
}

#[test]
fn g19_trap_arriving_during_health_or_sample_recovers_outside_reap() {
    for during_sample in [false,true] {
        let (mut d,s)=device();
        assert!(d.send(Request::undefined_command(Tag::Sync(9001),7,9001).unwrap()).is_ok());send(&mut d,23);
        s.borrow_mut().trap_at_sample=during_sample;s.borrow_mut().trap_at_health=!during_sample;
        assert!(matches!(d.reap(),Reap::Empty));
        assert!(s.borrow().writes.is_empty(),"CORE reaping must not reset or wait on hardware");
        assert_eq!(d.outstanding(),2);assert!(!d.needs_reset());recover(&mut d);
        assert!(matches!(d.reap(),Reap::Done(done) if done.tag==Tag::Sync(9001) && done.answer==0x60010000));
        assert!(matches!(d.reap(),Reap::Reset(Tag::Sync(23))));
        assert_eq!(s.borrow().release,0);
    }
}

#[test]
fn missing_trap_or_failed_ack_never_returns_a_successful_error_control() {
    for missing_trap in [false,true] {
        let (mut d,s)=device();
        assert!(d.send(Request::undefined_command(Tag::Sync(9001),7,9001).unwrap()).is_ok());
        if !missing_trap {mmu_fault(&s,1);s.borrow_mut().fail_ack=true;}
        s.borrow_mut().release=1;
        if missing_trap {assert!(matches!(d.reap(),Reap::Fault));}
        else {
            assert!(matches!(d.reap(),Reap::Empty));d.begin_reset();assert!(!d.reset_done());
            assert!(d.restart().1.is_err());assert!(matches!(d.reap(),Reap::Empty));
        }
        assert_eq!(d.outstanding(),1);
    }
}

#[test]
fn g19_later_mmu_fault_during_transfer_drain_reports_uncertain_peer_reset() {
    for sample_number in [2,3] {
        let (mut d,s)=device();assert!(d.send(gk208::transfer_request(23)).is_ok());
        assert!(d.send(Request::undefined_command(Tag::Sync(9001),7,9001).unwrap()).is_ok());
        {let mut s=s.borrow_mut();s.release=3;s.drain=1;s.trap_after_samples=sample_number;s.delayed_trap_sequence=2;}
        assert!(matches!(d.reap(),Reap::Empty));assert_eq!(d.outstanding(),2);
        assert!(s.borrow().writes.is_empty());assert!(!d.needs_reset());recover(&mut d);
        assert!(matches!(d.reap(),Reap::Done(done) if done.tag==Tag::Sync(9001) && done.answer==0x60010000));
        assert!(matches!(d.reap(),Reap::Reset(Tag::Sync(23))));
        assert_eq!(d.outstanding(),0);
    }
}

#[test]
fn mmu_control_pages_survive_hardware_alignment_and_sequence_wrap() {
    for sequence in [1, 893, 2837, 3735, 4328, 32767, 32768, 32769, u32::MAX] {
        let address = gk208::gr::error::address(sequence).unwrap();
        assert_eq!(address, 0xfff8000000 + u64::from(sequence % 32768) * 4096);
        assert_eq!(address & 4095, 0);
        assert_eq!(address / (1 << 27), 8191);
        let (_,s) = device();
        let mut d = Device::registered(NativeChannel(s.clone()), gk208::caps::BoardState::empty(), sequence-1);
        assert!(d.send(Request::undefined_command(Tag::Sync(9001),7,9001).unwrap()).is_ok());
        mmu_fault(&s,sequence);
        assert!(matches!(d.reap(),Reap::Empty));
        assert_eq!(d.outstanding(),1);
        recover(&mut d);
        let Reap::Done(done)=d.reap() else {panic!("page-aligned fault lost its pending identity")};
        assert_eq!((done.tag,done.answer),(Tag::Sync(9001),0x60010000));
        assert!(!done.fenced);assert_eq!(done.native_fence,None);
    }
    for sequence in [32766,32767,32768] {
        let pages: std::collections::BTreeSet<_> = (sequence..sequence+4)
            .map(|s|gk208::gr::error::address(s).unwrap()).collect();
        assert_eq!(pages.len(),4,"four native requests must have distinct tokens");
    }
}

#[test]
fn g19_refusal_keeps_all_four_transient_pgraph_words() {
    let (_,s)=device();let mut s=s.borrow_mut();
    s.transient_trap=true;
    s.registers.extend([(0x400100,1),(0x400108,2),(0x400110,3),(0x400118,4)]);
    assert!(gk208::gr::error::capture(&mut *s).is_err());
    assert_eq!(s.refusals,[[1,2,3,4]]);
    assert_eq!(gk208::gr::error::capture(&mut *s),Ok(None));
    assert_eq!(s.refusals,[[1,2,3,4]]);
}

#[test]
fn pri_sampling_precedes_mmu_attribution_and_retains_persistent_failure() {
    let (_,s)=device();let mut s=s.borrow_mut();
    s.plant_mmu(1);s.transient_trap=true;
    s.registers.insert(0x400100,0xbadf1201);
    let trap=gk208::gr::error::capture(&mut *s).unwrap().unwrap();
    assert_eq!(trap.status,0x60010000);assert_eq!(s.now,10);assert!(s.refusals.is_empty());
    s.transient_trap=false;s.registers.insert(0x400100,0xbadf1201);
    let start=s.now;
    assert_eq!(gk208::gr::error::capture(&mut *s),Err("GR PRI unreachable"));
    assert_eq!(s.now-start,1000);assert_eq!(s.refusals,[[0xbadf1201,0,0,0]]);
    assert!(s.writes.is_empty(),"PRI sampling must not acknowledge the MMU fault");
}
