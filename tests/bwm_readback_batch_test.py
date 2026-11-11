"""Run the production pixel checker with deterministic independent readbacks."""
from pathlib import Path
import os
import subprocess
import re
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT = Path(__file__).resolve().parents[1]
class ReadbackBatch(unittest.TestCase):
    def test_all_pixels_with_bounded_transfers_and_late_mismatch(self):
        path = 'userspace/programs/src/bwm/compositor.rs'
        old = os.environ.get('UITEST_OLD_COMMIT')
        source = subprocess.check_output(['git','show',f'{old}:{path}'],cwd=ROOT,text=True) if old else (ROOT/path).read_text()
        constants = '\n'.join(re.findall(r'^const CHECK_BAND_.*?;', source, re.M))
        run_rust(self, r'''
use std::sync::Mutex;
static CALLS:Mutex<Vec<[u32;4]>>=Mutex::new(Vec::new());
static OVERLAP:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(false);
static ENTERED:Mutex<[bool;2]>=Mutex::new([false;2]);
static READY:std::sync::Condvar=std::sync::Condvar::new();
fn independent_read(scanout:bool){
 if !OVERLAP.load(std::sync::atomic::Ordering::Relaxed){return;}
 let index=usize::from(scanout);let mut entered=ENTERED.lock().unwrap();
 entered[index]=true;READY.notify_all();
 let (entered,result)=READY.wait_timeout_while(entered,std::time::Duration::from_secs(3),|s|!s[1-index]).unwrap();
 assert!(!result.timed_out()&&entered[1-index],"independent desktop and scanout reads were serialized");
}
static CLIENT:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(false);
static BUFFERS:std::sync::atomic::AtomicUsize=std::sync::atomic::AtomicUsize::new(0);
static PIXELS:std::sync::atomic::AtomicUsize=std::sync::atomic::AtomicUsize::new(0);
static HOTSPOTS:std::sync::atomic::AtomicUsize=std::sync::atomic::AtomicUsize::new(0);
static POINTER:Mutex<bool>=Mutex::new(false);
static HIGH_X:Mutex<bool>=Mutex::new(false);
static BAD:Mutex<Option<(u32,u32)>>=Mutex::new(None);
CONSTANTS
mod libuitest {pub mod startup{pub fn mark(_: &str){}}}
mod abi {pub mod record{pub const SIZE:usize=1024;}}
mod libbreenix {
 pub mod errno {#[derive(Debug)]pub enum Errno{EINVAL}}
 pub mod error {#[derive(Debug)]pub enum Error{Os(super::errno::Errno)} impl std::fmt::Display for Error{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{write!(f,"error")}}}
}
mod wabi {pub struct WinInfo{pub id:u64,pub width:u32,pub height:u32} impl WinInfo{pub const ZERO:Self=Self{id:0,width:0,height:0};} impl Clone for WinInfo{fn clone(&self)->Self{Self::ZERO}}}
mod window {
 pub struct Captured{pub commit_id:u64,pub width:u32,pub height:u32}
 pub fn capture(_:u64,out:&mut[u8],_:Option<()>)->Result<Captured,super::libbreenix::error::Error>{
  if super::CLIENT.load(std::sync::atomic::Ordering::Relaxed){super::fill([0,0,256,256],out,false);Ok(Captured{commit_id:7,width:256,height:256})}
  else{Err(super::libbreenix::error::Error::Os(super::libbreenix::errno::Errno::EINVAL))}}
 pub fn list(_:&mut[super::wabi::WinInfo])->Result<usize,()>{Ok(0)}
}
fn pixel(x:u32,y:u32)->u32{0xff000000|((y*257+x*3)&0xffffff)}
fn fill(rect:[u32;4],out:&mut[u8],bad:bool){let[x,y,w,h]=rect;for row in 0..h{for col in 0..w{let mut p=pixel(x+col,y+row);if bad&&*POINTER.lock().unwrap() && (3..7).contains(&(x+col)) && (3..7).contains(&(y+row)) {p=if ((x+col-3)/2+(y+row-3)/2)%2==0{0xff000000}else{0xffffffff};}if bad&&*HIGH_X.lock().unwrap(){p^=0xff000000;}if bad&&*BAD.lock().unwrap()==Some((x+col,y+row)){p^=1;}let at=((row*w+col)*4)as usize;out[at..at+4].copy_from_slice(&p.to_le_bytes());}}}
mod gpu {
 pub struct Answer{pub reason:u32}
 pub fn readback_why(_:u64,r:[u32;4],out:&mut[u8],stride:u32,_:&mut[u8;1024])->(i64,Answer){super::independent_read(true);assert_eq!(stride,r[2]*4);super::CALLS.lock().unwrap().push(r);super::fill(r,out,true);(0,Answer{reason:0})}
}
struct Desktop;
impl Desktop{fn read_rect(&self,r:[usize;4],out:&mut[u8],stride:usize)->Result<(),i64>{independent_read(false);assert_eq!(stride,r[2]*4);fill(r.map(|x|x as u32),out,false);Ok(())}}
struct Sample{ id:u64,width:u32,height:u32,commit_id:u64 }
struct Layer{window_id:u64,frame:[[i32;4];3],frame_len:usize}
#[derive(Clone,Copy)]struct Pointer{x:i32,y:i32,shape:u32}
const CURSOR_WHITE:u32=0xffffffff;const CURSOR_BLACK:u32=0xff000000;
fn pointer_bitmap(_:u32)->&'static[&'static[u8]]{&[b"#o",b"o#"]}
fn hotspot(_:u32)->(i32,i32){HOTSPOTS.fetch_add(1,std::sync::atomic::Ordering::Relaxed);(0,0)}
fn intersect(a:[i32;4],b:[i32;4])->Option<[i32;4]>{let r=[a[0].max(b[0]),a[1].max(b[1]),a[2].min(b[2]),a[3].min(b[3])];(r[0]<r[2]&&r[1]<r[3]).then_some(r)}
fn monotonic_ns()->u64{0}
fn prefaulted_bytes(n:usize)->Vec<u8>{if n>=65536{BUFFERS.fetch_add(1,std::sync::atomic::Ordering::Relaxed);}vec![0;n]}
#[derive(Debug)]enum CheckError{Failed(String),Moved(String)}
struct Compositor{sampled:Vec<Sample>,check_first:u64,check_workspace:CheckWorkspace,desktop:Desktop,scale:i32,appbar_top:i32}
impl Compositor {
 fn sampled_content(&self,_:&Layer)->Option<[i32;4]>{CLIENT.load(std::sync::atomic::Ordering::Relaxed).then_some([0,0,256,256])}
 @EXPECTED@
 @CHECK@
}
#[test]fn full_frame_and_offset_tail(){
 let mut c=Compositor{sampled:vec![],check_first:0,check_workspace:CheckWorkspace::default(),desktop:Desktop,scale:2,appbar_top:1600};
 OVERLAP.store(true,std::sync::atomic::Ordering::Relaxed);
 assert_eq!(c.check_area(0,&[],None,[0,0,64,64]).unwrap(),4096);
 OVERLAP.store(false,std::sync::atomic::Ordering::Relaxed);CALLS.lock().unwrap().clear();
 *POINTER.lock().unwrap()=true;
 assert_eq!(c.check_area(0,&[],Some(Pointer{x:3,y:3,shape:0}),[0,0,2560,1600]).unwrap(),4096000);
 let hotspots=HOTSPOTS.load(std::sync::atomic::Ordering::Relaxed);
 assert!(hotspots<=20000,"cursor geometry evaluated for {hotspots} pixels instead of its rows");
 *POINTER.lock().unwrap()=false;
 let calls=CALLS.lock().unwrap().clone();
 assert!(calls.len()<=4,"32-row calls repeated the same transfer setup {} times",calls.len());
 assert!(calls.iter().all(|r| r[2] as usize*r[3] as usize*4<=4*1024*1024));
 CALLS.lock().unwrap().clear();
 assert_eq!(c.check_area(0,&[],None,[13,7,2559,1599]).unwrap(),2546*1592);
 let calls=CALLS.lock().unwrap();let mut y=7;for r in calls.iter(){assert_eq!((r[0],r[1],r[2]),(13,y,2546));y+=r[3];}assert_eq!(y,1599);drop(calls);
 *BAD.lock().unwrap()=Some((2558,1598));
 match c.check_area(0,&[],None,[13,7,2559,1599]){Err(CheckError::Failed(s))=>assert!(s.contains("pixel (2558, 1598)"),"{s}"),Err(CheckError::Moved(s))=>panic!("{s}"),Ok(n)=>panic!("missed late mismatch after {n}")}
 *BAD.lock().unwrap()=None;
 // A chrome-only layer samples the desktop itself. It must retain the full
 // comparison without sending its other 1590 rows through the pixel oracle.
 PIXELS.store(0,std::sync::atomic::Ordering::Relaxed);
 let layers=[Layer{window_id:7,frame:[[0,10,2560,20];3],frame_len:1}];
 assert_eq!(c.check_area(0,&layers,None,[0,0,2560,1600]).unwrap(),4096000);
 assert_eq!(PIXELS.load(std::sync::atomic::Ordering::Relaxed),0,"chrome-only rows used the per-pixel layer oracle");
 let allocated=BUFFERS.load(std::sync::atomic::Ordering::Relaxed);
 assert_eq!(c.check_area(0,&layers,None,[0,0,2560,1600]).unwrap(),4096000);
 assert_eq!(BUFFERS.load(std::sync::atomic::Ordering::Relaxed),allocated,"steady checks allocated fresh readback buffers");
 CLIENT.store(true,std::sync::atomic::Ordering::Relaxed);
 c.sampled.push(Sample{id:7,width:256,height:256,commit_id:7});
 assert_eq!(c.check_area(0,&layers,None,[0,0,2560,1600]).unwrap(),4096000);
 let allocated=BUFFERS.load(std::sync::atomic::Ordering::Relaxed);
 assert_eq!(c.check_area(0,&layers,None,[0,0,2560,1600]).unwrap(),4096000);
 assert_eq!(BUFFERS.load(std::sync::atomic::Ordering::Relaxed),allocated,"stable client capture allocated another pixel buffer");
 *BAD.lock().unwrap()=Some((100,100));
 assert!(matches!(c.check_area(0,&layers,None,[0,0,2560,1600]),Err(CheckError::Failed(_))),"client corruption escaped the independent capture");
 CLIENT.store(false,std::sync::atomic::Ordering::Relaxed);
 *BAD.lock().unwrap()=Some((2558,1598));
 assert!(matches!(c.check_area(0,&layers,None,[0,0,2560,1600]),Err(CheckError::Failed(_))),"late desktop corruption escaped with a layer present");
 *BAD.lock().unwrap()=None;*HIGH_X.lock().unwrap()=true;
 assert_eq!(c.check_area(0,&[],None,[13,7,2559,1599]).unwrap(),2546*1592);
}
'''.replace('CONSTANTS',constants+'\n'+'#[derive(Default)]'+item(source,'struct CheckWorkspace {')+'\n'+item(source,'fn resize_readback(')+'\n'+(item(source,'fn equal_row(') if 'fn equal_row(' in source else '')).replace('@CHECK@',item(source,'pub fn check_area(')+'\n'+item(source,'fn check_area_into(')).replace('@EXPECTED@',item(source,'fn expected(').replace('-> u32 {', '-> u32 { PIXELS.fetch_add(1,std::sync::atomic::Ordering::Relaxed);', 1)))
if __name__=='__main__':unittest.main()
