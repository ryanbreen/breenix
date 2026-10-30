#![allow(dead_code)] // Production slices include diagnostic APIs beyond this fixture.
extern crate alloc;
use std::{cell::RefCell, collections::{BTreeMap,BTreeSet}, ops::Range,
    sync::{Arc,atomic::{AtomicU64,Ordering}}};
use gpu_abi as abi;
use abi::{Plain,backend};
use display::owner;
const EIO:i64=5;const ENODEV:i64=19;const EPERM:i64=1;
#[derive(Clone,Copy)] struct LastWrite {ctx:u32,device_ctx:u32,fence:u64,serial:u64,answer:u32,kind:u32}
struct Resource {width:u32,height:u32,lost:bool,holds:usize,last:Option<LastWrite>,generation:u64,invalid:bool}
struct Core {display:display::State,resources:BTreeMap<u32,Resource>,released:Vec<u32>,next:u64}
impl Core {
 fn unhold(&mut self,id:u32){self.released.push(id);self.resources.get_mut(&id).unwrap().holds-=1;}
 fn device_fence(&mut self)->u64{self.next+=1;self.next}
 fn serial(&mut self)->u64{self.next+=1;self.next}
 // CORE_METHODS
}
#[derive(Default)] struct Host {events:Vec<(&'static str,u32)>,fail:bool,mode:u32,restores:usize,firmware:[u8;32]}
thread_local! {
 static CORE:RefCell<Core>=RefCell::new(Core{display:display::State::new(),resources:BTreeMap::new(),released:vec![],next:100});
 static HOST:RefCell<Host>=RefCell::new(Host{mode:1,firmware:[0x5a;32],..Host::default()});
}
fn with_core<R>(f:impl FnOnce(&mut Core)->R)->Option<R>{Some(CORE.with(|c|f(&mut c.borrow_mut())))}
fn event(k:&'static str,id:u32){HOST.with(|h|h.borrow_mut().events.push((k,id)));}
fn notify(){}
fn wait_for<R>(_:Option<u64>,_:bool,mut f:impl FnMut(&mut Core)->Option<R>)->Result<R,()>{with_core(|c|f(c).expect("unexpected blocked transition")).ok_or(())}
fn caller()->Result<u64,i64>{Ok(99)}
mod syscall {pub mod display {pub static DISPLAY_OWNER_GROUP:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(u64::MAX);pub static DISPLAY_OWNER_PID:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(u64::MAX);}}
mod console {
 pub fn give_back_display(){crate::event("redraw",0);}
 pub fn retire_firmware(){}
 pub fn handed_off(){}
 pub fn request_restore(_:u64){crate::HOST.with(|h|h.borrow_mut().restores+=1);}
 pub mod vt {
  pub struct Presentation;
  impl Presentation {pub fn begin(_:bool)->Option<Self>{Some(Self)}}
  pub fn give_back_display(){crate::event("vt-back",0);}
 }
}
mod arch {pub fn with_interrupts<R>(f:impl FnOnce()->R)->R{f()}}
mod gk208 {
 pub fn console_state()->Option<(crate::abi::Mode,u64,bool,u64)>{
  let low=crate::HOST.with(|h|h.borrow().mode==0);
  Some((crate::abi::Mode{width:if low{1280}else{2560},height:if low{800}else{1600},scale:if low{1}else{2},reserved:0},u32::MAX.into(),true,0))
 }
 pub fn restore_console()->Result<(),&'static str>{crate::event("legacy-restore",0);Ok(())}
 pub struct ConsoleSurface{pub pointer:usize,pub width:usize,pub height:usize,pub pitch:usize}
 pub fn set_mode(index:u32)->Result<(),&'static str>{crate::HOST.with(|h|h.borrow_mut().mode=index);crate::event("mode",index);Ok(())}
 struct Surface{width:u32,height:u32}
 struct Resources;
 impl Resources {fn scanout(&self,id:u32)->Option<Surface>{crate::with_core(|c|c.resources.get(&id).map(|r|Surface{width:r.width,height:r.height})).flatten()}}
 fn with_resources<R>(f:impl FnOnce(&mut Resources,&mut (),&mut ())->Result<R,&'static str>)->Result<R,&'static str>{f(&mut Resources,&mut (),&mut ())}
 fn flush_resource(_:crate::Tag,_:u32,fence:u64,id:u32,w:u32,h:u32)->Option<crate::Command>{
  crate::with_core(|c|{let r=c.resources.get(&id).unwrap();assert_eq!((w,h),(r.width,r.height));});
  Some(crate::Command{kind:"flush",id,fence})
 }
 // NATIVE_FLUSH_RECT

}
mod gpu {pub fn display_active()->bool{true}pub mod console{pub fn active()->bool{false}}}
mod raster {
 #[derive(Clone,Copy)] pub struct Font;
 impl Font {pub fn for_display(_:usize,_:usize)->Self{Self}}
 #[derive(Clone,Copy)] pub struct Layout {pub width:usize,pub height:usize,pub stride_bytes:usize,pub bytes_per_pixel:usize,pub reserved:u8,pub bgr:bool,pub font:Font}
 impl Layout {pub fn bytes(&self)->usize{self.stride_bytes*self.height}}
}
mod console_display {
 pub mod gpu_core {
  pub struct GpuCore;
  impl GpuCore {
   pub fn new()->Option<Self>{Some(Self)}
   pub fn write(&self,shadow:&[u8],lines:std::ops::Range<usize>,_:fn()->bool)->bool {
    assert_eq!(lines,0..2);assert_eq!(shadow,[0x33;32]);crate::event("backing-write",50);true
   }
  }
 }
 pub mod linear {
  use crate::{Ordering,raster::Layout};use std::{ops::Range,sync::{OnceLock,atomic::AtomicBool}};
  pub struct Linear {pub buffer:*mut u8,pub layout:Layout,pub native:OnceLock<crate::gk208::ConsoleSurface>,pub disabled:AtomicBool}
  unsafe fn write_bands(p:*mut u8,l:&Layout,shadow:&[u8],_:Range<usize>,allowed:fn()->bool)->bool{
   if !allowed(){return false;}unsafe{std::ptr::copy_nonoverlapping(shadow.as_ptr(),p,l.bytes());}true
  }
  impl Linear {
  // LINEAR_METHODS
  }
 }
}
mod display {
 use super::*;
 // DEFINITIONS
 pub(super) struct Transition;
 impl Transition {fn acquire()->Result<Self,i64>{Ok(Self)}}
 // DISPLAY_FUNCTIONS
}
#[derive(Clone,Copy)] enum Tag {Sync(u64)}
struct Command {kind:&'static str,id:u32,fence:u64}
enum Request {Gk208(Command),Svga(Command),Virgl(Command)}
struct Done {answer:u32,echoed:u64}
impl Done {fn matches_fence(&self,f:u64)->bool{self.echoed==f}}
fn exec_ok(build:impl FnOnce(Tag)->Option<Request>,_:u32,_:&[u32])->Result<Done,i64>{execute(build(Tag::Sync(1)).ok_or(EIO)?)}
fn exec_req(_:u64,req:Request,_:u32,_:&[u32],_:&[u32])->Result<Done,i64>{execute(req)}
fn execute(req:Request)->Result<Done,i64>{
 let Request::Gk208(c)=req else {panic!("wrong backend")};
 event(c.kind,c.id);
 if c.kind=="flip" && HOST.with(|h|h.borrow().fail){return Err(EIO);}
 Ok(Done{answer:0x60000000,echoed:c.fence})
}
struct Pages;
impl Pages {fn maintain(&self,_:usize,_:usize,_:bool){}}
mod dev {
 use super::*;
 fn backend()->u32{abi::backend::GK208}
 pub fn resource_id(id:u32)->Option<u32>{(id!=0).then_some(id+1000)}
 pub fn context_id(c:u32)->u32{c+1000}pub fn upload_kind()->u32{1}pub fn ok(a:u32)->bool{a==0x60000000}
 pub fn transfer_to_host(_:Tag,_:u32,fence:u64,id:u32,_:Arc<Pages>,_:u32,_:u32,_:u32,_:u32,_:u64,_:u32)->Option<Request>{Some(Request::Gk208(Command{kind:"upload",id,fence}))}
 pub fn set_scanout(_:Tag,_:u32,id:u32,_:u32,_:u32)->Option<Request>{Some(Request::Gk208(Command{kind:"bind",id,fence:0}))}
 pub fn flush_resource(_:Tag,_:u32,fence:u64,id:u32,_:u32,_:u32)->Option<Request>{Some(Request::Gk208(Command{kind:"flip",id,fence}))}
 // FLUSH_RECT
 mod svga {use super::*;pub fn flush_rect(_:Tag,_:u32,fence:u64,_:u32,_:u32,_:u32,_:u32)->Option<Command>{Some(Command{kind:"svga",id:0,fence})}}
 mod virgl {use super::*;pub fn flush_rect(_:Tag,_:u32,fence:u64,id:u32,_:u32,_:u32,_:u32,_:u32)->Command{Command{kind:"virgl",id,fence}}}
}
mod screen {
 use super::*;use display::Binding;
 const WIDTH:u32=2560;const HEIGHT:u32=1600;const PITCH:usize=WIDTH as usize*4;
 static RESOURCE:AtomicU64=AtomicU64::new(50);static PRESENTS:AtomicU64=AtomicU64::new(0);
 struct Target {context:u32,id:u32,pages:Arc<Pages>}
 impl Target {
  // SCREEN_METHODS
 }
 struct Lease(Target);
 impl Lease {
  fn take()->Self{Self(Target{context:7,id:50,pages:Arc::new(Pages)})}
  fn ensure(&mut self)->Result<&Target,i64>{Ok(&self.0)}
 }
 // SCREEN_FUNCTIONS
 pub fn reset_presents(){PRESENTS.store(0,Ordering::Release);}
 pub fn repaint_rows()->Result<(),i64>{let t=Lease::take();t.0.upload(3..7)?;t.0.flush(3..7)}
}
fn reset(){
 syscall::display::DISPLAY_OWNER_GROUP.store(2,Ordering::Release);
 with_core(|c|{c.display=display::State::new();c.resources.clear();c.released.clear();
  for id in [11,22,50]{c.resources.insert(id,Resource{width:2560,height:1600,lost:false,holds:4,last:None,generation:0,invalid:false});}
  c.display.stack=vec![display::Owner{group:1,pid:1,mode:1,last:Some(display::Binding{id:11,fence:41,answer:0x60000000})},display::Owner{group:2,pid:2,mode:0,last:Some(display::Binding{id:22,fence:42,answer:0x60000000})}];
  c.display.binding=Some(display::Binding{id:22,fence:42,answer:0x60000000});c.display.holds.insert(22);
 });
 HOST.with(|h|*h.borrow_mut()=Host{mode:0,firmware:[0x5a;32],..Host::default()});screen::reset_presents();
}
fn events()->Vec<(&'static str,u32)>{HOST.with(|h|h.borrow().events.clone())}
#[test] fn give_back_reflips_the_previous_target_and_records_its_identity(){
 reset();display::back(2).unwrap();
 assert!(events().contains(&("flip",11)),"give-back never flipped the saved native target");
 with_core(|c|{assert_eq!(c.display.stack.len(),1);assert_eq!(c.display.binding.unwrap().id,11);
  let b=display::binding(c,1,11,false).unwrap().unwrap();assert_eq!(dev::resource_id(b.id),Some(1011));assert_eq!(b.fence,41);assert!(c.display.known);});
 assert_eq!(HOST.with(|h|h.borrow().mode),1);
 // A failed latch must not publish a successful restore or drop the giver.
 reset();HOST.with(|h|h.borrow_mut().fail=true);assert_eq!(display::back(2),Err(EIO));
 with_core(|c|{assert_eq!(c.display.stack.len(),2);assert!(!c.display.known);});
 // Exit requests the same restoration after removing the ended top owner.
 reset();with_core(|c|display::ended(c,2));display::restore().unwrap();
 assert!(events().contains(&("flip",11)));assert_eq!(HOST.with(|h|h.borrow().restores),1);
}
#[test] fn kernel_return_uploads_binds_flushes_and_advances_modes_offset_80(){
 reset();with_core(|c|{c.display.stack.remove(0);});
 let before=screen::describe().1;display::back(2).unwrap();
 let e=events();let upload=e.iter().position(|x|*x==("upload",50)).expect("console was not uploaded");
 let bind=e.iter().position(|x|*x==("bind",50)).expect("console was not bound");
 let flip=e.iter().position(|x|*x==("flip",50)).expect("console was not flushed");
 assert!(upload<bind && bind<flip);assert!(e.contains(&("redraw",0)));
 let mut modes=abi::Modes::ZERO;(modes.console_resource,modes.console_draws)=screen::describe();
 assert!(modes.console_draws>before);let bytes=unsafe{std::slice::from_raw_parts(&modes as *const _ as *const u8,256)};
 assert_eq!(u64::from_ne_bytes(bytes[80..88].try_into().unwrap()),modes.console_draws);
 screen::repaint_rows().expect("native rectangular console flush missing");
 assert!(events().contains(&("flush",50)));
 // A text VT also uses the high kernel screen while a low-mode graphics
 // owner is temporarily released, before that owner's saved mode resumes.
 HOST.with(|h|h.borrow_mut().mode=0);screen::show().unwrap();
 assert_eq!(HOST.with(|h|h.borrow().mode),1);
}
#[derive(PartialEq)] enum Observer{Window,Owner}
enum Refused{Errno(i64)}impl From<i64> for Refused{fn from(e:i64)->Self{Self::Errno(e)}}
fn present_denied(tgid:u64)->Result<(),i64>{// PRESENT_PERMISSION
 panic!("non-owner reached device work")}
fn mode_denied()->Result<(),i64>{// MODE_PERMISSION
 panic!("non-owner reached mode work")}
fn readback_denied(tgid:u64,a:&abi::Readback,observer:Observer)->Result<(),Refused>{// READBACK_PERMISSION
 panic!("non-owner reached scanout work")}
#[test] fn nonowners_get_eperm_before_present_mode_and_scanout_work(){
 reset();assert_eq!(present_denied(99),Err(EPERM));assert_eq!(mode_denied(),Err(EPERM));
 assert!(matches!(readback_denied(99,&abi::Readback::ZERO,Observer::Owner),Err(Refused::Errno(EPERM))));
 assert!(events().is_empty());
}
#[test] fn restoration_and_console_rows_preserve_firmware_memory(){
 reset();with_core(|c|{c.display.stack.clear();});
 let mut firmware=[0x5a;32];let mut native=[0xaa;32];
 let cell=std::sync::OnceLock::new();cell.set(gk208::ConsoleSurface{pointer:native.as_mut_ptr() as usize,width:4,height:2,pitch:16}).ok().unwrap();
 let linear=console_display::linear::Linear{buffer:firmware.as_mut_ptr(),layout:raster::Layout{width:4,height:2,stride_bytes:16,bytes_per_pixel:4,reserved:0,bgr:true,font:raster::Font},native:cell,disabled:std::sync::atomic::AtomicBool::new(false)};
 for _ in 0..3 {
  assert!(linear.write(&[0x33;32],0..2,||true));
  display::restore().unwrap();screen::repaint_rows().unwrap();
 }
 assert_eq!(firmware,[0x5a;32],"normal console touched the firmware framebuffer");
 assert_eq!(native,[0xaa;32],"admitted console bypassed upload and wrote the bootstrap BAR surface");
 assert_eq!(events().iter().filter(|&&(name,_)|name=="backing-write").count(),3);
 assert!(!events().iter().any(|&(name,_)|name=="legacy-restore"));
 assert!(events().iter().filter(|&&(name,_)|name=="upload").count()>=3);
}
