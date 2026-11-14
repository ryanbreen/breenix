use std::{cell::Cell, sync::{Mutex,atomic::AtomicU32}};
thread_local! {
 static GROUP:Cell<u64>=const{Cell::new(1)};
 static NATIVE:Cell<bool>=const{Cell::new(true)};
 static MODE_SETS:Cell<usize>=const{Cell::new(0)};
 static BOOTSTRAPS:Cell<usize>=const{Cell::new(0)};
}
const EINVAL:i64=22;const EBUSY:i64=16;const ENOMEM:i64=12;
fn read_arg<T:Copy>(arg:u64,_:u64)->Result<T,i64>{Ok(unsafe{*(arg as *const T)})}
mod graphics {
 pub fn give_back_display()->Result<(),i64>{super::display::back(super::caller()?)}
 pub fn take_over_display()->Result<(),i64>{super::display::take(super::caller()?,super::caller()?,1)}
}
#[derive(Debug)] enum StageRefused{Eio(u32,u32)}
mod readback_reason {pub const BINDING_UNKNOWN:u32=10;}
fn binding_check()->Result<(),StageRefused>{
 let a=abi::Readback::ZERO;
 // BINDING_CHECK
 Ok(())
}
mod stage {
 use super::*;
 static DISPLAY_MODE:AtomicU32=AtomicU32::new(u32::MAX);
 pub fn remember(){
 // REMEMBER
 }
 pub mod gpu {
  pub use gpu_abi as abi;
  use abi::Plain;
  pub fn modes()->(i64,abi::Modes){
   (0,super::with_core(|c|{let mut m=abi::Modes::ZERO;m.current=c.display.mode;
    m.count=c.display.modes.len() as u32;for(i,v)in c.display.modes.iter().enumerate(){if i<2{m.modes[i]=*v;}else{m.extra_modes[i-2]=*v;}}
    m.binding_known=c.display.known as u32;m.scanout_resource=c.display.binding.map_or(0,|b|b.id as u64);m}).unwrap())
  }
  pub fn set_mode(index:u32)->i64{super::MODE_SETS.with(|n|n.set(n.get()+1));let a=abi::SetMode{index,..abi::SetMode::ZERO};super::display::set_mode(&a as *const _ as u64,std::mem::size_of_val(&a) as u64).map_or_else(|e|-e, |_|0)}
 }
 mod process {
  pub struct Pid;impl Pid{pub fn raw(&self)->u32{super::GROUP.with(|g|g.get() as u32)}}
  pub fn getpid()->Result<Pid,i64>{Ok(Pid)}
 }
 struct Gpu{pid:u64,seen:Vec<u8>,canvas:u64}
 impl Gpu {
  fn new(pid:u64)->Result<Self,String>{Ok(Self{pid,seen:Vec::new(),canvas:80})}
  fn run(&self,_:&[u32],_:&str)->Result<(),String>{Ok(())}
  fn present(&mut self,_:bool)->Result<(u64,Option<(i32,i32)>),String>{
   BOOTSTRAPS.with(|n|n.set(n.get()+1));
   with_core(|c|{let m=c.display.modes[c.display.mode as usize];
    c.resources.insert(80,Resource{width:m.width,height:m.height,lost:false,holds:0,last:None,generation:0,invalid:false});});
   display::restored(display::Binding{id:80,fence:8,answer:0x60000000});Ok((8,None))
  }
 }
 static GPU:Mutex<Option<Gpu>>=Mutex::new(None);
 struct Font;impl Font{fn ready(&mut self)->Result<(),String>{Ok(())}}
 static FONT:Mutex<Font>=Mutex::new(Font);
 struct Observation{backend:u32,flags:u32,boundary:u32}
 struct Seen{backend:u32,flags:u32,boundary:u32}
 fn observe(_:&mut Vec<u8>)->Result<Seen,String>{
  binding_check().map_err(|StageRefused::Eio(reason,detail)|format!("stage readback: EIO, reason {reason}, detail {detail}"))?;
  Ok(Seen{backend:6,flags:1,boundary:5})
 }
 struct Show{observation:Observation}
 impl Show{fn draw(&mut self,_:u32)->Result<(),String>{Ok(())}}
 static SHOW:Mutex<Option<Show>>=Mutex::new(None);
 // TAKE
 // PREPARE
 pub fn child(name:&str)->Result<(),String>{
  // CHILD_READY
  ready.map_err(|e|e.to_string())
 }
 pub fn case_takes_display(_: &str)->bool{true}
 pub fn take_for_intent()->Result<(),String>{
  // INTENT_TAKE
 }
 pub fn clean(){*GPU.lock().unwrap()=None;}
}
fn start(native:bool){
 reset();MODE_SETS.with(|n|n.set(0));NATIVE.with(|n|n.set(native));GROUP.with(|g|g.set(1));BOOTSTRAPS.with(|n|n.set(0));stage::clean();
 with_core(|c|{
  c.display=display::State::new();
  if native {c.display.modes=vec![abi::Mode{width:1024,height:768,scale:1,reserved:0},abi::Mode{width:1280,height:1024,scale:1,reserved:0},abi::Mode{width:2560,height:2048,scale:2,reserved:0}];}
  c.display.stack.clear();c.display.mode=1;c.display.holds.clear();
  c.display.binding=Some(display::Binding{id:50,fence:1,answer:0x60000000});
 });
 syscall::display::DISPLAY_OWNER_GROUP.store(u64::MAX,Ordering::Release);
 graphics::take_over_display().unwrap();
 // main's startup show::scale2_mode: take then SET_MODE.
 let index=if native{2}else{1};assert_eq!(stage::gpu::set_mode(index),0);stage::remember();
}
#[test] fn breengel_first_stage_observes_a_bound_target(){
 for native in [false,true]{
  start(native);let result=stage::prepare();
  assert!(result.is_ok(),"native={native}: first stage observe {result:?}");
  assert_eq!(MODE_SETS.with(|n|n.get()),1,"prepare must not reset an already selected mode");
  assert_eq!(BOOTSTRAPS.with(|n|n.get()),usize::from(native),"virtio keeps its existing bound frame");
 }
}
#[test] fn breengel_later_case_keeps_scale2_after_kernel_return(){
 for native in [false,true]{
  start(native);let expected=if native{2}else{1};
  // The intent frame's idempotent takeover is followed by a root giveback
  // on drop. A second page takes ownership before selecting its retained mode.
  intent::page();intent::page();
  assert_eq!(with_core(|c|c.display.mode),Some(1));
  stage::child("runner-reclaim").unwrap();
  // Ordinary child exit restores the root's requested mode.
  GROUP.with(|g|g.set(2));stage::child("windows/distinct-content").unwrap();
  with_core(|c|display::ended(c,2));display::restore().unwrap();
  assert_eq!(with_core(|c|c.display.mode),Some(expected));
  // ABOVE_KERNEL_SCREEN release empties the stack and publish_owner restores
  // the native default. Its next child must select scale 2 while it owns it.
  GROUP.with(|g|g.set(1));display::back(1).unwrap();
  assert_eq!(with_core(|c|c.display.mode),Some(1));
  GROUP.with(|g|g.set(3));stage::child("composition/ownership-stack").unwrap();
  assert_eq!(with_core(|c|c.display.mode),Some(expected),"native={native}: later case reverted to kernel mode");
  with_core(|c|display::ended(c,3));display::restore().unwrap();
  GROUP.with(|g|g.set(1));stage::child("panel/kernel-screens").unwrap();
  assert_eq!(with_core(|c|c.display.mode),Some(expected));
  assert_eq!(MODE_SETS.with(|n|n.get()),if native{5}else{1},"virtio must keep the startup-only SET_MODE sequence");
 }
}

mod suite_stage {pub fn take_display()->Result<(),String>{crate::stage::take_for_intent()}}
mod intent {
 use super::*;
 mod gpu {
  pub fn resource_destroy(_:u64)->i64{0}
  pub fn context_destroy(_:u64)->i64{0}
 }

 // INTENT_FRAME
 impl Frame {
  fn takeover(&mut self)->Result<(),&'static str>{
   // INTENT_CUSTODY
   Ok(())
  }
 }
 pub fn page(){
  // INTENT_SUITE
  // INTENT_NEW
  frame.takeover().unwrap();
 }
}
