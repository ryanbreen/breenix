"""Execute the production capture transfer/copy loop at normal and tight quota."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class CaptureBatch(unittest.TestCase):
 def test_transfer_batches_keep_pixels_and_tight_quota(self):
  src=item((ROOT/'kernel/src/gpu/window_targets.rs').read_text(),'    fn read_to_inner(')
  body=src[src.index('        let result = (|| {'):src.index('        release_cpu_read(self.id);')]
  run_rust(self,r'''
use std::sync::{Mutex,atomic::{AtomicUsize,AtomicBool,Ordering}};
const ENOMEM:i64=-12;const EFAULT:i64=-14;const EIO:i64=-5;
static LIMIT:AtomicUsize=AtomicUsize::new(4*1024*1024);
static HELD:AtomicUsize=AtomicUsize::new(0);
static CALLS:Mutex<Vec<(u32,u32)>>=Mutex::new(Vec::new());
static BAD:AtomicBool=AtomicBool::new(false);
struct Pages(Mutex<Vec<u8>>);
impl Pages{fn read(&self,at:usize,out:&mut[u8]){out.copy_from_slice(&self.0.lock().unwrap()[at..at+out.len()]);}}
struct Staging{pages:Pages,bytes:usize}
impl Staging{fn new(_:u64,n:usize)->Result<Self,i64>{if n>LIMIT.load(Ordering::Relaxed){return Err(ENOMEM);}HELD.fetch_add(n,Ordering::Relaxed);Ok(Self{pages:Pages(Mutex::new(vec![0;n])),bytes:n})}}
impl Drop for Staging{fn drop(&mut self){HELD.fetch_sub(self.bytes,Ordering::Relaxed);}}
mod abi{#[derive(Default)]pub struct Readback{pub target:u64,pub y:u32,pub width:u32,pub height:u32}impl Readback{pub const ZERO:Self=Self{target:0,y:0,width:0,height:0};}}
mod format{pub const B8G8R8A8:u32=1;}
struct Context{id:u32}
struct Observation{unwritten:u32}
fn pixel(x:u32,y:u32)->[u8;4]{[x as u8,y as u8,(x^y)as u8,255]}
fn observe(_:u64,id:u32,_:Option<()>,_:&(),pitch:u32,fmt:u32,a:abi::Readback,stage:&Pages,ctx:Option<u32>,_:Option<()>)->Result<Observation,(i64,u32)>{
 assert_eq!((a.target,id,pitch,fmt,ctx),(7,7,960,1,Some(9)));
 CALLS.lock().unwrap().push((a.y,a.height));
 let mut bytes=stage.0.lock().unwrap();assert!(bytes.len()>=(a.width*a.height*4)as usize);
 for y in 0..a.height{for x in 0..a.width{let at=((y*a.width+x)*4)as usize;bytes[at..at+4].copy_from_slice(&pixel(x,a.y+y));}}
 Ok(Observation{unwritten:u32::from(BAD.load(Ordering::Relaxed))})
}
mod syscall{pub mod userptr{pub fn write_user_bytes(dst:u64,src:*const u8,n:usize)->Result<(),()>{unsafe{std::ptr::copy_nonoverlapping(src,dst as *mut u8,n)};Ok(())}}}
struct Target{width:u32,height:u32,handle:u64,id:u32}
impl Target{fn copy(&self,dst:u64,pause_at:Option<u64>,pause:impl Fn())->Result<(),i64>{
 let owner=1;let context=Context{id:9};let last=None;let backing=();let pitch=self.width*4;
 @BODY@
 result
}}
#[test]fn batching_and_quota(){
 let t=Target{width:240,height:160,handle:7,id:7};
 for limit in [4*1024*1024,16*1024]{
  LIMIT.store(limit,Ordering::Relaxed);CALLS.lock().unwrap().clear();let mut out=vec![0;240*160*4];
  let pauses=AtomicUsize::new(0);
  t.copy(out.as_mut_ptr()as u64,Some(80),||{pauses.fetch_add(1,Ordering::Relaxed);}).unwrap();
  assert_eq!(pauses.load(Ordering::Relaxed),1);assert_eq!(HELD.load(Ordering::Relaxed),0);
  for y in 0..160{for x in 0..240{let at=((y*240+x)*4)as usize;assert_eq!(&out[at..at+4],&pixel(x,y));}}
  let calls=CALLS.lock().unwrap();let mut y=0;for &(at,n)in calls.iter(){assert_eq!(at,y);assert!(n>0);assert!(n as usize*960<=limit);y+=n;}assert_eq!(y,160);
  if limit>16384{assert_eq!(calls.len(),1,"normal capture split one client into repeated two-pass transfers");}else{assert_eq!(calls.len(),10,"tight quota changed its bounded staging");}
 }
 let mut out=vec![0;240*160*4];BAD.store(true,Ordering::Relaxed);assert_eq!(t.copy(out.as_mut_ptr()as u64,None,||{}),Err(EIO));assert_eq!(HELD.load(Ordering::Relaxed),0);
 BAD.store(false,Ordering::Relaxed);LIMIT.store(1,Ordering::Relaxed);assert_eq!(t.copy(out.as_mut_ptr()as u64,None,||{}),Err(ENOMEM));assert_eq!(HELD.load(Ordering::Relaxed),0);
}
'''.replace('@BODY@',body).replace('#[cfg(target_arch = "x86_64")]',''))
if __name__=='__main__':unittest.main()
