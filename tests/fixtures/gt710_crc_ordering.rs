// Host boundaries: deterministic device receipt, resources, event wake and user memory.
// The session producers, adapter flip retirement, transition predicates and CRC
// entry/readback selection are extracted from production by the Python harness.
use std::{cell::{Cell, RefCell}, sync::Mutex};
const ENODEV:i64=19;const EIO:i64=5;const EPERM:i64=1;const EINTR:i64=4;const ETIMEDOUT:i64=110;
const OBSERVATION_TIMEOUT_NS:u64=5_000_000_000;
mod abi {
 pub mod backend {pub const GK208:u32=6;}
 pub mod screen_crc {pub const MAX_RECORD:usize=4096;}
 pub struct Readback {pub pixels:u64,pub pixels_len:u64,pub record:u64}
 impl Readback {pub const ZERO:Self=Self{pixels:0,pixels_len:0,record:0};}
 pub mod screen {
  #[derive(Clone,Copy)]pub struct CrcRequest<'a> {pub pixels:u64,pub len:u64,pub record:u64,pub observation:u64,pub case:&'a str}
 }
}
mod dev {pub fn backend()->u32{6}}
mod console {pub fn await_repaint(){}}
pub mod signal {pub fn monotonic_nanos()->u64{100}}
struct GpuPath;
impl GpuPath {fn enter()->Self{Self}}
fn probe_writable(_:u64,_:usize)->Result<(),i64>{Ok(())}
fn screen_proof_enabled()->bool{true}
fn caller()->Result<u64,i64>{Ok(7)}
fn now_ns()->u64{100}
mod readback_reason {pub const BINDING_UNKNOWN:u32=10;}
#[derive(Debug,PartialEq)]enum Refused{Errno(i64),Eio(u32,u64)}
impl From<i64> for Refused {fn from(e:i64)->Self{Self::Errno(e)}}
#[derive(Clone,Copy,PartialEq)]enum Observer{Owner,Window,Screen}
mod present {
 #[derive(Clone,Copy,Debug,PartialEq)]pub struct Surface {pub resource:u32}
 #[derive(Clone,Copy)]pub struct Receipt {pub notifier:u32,pub active:[u32;5],pub armed:[u32;5]}
}
struct Registers(usize);
struct Range {start:u64,size:u64}
struct Object {name:&'static str,range:Range}
struct Layout {objects:Vec<Object>}
struct Console {vram:u64,bytes:usize}
struct Resources;
impl Resources {
 fn hold(&self,id:u32)->Result<u32,&'static str>{Ok(id)}
 fn scanout_allocation(&self,_:u32)->Option<Range>{Some(Range{start:0,size:4096})}
 fn scanout(&self,id:u32)->Option<present::Surface>{Some(present::Surface{resource:id})}
}
mod display_crc {
 pub struct Checkpoint<'a>(pub &'a str);
 pub struct Binding{pub resource:u64}
 #[derive(Debug,PartialEq)]pub struct Record;
 pub struct Perturbation;
}
struct Session {pending_present:Option<present::Surface>,ready_present:Option<present::Receipt>,work:Option<()>,
 scanout:Vec<u32>,evo:native::Evo,registers:usize,resources:Resources,mode:u32,console:Console,layout:Layout,
 output_perturbation:display_crc::Perturbation}
struct Owner(Mutex<Option<Session>>);
impl Owner {fn lock(&self)->std::sync::MutexGuard<'_,Option<Session>>{self.0.lock().unwrap()}}
static SESSION:Owner=Owner(Mutex::new(None));
thread_local! {
 static READY:Cell<bool>=const{Cell::new(false)};
 static POLLS:Cell<u32>=const{Cell::new(0)};
 static CAPTURES:Cell<u32>=const{Cell::new(0)};
 static COMPLETE_AT:Cell<u32>=const{Cell::new(3)};
}
mod native {
 use super::*;
 pub struct Evo{pub head:u32}
 fn receipt()->present::Receipt{present::Receipt{notifier:1,active:[7;5],armed:[7;5]}}
 pub fn present_accepted(_: &mut Evo,r:&mut Registers,_:present::Surface,_:u32)->Result<Option<present::Receipt>,&'static str>{
  assert_eq!(r.0,0);Ok(READY.with(Cell::get).then(receipt))
 }
 pub fn present_latched(_: &mut Evo,_:&mut Registers,_:present::Surface,_:u32)->Result<Option<present::Receipt>,&'static str>{
  Ok((POLLS.with(Cell::get)>=COMPLETE_AT.with(Cell::get)).then(receipt))
 }
 pub fn wait_present(e:&mut Evo,r:&mut Registers,s:present::Surface,m:u32)->Result<present::Receipt,&'static str>{
  present_latched(e,r,s,m)?.ok_or("incomplete latch")
 }
 pub fn capture_crc(_: &mut Evo,_:&mut Registers,notifier:u64,s:present::Surface,c:display_crc::Checkpoint<'_>,b:display_crc::Binding,
  mode:[u32;2],index:u32,_:&mut display_crc::Perturbation)->Result<display_crc::Record,&'static str>{
  assert_eq!(notifier,4096);assert_eq!(s.resource,b.resource as u32);assert_eq!(c.0,"checkpoint");
  assert_eq!(mode,[1280,1024]);assert_eq!(index,1);
  assert_eq!(with_core(|c|c.display.outstanding),Some(0),"CRC reused a live present notifier");
  CAPTURES.with(|c|c.set(c.get()+1));Ok(display_crc::Record)
 }
}
mod display_evidence {
 pub fn transition(_: &mut super::Registers,_:u32,_:super::present::Surface,_:&super::present::Receipt,_:[u64;2],_:[u64;2],_:&str,_:u32)->Result<(),&'static str>{Ok(())}
}
// SESSION_FUNCTIONS
struct Channel;
impl Channel{fn present_complete(&mut self,s:present::Surface)->Result<bool,&'static str>{present_complete(s)}}
struct Request {scanout:Option<present::Surface>,tag:u64}
#[derive(Clone,Copy)]struct Identity {caller:u64,sequence:u32}
struct Sent {request:Request,identity:Option<Identity>}
struct Sample {sequence:u32}
struct Done {tag:u64,fence:Option<u64>,native_fence:Option<u64>,answer:u32,echoed:u64,fenced:bool}
enum Reap {Done(Done),Empty,Fault}
mod gpu_abi {pub mod gk208 {pub const SUCCESS:u32=0x60000000;}}
struct Adapter {channel:Channel,flipping:Option<(Sent,Sample)>,first_error:Option<&'static str>,fault:bool}
impl Adapter {
 fn reap(&mut self)->Reap {
 // REAP
 Reap::Empty
 }
}
thread_local! {static ADAPTER:RefCell<Adapter>=const{RefCell::new(Adapter{channel:Channel,flipping:None,first_error:None,fault:false})};}
struct Display {changing:bool,outstanding:u32,known:bool,accepted:u64,sent:u64}
struct Core {display:Display}
impl Core {fn admission(&self)->Result<(),()>{Ok(())}}
thread_local! {static CORE:RefCell<Core>=const{RefCell::new(Core{display:Display{changing:false,outstanding:0,known:true,accepted:1,sent:1}})};}
fn with_core<T>(f:impl FnOnce(&mut Core)->T)->Option<T>{CORE.with(|c|Some(f(&mut c.borrow_mut())))}
#[derive(Debug)]enum Stop{Timeout,Interrupted}
fn wait_for<T>(deadline:Option<u64>,_:bool,mut f:impl FnMut(&mut Core)->Option<T>)->Result<T,Stop>{
 assert!(deadline==Some(200)||deadline==Some(5_000_000_100)||deadline.is_none());
 for _ in 0..4 {
  POLLS.with(|n|n.set(n.get()+1));
  // Force the first core poll between GET acceptance and the reaper's next wake.
  if POLLS.with(Cell::get)>1 {
   ADAPTER.with(|a|match a.borrow_mut().reap(){
    Reap::Done(d)=>{assert_eq!((d.tag,d.fence,d.native_fence,d.answer,d.echoed,d.fenced),(9,Some(11),Some(13),0x60000000,13,true));with_core(|c|c.display.outstanding-=1);},
    Reap::Empty=>(),Reap::Fault=>panic!("adapter fault"),
   });
  }
  if let Some(found)=with_core(&mut f).flatten(){return Ok(found)}
 }
 if deadline.is_some(){Err(Stop::Timeout)}else{Err(Stop::Interrupted)}
}
fn owner(group:u64)->bool{group==7}
pub mod gk208 {pub mod timing {pub fn scanout_wait(_:bool,_:u64){}}}
mod display {
 use super::*;
 pub struct Transition;
 impl Transition {
 // TRANSITION_METHODS
 }
 impl Drop for Transition {fn drop(&mut self){with_core(|c|c.display.changing=false);}}
}
fn read_back_impl(tgid:u64,a:&mut abi::Readback,observer:Observer,arg:Option<u64>,crc:Option<abi::screen::CrcRequest<'_>>)->Result<u64,Refused>{
 assert_eq!((tgid,a.pixels,a.pixels_len,a.record,arg),(7,1,4,3,None));
 let window=observer!=Observer::Owner;let deadline=Some(200);
 let _scanout = // SELECTION
 ;
 assert!(with_core(|c|c.display.changing).unwrap());
 let request=crc.unwrap();assert_eq!(request.case,"checkpoint");
 capture_crc(display_crc::Checkpoint(request.case),display_crc::Binding{resource:7},[1280,1024]).map_err(|_|Refused::Errno(EIO))?;
 Ok(1)
}
fn observe_screen_crc(request:abi::screen::CrcRequest<'_>)->Result<(),Refused>{
 // ENTRY
}
fn setup(ready:bool){
 READY.with(|r|r.set(ready));POLLS.with(|p|p.set(0));CAPTURES.with(|c|c.set(0));COMPLETE_AT.with(|n|n.set(3));
 *SESSION.lock()=Some(Session{pending_present:None,ready_present:None,work:None,scanout:vec![6],evo:native::Evo{head:0},registers:0,
  resources:Resources,mode:1,console:Console{vram:0,bytes:4096},layout:Layout{objects:vec![Object{name:"dma",range:Range{start:4096,size:4096}}]},output_perturbation:display_crc::Perturbation});
 present(present::Surface{resource:7}).unwrap();
 assert!(SESSION.lock().as_ref().unwrap().pending_present.is_some());
 assert_eq!(SESSION.lock().as_ref().unwrap().ready_present.is_some(),ready);
 with_core(|c|{c.display.outstanding=1;c.display.changing=false;});
 ADAPTER.with(|a|a.borrow_mut().flipping=Some((Sent{request:Request{scanout:Some(present::Surface{resource:7}),tag:9},identity:Some(Identity{caller:11,sequence:13})},Sample{sequence:13})));
}
fn request()->abi::screen::CrcRequest<'static>{abi::screen::CrcRequest{pixels:1,len:4,record:2,observation:3,case:"checkpoint"}}
#[test]fn crc_waits_for_incomplete_latch_and_completed_retained_receipt(){
 let mut failures=Vec::new();
 for ready in [false,true] {
  setup(ready);
  let result=observe_screen_crc(request());
  if result.is_err(){failures.push((ready,result));continue;}
  assert!(SESSION.lock().as_ref().unwrap().pending_present.is_none());
  assert_eq!(SESSION.lock().as_ref().unwrap().scanout,[7]);
  assert_eq!(CAPTURES.with(Cell::get),1);
  assert_eq!(POLLS.with(Cell::get),if ready{2}else{3});
 }
 assert!(failures.is_empty(),"CRC refused before adapter retirement: {failures:?}");
 // An incomplete latch times out before CRC touches its notifier.
 setup(false);COMPLETE_AT.with(|n|n.set(100));
 assert_eq!(observe_screen_crc(request()),Err(Refused::Errno(ETIMEDOUT)));
 assert!(SESSION.lock().as_ref().unwrap().pending_present.is_some());assert_eq!(CAPTURES.with(Cell::get),0);
 // Ordinary pixel-only screen observation retains its accepted-binding behavior.
 setup(false);assert!(display::Transition::for_window_observer(Some(200)).is_ok());
 assert_eq!(POLLS.with(Cell::get),1);
 // The native guard still refuses work or a pending present directly.
 assert!(capture_crc(display_crc::Checkpoint("checkpoint"),display_crc::Binding{resource:7},[1280,1024]).is_err());
 SESSION.lock().as_mut().unwrap().pending_present=None;
 SESSION.lock().as_mut().unwrap().work=Some(());
 assert!(capture_crc(display_crc::Checkpoint("checkpoint"),display_crc::Binding{resource:7},[1280,1024]).is_err());
}
