"""Run the production pixel checker with deterministic independent readbacks."""
from pathlib import Path
import os
import subprocess
import re
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT = Path(__file__).resolve().parents[1]
class CaptureTree(unittest.TestCase):
    def test_tree_and_pixels_share_the_sampled_capture(self):
        path = 'userspace/programs/src/bwm/compositor.rs'
        old = os.environ.get('UITEST_OLD_COMMIT')
        source = subprocess.check_output(['git','show',f'{old}:{path}'],cwd=ROOT,text=True) if old else (ROOT/path).read_text()
        constants = '\n'.join(re.findall(r'^const CHECK_BAND_.*?;', source, re.M))
        check = item(source, 'pub fn check_area(')
        if 'want_tree: bool' not in check:
            check = check.replace('pub fn check_area(', 'fn old_check_area(', 1) + '''
    pub fn check_area(&mut self, subject:u64, layers:&[Layer], pointer:Option<Pointer>, area:[u32;4], _want_tree:bool)->Result<(u64,Option<(u64,Vec<u8>)>),CheckError>{
        self.old_check_area(subject,layers,pointer,area).map(|n|(n,None))
    }
'''
        workspace = ''
        if 'fn check_area_with(' in source:
            workspace = '#[derive(Default)]' + item(source, 'struct CheckWorkspace {') + '\n' + item(source, 'fn resize_readback(') + '\n' + item(source, 'fn client_buffer(')
            check += '\n' + item(source, 'fn check_area_with(') + '\n' + item(source, 'fn check_area_into(')
        run_rust(self, r'''
use std::sync::Mutex;
static CALLS:Mutex<Vec<[u32;4]>>=Mutex::new(Vec::new());
static CAPTURES:Mutex<Vec<(u64,usize)>>=Mutex::new(Vec::new());
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
mod wabi {pub mod limits {pub const MAX_TREE_KIB:u32=1024;} pub struct WinInfo{pub id:u64,pub width:u32,pub height:u32} impl WinInfo{pub const ZERO:Self=Self{id:0,width:0,height:0};} impl Clone for WinInfo{fn clone(&self)->Self{Self::ZERO}}}
mod window {
 pub struct Captured{pub commit_id:u64,pub width:u32,pub height:u32}
 pub fn capture(id:u64,out:&mut[u8],tree:Option<&mut[u8]>)->Result<Captured,super::libbreenix::error::Error>{let len=tree.as_ref().map_or(0,|t|t.len());super::CAPTURES.lock().unwrap().push((id,len));if let Some(t)=tree {t[..4].copy_from_slice(b"tree");}super::fill([if id==7 {0}else{2},0,2,2],out,false);Ok(Captured{commit_id:42,width:2,height:2})}
 pub fn list(_:&mut[super::wabi::WinInfo])->Result<usize,()>{Ok(0)}
}
fn pixel(x:u32,y:u32)->u32{0xff000000|((y*257+x*3)&0xffffff)}
fn fill(rect:[u32;4],out:&mut[u8],bad:bool){let[x,y,w,h]=rect;for row in 0..h{for col in 0..w{let mut p=pixel(x+col,y+row);if bad&&*POINTER.lock().unwrap() && (3..7).contains(&(x+col)) && (3..7).contains(&(y+row)) {p=if ((x+col-3)/2+(y+row-3)/2)%2==0{0xff000000}else{0xffffffff};}if bad&&*HIGH_X.lock().unwrap(){p^=0xff000000;}if bad&&*BAD.lock().unwrap()==Some((x+col,y+row)){p^=1;}let at=((row*w+col)*4)as usize;out[at..at+4].copy_from_slice(&p.to_le_bytes());}}}
mod gpu {
 pub struct Answer{pub reason:u32}
 pub fn readback_why(_:u64,r:[u32;4],out:&mut[u8],stride:u32,_:&mut[u8;1024])->(i64,Answer){assert_eq!(stride,r[2]*4);super::CALLS.lock().unwrap().push(r);super::fill(r,out,true);(0,Answer{reason:0})}
}
struct Desktop;
impl Desktop{fn read_rect(&self,r:[usize;4],out:&mut[u8],stride:usize)->Result<(),i64>{assert_eq!(stride,r[2]*4);fill(r.map(|x|x as u32),out,false);Ok(())}}
struct Sample{ id:u64,width:u32,height:u32,commit_id:u64 }
struct Layer{window_id:u64,frame:[[i32;4];3],frame_len:usize}
#[derive(Clone,Copy)]struct Pointer{x:i32,y:i32,shape:u32}
const CURSOR_WHITE:u32=0xffffffff;const CURSOR_BLACK:u32=0xff000000;
fn pointer_bitmap(_:u32)->&'static[&'static[u8]]{&[b"#o",b"o#"]}
fn hotspot(_:u32)->(i32,i32){HOTSPOTS.fetch_add(1,std::sync::atomic::Ordering::Relaxed);(0,0)}
fn intersect(a:[i32;4],b:[i32;4])->Option<[i32;4]>{let r=[a[0].max(b[0]),a[1].max(b[1]),a[2].min(b[2]),a[3].min(b[3])];(r[0]<r[2]&&r[1]<r[3]).then_some(r)}
fn monotonic_ns()->u64{0}
fn prefaulted_bytes(n:usize)->Vec<u8>{vec![0;n]}
#[derive(Debug)]enum CheckError{Failed(String),Moved(String)}
struct Compositor{sampled:Vec<Sample>,check_first:u64,check_workspace:CheckWorkspace,desktop:Desktop,scale:i32,appbar_top:i32}
impl Compositor {
 fn sampled_content(&self,l:&Layer)->Option<[i32;4]>{Some(if l.window_id==7 {[0,0,2,2]} else {[2,0,4,2]})}
 @EXPECTED@
 @CHECK@
}
#[test]fn tree_is_copied_with_subject_pixels(){
 let _ = libbreenix::error::Error::Os(libbreenix::errno::Errno::EINVAL);
 let _ = wabi::limits::MAX_TREE_KIB;
 let describe = |e:CheckError| match e {CheckError::Failed(s)|CheckError::Moved(s)=>s};
 let mut c=Compositor{sampled:vec![Sample{id:7,width:2,height:2,commit_id:42},Sample{id:8,width:2,height:2,commit_id:42}],check_first:0,check_workspace:CheckWorkspace::default(),desktop:Desktop,scale:1,appbar_top:2};
 let layers=[Layer{window_id:7,frame:[[0;4];3],frame_len:0},Layer{window_id:8,frame:[[0;4];3],frame_len:0}];
 let (count,tree)=c.check_area(7,&layers,None,[0,0,4,2],true).unwrap_or_else(|e|panic!("{}",describe(e)));
 assert_eq!(count,8);
 assert_eq!(*CAPTURES.lock().unwrap(),vec![(7,1024*1024),(8,0)]);
 let (commit,bytes)=tree.expect("subject tree and commit");
 assert_eq!(commit,42);assert_eq!(&bytes[..4],b"tree");
 CAPTURES.lock().unwrap().clear();
 let (_,tree)=c.check_area(7,&layers,None,[0,0,4,2],false).unwrap_or_else(|e|panic!("{}",describe(e)));
 assert!(tree.is_none());assert_eq!(*CAPTURES.lock().unwrap(),vec![(7,0),(8,0)]);
}
'''.replace('CONSTANTS',constants+'\n'+workspace+'\n'+(item(source,'fn equal_row(') if 'fn equal_row(' in source else '')).replace('@CHECK@',check).replace('@EXPECTED@',item(source,'fn expected(')))
if __name__=='__main__':unittest.main()
