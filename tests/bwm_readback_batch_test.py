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
static BAD:Mutex<Option<(u32,u32)>>=Mutex::new(None);
CONSTANTS
mod abi {pub mod record{pub const SIZE:usize=1024;}}
mod libbreenix {
 pub mod errno {#[derive(Debug)]pub enum Errno{EINVAL}}
 pub mod error {#[derive(Debug)]pub enum Error{Os(super::errno::Errno)} impl std::fmt::Display for Error{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{write!(f,"error")}}}
}
mod wabi {pub struct WinInfo{pub id:u64,pub width:u32,pub height:u32} impl WinInfo{pub const ZERO:Self=Self{id:0,width:0,height:0};} impl Clone for WinInfo{fn clone(&self)->Self{Self::ZERO}}}
mod window {
 pub struct Captured{pub commit_id:u64,pub width:u32,pub height:u32}
 pub fn capture(_:u64,_:&mut[u8],_:Option<()>)->Result<Captured,super::libbreenix::error::Error>{Err(super::libbreenix::error::Error::Os(super::libbreenix::errno::Errno::EINVAL))}
 pub fn list(_:&mut[super::wabi::WinInfo])->Result<usize,()>{Ok(0)}
}
fn pixel(x:u32,y:u32)->u32{0xff000000|((y*257+x*3)&0xffffff)}
fn fill(rect:[u32;4],out:&mut[u8],bad:bool){let[x,y,w,h]=rect;for row in 0..h{for col in 0..w{let mut p=pixel(x+col,y+row);if bad&&*BAD.lock().unwrap()==Some((x+col,y+row)){p^=1;}let at=((row*w+col)*4)as usize;out[at..at+4].copy_from_slice(&p.to_le_bytes());}}}
mod gpu {
 pub struct Answer{pub reason:u32}
 pub fn readback_why(_:u64,r:[u32;4],out:&mut[u8],stride:u32,_:&mut[u8;1024])->(i64,Answer){assert_eq!(stride,r[2]*4);super::CALLS.lock().unwrap().push(r);super::fill(r,out,true);(0,Answer{reason:0})}
}
struct Desktop;
impl Desktop{fn read_rect(&self,r:[usize;4],out:&mut[u8],stride:usize)->Result<(),i64>{assert_eq!(stride,r[2]*4);fill(r.map(|x|x as u32),out,false);Ok(())}}
struct Sample{ id:u64,width:u32,height:u32,commit_id:u64 }
struct Layer{window_id:u64}
#[derive(Clone,Copy)]struct Pointer{shape:u32}
fn pointer_bitmap(_:u32)->(){ }
fn intersect(_:[i32;4],_:[i32;4])->Option<[i32;4]>{None}
fn prefaulted_bytes(n:usize)->Vec<u8>{vec![0;n]}
#[derive(Debug)]enum CheckError{Failed(String),Moved(String)}
struct Compositor{sampled:Vec<Sample>,check_first:u64,desktop:Desktop}
impl Compositor {
 fn sampled_content(&self,_:&Layer)->Option<[i32;4]>{None}
 fn expected(&self,p:u32,_:&[Layer],_:&[(u64,[i32;4],u32,Vec<u8>)],_:Option<(Pointer,())>,_:i32,_:i32)->u32{p}
 @CHECK@
}
#[test]fn full_frame_and_offset_tail(){
 let mut c=Compositor{sampled:vec![],check_first:0,desktop:Desktop};
 assert_eq!(c.check_area(0,&[],None,[0,0,2560,1600]).unwrap(),4096000);
 let calls=CALLS.lock().unwrap().clone();
 assert!(calls.len()<=4,"32-row calls repeated the same transfer setup {} times",calls.len());
 assert!(calls.iter().all(|r| r[2] as usize*r[3] as usize*4<=4*1024*1024));
 CALLS.lock().unwrap().clear();
 assert_eq!(c.check_area(0,&[],None,[13,7,2559,1599]).unwrap(),2546*1592);
 let calls=CALLS.lock().unwrap();let mut y=7;for r in calls.iter(){assert_eq!((r[0],r[1],r[2]),(13,y,2546));y+=r[3];}assert_eq!(y,1599);drop(calls);
 *BAD.lock().unwrap()=Some((2558,1598));
 match c.check_area(0,&[],None,[13,7,2559,1599]){Err(CheckError::Failed(s))=>assert!(s.contains("pixel (2558, 1598)"),"{s}"),Err(CheckError::Moved(s))=>panic!("{s}"),Ok(n)=>panic!("missed late mismatch after {n}")}
}
'''.replace('CONSTANTS',constants).replace('@CHECK@',item(source,'pub fn check_area(')))
if __name__=='__main__':unittest.main()
