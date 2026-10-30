#![allow(dead_code)] // The legacy API fixture also compiles the pre-repair path.
use std::{cell::RefCell};
type Verdict<T=()> = Result<T,String>;
fn require(ok:bool,s:&str)->Verdict {if ok {Ok(())} else {Err(s.into())}}
fn gpu_display()->Verdict<Option<((usize,usize),u64)>> {Ok(Some(((4,2),42)))}
thread_local! {static EVENTS:RefCell<Vec<&'static str>>=const{RefCell::new(Vec::new())};}
fn event(s:&'static str){EVENTS.with(|e|e.borrow_mut().push(s));}
mod libbreenix {
 pub mod graphics {
  #[derive(Debug)] pub struct Busy;
  impl std::fmt::Display for Busy {fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{write!(f,"EBUSY")}}
  pub struct Info {pub width:u64,pub height:u64,pub bytes_per_pixel:u64}
  pub fn fbinfo()->Result<Info,Busy>{Ok(Info{width:4,height:2,bytes_per_pixel:4})}
  pub fn fb_mmap_pane()->Result<(*mut u8,u64),Busy>{let p=Box::leak(Box::new([0u8;32]));Ok((p.as_mut_ptr(),4))}
  pub fn fb_flush()->Result<(),Busy>{crate::event("legacy-flush");Err(Busy)}
  pub fn fb_read_pixel(_:i32,_:i32)->Result<u32,Busy>{crate::event("legacy-read");Ok(0x123456)}
 }
 pub mod gpu {
  pub mod abi {
   pub mod format{pub const B8G8R8A8:u32=1;}
   pub mod record{pub const SIZE:usize=512;}
   pub mod stream{pub mod encode{
    pub fn begin(w:&mut Vec<u32>,_:u64){w.push(1);}
    pub fn clear(w:&mut Vec<u32>,c:u32){w.push(c);}
   }}
  }
  pub struct Mode {pub width:u32,pub height:u32}
  pub struct Modes {pub current:u32,pub modes:[Mode;1]}
  pub struct Submit {pub fence:u64}
  pub struct Wait {pub status:u32}
  pub fn modes()->(i64,Modes){(0,Modes{current:0,modes:[Mode{width:4,height:2}]})}
  pub fn context_create()->Result<u64,i64>{crate::event("context");Ok(7)}
  pub fn scanout_target_create(_:u32,w:u32,h:u32)->Result<u64,i64>{assert_eq!((w,h),(4,2));crate::event("target");Ok(42)}
  pub fn context_destroy(_:u64)->i64{crate::event("destroy-context");0}
  pub fn resource_destroy(_:u64)->i64{crate::event("destroy-target");0}
  pub fn submit(c:u64,w:&[u32])->(i64,Submit){assert_eq!(c,7);assert_eq!(w,&[1,0xff3c3c3c]);crate::event("clear");(0,Submit{fence:9})}
  pub fn wait(c:u64,f:u64,_:u64)->(i64,Wait){assert_eq!((c,f),(7,9));crate::event("wait");(0,Wait{status:0})}
  pub fn present(c:u64,t:u64)->(i64,()){assert_eq!((c,t),(7,42));crate::event("present");(0,())}
  pub fn readback(t:u64,r:[u32;4],b:&mut[u8],s:u32,_:&mut Box<[u8;512]>)->i64{
   assert_eq!((t,r,s),(0,[0,0,4,2],16));crate::event("readback");
   for p in b.chunks_exact_mut(4){p.copy_from_slice(&0xff3c3c3cu32.to_le_bytes());}0
  }
 }
}
// OWNER_FUNCTIONS
#[test] fn native_owner_uses_gpu_instead_of_the_refused_pane(){
 let painted=paint_owner().expect("GPU owner painting must not call the EBUSY pane path");
 assert_eq!((painted.width,painted.height),(4,2));
 assert_eq!(displayed(&[(0,0),(3,1)]).unwrap(),vec![0x3c3c3c;2]);
 drop(painted);
 EVENTS.with(|e|assert_eq!(*e.borrow(),vec!["context","target","clear","wait","present","readback","destroy-target","destroy-context"]));
}
#[test] fn native_reads_the_latched_resource_instead_of_stale_bar_pixels(){
 assert_eq!(displayed(&[(0,0),(3,1)]).unwrap(),vec![0x3c3c3c;2]);
}
