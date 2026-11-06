#![allow(dead_code)] // Production present.rs includes unrelated surface APIs.
extern crate alloc;
use std::collections::BTreeMap;
#[path="@ROOT@/kernel/src/gpu/gk208/present.rs"] mod present;
struct Registers { words:BTreeMap<usize,u32>, updates:usize, acks:usize, acknowledge:bool }
impl Registers {fn read(&self,r:usize)->u32{*self.words.get(&r).unwrap_or(&0)}fn delay_us(&mut self,_:u32){}}
struct PendingPresentation {acknowledged:Option<u32>}
struct Evo {head:u32,sor:u32,total:u64,push:u64,put:u32,notifier:u64,limits:(),coefficients:(),scripts:[Vec<()>;4],pending_presentation:Option<PendingPresentation>}
impl Evo {
 fn start_clock(&mut self,_:&Registers)->Result<(),&'static str>{Ok(())}
 fn record_presentation(&mut self,_:&Registers,_:u32)->Result<(),&'static str>{Ok(())}
 fn arm_presentation(&mut self,_:&Registers,_:u32)->Result<(),&'static str>{assert!(self.pending_presentation.is_none());self.pending_presentation=Some(PendingPresentation{acknowledged:None});Ok(())}
 fn finish_presentation(&mut self,_:&Registers,_:u32)->Result<bool,&'static str>{assert!(self.pending_presentation.take().is_some());Ok(true)}
}
mod modeset {
 #[derive(Clone,Copy)]pub struct Viewport;
 impl Viewport {
  pub fn for_raster(_:u32,_:u32,_:u32,_:u32)->Result<Self,&'static str>{Ok(Self)}
  pub fn verify(&self,r:&super::Registers,_:u32)->Result<(),&'static str>{if r.read(0x4c0)==1{Ok(())}else{Err("wrong viewport")}}
  pub fn methods(&self,_:u32)->Result<Vec<(u32,u32)>,&'static str>{Ok(vec![(0x4c0,1)])}
 }
 pub fn wait(_: &mut super::Registers,_:usize,_:u32,_:u32)->Result<(),&'static str>{Ok(())}
 pub fn publish(_: &mut super::Registers,_:u32)->Result<(),&'static str>{Ok(())}
}
mod cost {pub enum Slot{Evo}pub fn phase(_:Slot){}}
mod memx_display {
 #[derive(Clone,Copy)]pub struct Health;
 impl Health {pub fn read(_: &mut super::Registers,_:u32,_:u32)->Self{Self}pub fn check(&self)->Result<(),&'static str>{Ok(())}}
}
mod display_evidence {pub fn record(_: &mut super::Registers,_:u32,_:super::present::Surface,_:&super::present::Receipt){}}
mod presentation_trace {pub fn dump(){}}
mod native {
 use super::*;
 static MEMX_PRESENT_PHASE:core::sync::atomic::AtomicU32=core::sync::atomic::AtomicU32::new(0);
 static MEMX_CLEAR_AVAILABLE:core::sync::atomic::AtomicBool=core::sync::atomic::AtomicBool::new(false);
 fn memx_health_record(_: &str,_:&str,_:u32,_:u32,_:memx_display::Health){}
 fn memx_pending_check(_: &mut Registers,_:u32,_:u32,_:memx_display::Health)->((),Result<(),&'static str>){((),Ok(()))}
 fn write_words(r:&mut Registers,_:u64,a:u64,w:&[u32])->Result<(),&'static str>{for(i,&v)in w.iter().enumerate(){r.words.insert(a as usize+i*4,v);}Ok(())}
 fn read_word(r:&mut Registers,_:u64,a:u64)->Result<u32,&'static str>{Ok(r.read(a as usize))}
 fn submit(r:&mut Registers,_:u64,_:u64,p:&mut u32,m:&[(u32,u32)])->Result<(),&'static str>{
  r.updates+=1;*p+=m.len() as u32*8;
  for &(a,v)in m {if (0x460..=0x474).contains(&a){r.words.insert(0x640000+a as usize,v);r.words.insert(0x660000+a as usize,v);}}
  Ok(()) // UPDATE is consumed, but the display notifier has not latched.
 }
 fn complete(r:&mut Registers,_:u64,a:u64,_:&(),_:&(),_:(u32,u32,bool,bool),_:&[Vec<()>;4])->Result<u32,&'static str>{let v=r.read(a as usize);if v&1==1{Ok(v)}else{Err("not latched")}}
 fn acknowledge(_: &mut Evo,r:&mut Registers)->Result<u32,&'static str>{r.acks+=1;if r.acknowledge{Ok(0xcafe0001)}else{Err("withheld fresh ack")}}
 // PRESENT_INNER
 pub fn run(e:&mut Evo,r:&mut Registers,s:present::Surface)->Result<Option<present::Receipt>,&'static str>{present_inner(e,r,s,false)}
}
fn setup()->(Evo,Registers,present::Surface){
 let s=present::Surface{resource:71,address:0x12340000,width:1280,height:800,pitch:5120,block_height:0,format:0,dma:1};
 let mut r=Registers{words:BTreeMap::from([(0x64041c,0),(0x640420,1280|1024<<16),(0x4c0,1)]),updates:0,acks:0,acknowledge:true};
 for &(m,v)in &s.methods(0).unwrap()[..5]{for bank in [0x640000,0x660000]{r.words.insert(bank+m as usize,v);}}
 (Evo{head:0,sor:0,total:1<<30,push:0x2000,put:0,notifier:0x1000,limits:(),coefficients:(),scripts:std::array::from_fn(|_|vec![]),pending_presentation:None},r,s)
}
#[test]fn retained_refresh_does_not_schedule_a_redundant_flip(){
 let(mut e,mut r,s)=setup();
 for _ in 0..950 {
  assert!(native::run(&mut e,&mut r,s).unwrap().is_none(),"W17 retained content must await its next edge");
  assert_eq!(e.pending_presentation.as_ref().unwrap().acknowledged,Some(0xcafe0001));
  assert!(e.finish_presentation(&r,s.resource).unwrap());
 }
 assert_eq!(r.acks,950);assert_eq!(r.updates,0);
}
#[test]fn retained_refresh_refuses_a_missing_fresh_device_ack(){
 let(mut e,mut r,s)=setup();r.acknowledge=false;
 assert!(native::run(&mut e,&mut r,s).is_err(),"stale latched state replaced a fresh acknowledgement");
}
#[test]fn changed_surface_or_viewport_remains_a_real_pending_flip(){
 for mismatch in [0x640460,0x660460,0x4c0] {
  let(mut e,mut r,s)=setup();r.words.insert(mismatch,0);
  assert!(native::run(&mut e,&mut r,s).unwrap().is_none());
  assert_eq!((r.updates,r.acks),(1,0));assert_eq!(r.read(e.notifier as usize),0);
 }
}
