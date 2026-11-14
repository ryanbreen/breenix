extern crate gpu_abi as abi;
use abi::{record, Plain};
use std::sync::atomic::{AtomicUsize, AtomicBool, Ordering::SeqCst};
pub type CaseError=String;
static JUDGED:AtomicUsize=AtomicUsize::new(0);
static COMPARED:AtomicUsize=AtomicUsize::new(0);
static CORRUPT:AtomicBool=AtomicBool::new(false);
pub fn fail_msg(s:String)->String{s}
pub fn errno(r:i64)->String{r.to_string()}
pub fn ensure(ok:bool,msg:impl FnOnce()->String)->Result<(),String>{if ok {Ok(())} else {Err(msg())}}
pub fn native_success()->Result<u32,String>{Ok(0)}
pub fn wait_ok(_:u64,_:u64,_:u64)->Result<(),String>{Ok(())}
pub fn u32_at(s:&[u8],at:usize)->u32{u32::from_le_bytes(s[at..at+4].try_into().unwrap())}
pub fn u64_at(s:&[u8],at:usize)->u64{u64::from_le_bytes(s[at..at+8].try_into().unwrap())}
pub mod libbreenix {pub mod graphics {pub fn take_over_display()->Result<(),i64>{Ok(())}}}
pub mod gpu {
 use super::*;
 pub fn call_with(op:u32,m:&mut abi::Modes)->i64 {
  assert_eq!(op,abi::op::MODES);m.count=1;m.current=0;
  m.modes[0]=abi::Mode{width:2560,height:2048,scale:2,reserved:0};0
 }
 pub fn present(_:u64,t:u64)->(i64,abi::Present){assert_eq!(t,2);(0,abi::Present{fence:7,..abi::Present::ZERO})}
 pub fn resource_destroy(t:u64)->i64{assert_eq!(t,2);0}
}
pub fn scanout_target(_:u32,w:u32,h:u32)->Result<u64,String>{assert_eq!([w,h],[2560,2048]);Ok(2)}
pub fn scene(_:u64,_:u32,_:&[()])->Vec<u32>{vec![]}
pub mod encode {pub fn copy(_: &mut Vec<u32>,_:u64,r:[u32;4],_:u64,_:[u32;2]){assert_eq!(r,[0,0,2560,1600]);}}
pub fn frame(_:u64,_:&[u32])->Result<u64,String>{Ok(1)}
pub fn read(t:u64,w:u32,h:u32)->Result<(Vec<u8>,[u8;record::SIZE]),String>{
 let mut r=[0;record::SIZE];
 for (at,v) in [(record::TARGET_WIDTH,2560u32),(record::TARGET_HEIGHT,if t==0 {2048} else {1600}),
                (record::FORMAT,abi::format::B8G8R8A8),(record::FLAGS,if t==0 {1} else {0})] {r[at..at+4].copy_from_slice(&v.to_le_bytes());}
 for (at,v) in [(record::RESOURCE,if t==0 {2u64} else {1}),(record::PRESENT_FENCE,7)] {r[at..at+8].copy_from_slice(&v.to_le_bytes());}
 let mut pixels=vec![0;(w*h*4) as usize];if t==0 && CORRUPT.load(SeqCst){pixels[0]=1;}
 Ok((pixels,r))
}
pub struct Canvas {pub w:u32,pub h:u32,pub px:Vec<u8>}
impl Canvas {pub fn compare(&self,got:&[u8],_:&str)->Result<(),String>{
 COMPARED.fetch_add(1,SeqCst);assert_eq!(self.px.len(),(self.w*self.h*4) as usize);
 ensure(self.px==got,||"pixel mismatch".into())
}}
pub mod breenix_suite {pub mod suite {
 pub fn screen_judged_readback(_: &[u8],_: &[u8]){}
 pub fn judged_readback_owned(_:Vec<u8>,w:u32,h:u32)->Result<(),String>{assert_eq!([w,h],[2560,1600]);super::super::JUDGED.fetch_add(1,super::super::SeqCst);Ok(())}
}}
mod intent {
 // INTENT
 pub fn irq_plan()->Result<Vec<Layout>, &'static str>{picture_pages("gpu","isolation/irq-masked-bound",[2560,1600],[2560,2048])}
 pub fn control(){
  let p=picture_pages("gpu","performance/clear-rate-2560",[2560,1600],[2560,2048]).unwrap().remove(0);
  let _=(p.scale,p.picture,p.border);assert!(!samples([2560,1600],p).is_empty());
  assert_eq!(picture_pages("gpu","missing-declared-intent",[2560,1600],[2560,2048]),Err("intent-inventory"));
 }
}
// VIEW
fn irq_presentation()->Result<(),String>{
 // ALIAS
 let(ctx,t,w,h)=(1,1,2560,1600);
 for _ in 0..10 { // IRQ
 }
 Ok(())
}
fn main(){
 intent::control();assert_eq!(intent::irq_plan(),Err("intent-inventory"));
 irq_presentation().unwrap();
 assert_eq!(COMPARED.load(SeqCst),10,"retain exact scanout comparisons");
 if JUDGED.load(SeqCst)>0 {panic!("end-only IRQ requested {:?}",intent::irq_plan());}
 CORRUPT.store(true,SeqCst);assert!(irq_presentation().unwrap_err().contains("pixel mismatch"));
 CORRUPT.store(false,SeqCst);present_view(1,1).unwrap();
 assert_eq!(JUDGED.load(SeqCst),1,"declared intent callers keep their judged handoff");
}
