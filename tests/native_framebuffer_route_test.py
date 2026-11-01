"""Execute the legacy framebuffer syscall after native GPU admission."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
sys.path.insert(0, str(Path(__file__).parent))
from gt710_gr_live_unwind_test import item
ROOT = Path(__file__).resolve().parents[1]

class NativeFramebuffer(unittest.TestCase):
    def test_gpu_owner_flush_and_privileged_read_use_the_live_gpu(self):
        source = (ROOT/'kernel/src/syscall/graphics.rs').read_text()
        fixture = r'''
use std::sync::atomic::{AtomicBool,AtomicUsize,Ordering};
#[derive(Debug,PartialEq)] pub enum SyscallResult {Ok(u64),Err(u64)}
mod errors {pub enum ErrorCode {Fault=14,InvalidArgument=22,PermissionDenied=1,Busy=16}}
use errors::ErrorCode;
#[derive(Clone,Copy)] struct FbDrawCmd {op:u32,p1:i32,p2:i32,p3:i32,p4:i32}
#[derive(Clone,Copy)] pub struct Pane {pub whole_screen:bool,pub user_addr:u64,pub user_stride:usize,pub height:usize,pub width:usize}
static OWNER:AtomicBool=AtomicBool::new(true);static ROOT_USER:AtomicBool=AtomicBool::new(true);
static ACTIVE:AtomicBool=AtomicBool::new(true);static RETIRED:AtomicBool=AtomicBool::new(false);static CALLS:AtomicUsize=AtomicUsize::new(0);
fn owns_display(_:u64,_:&process::Process)->bool{OWNER.load(Ordering::SeqCst)}
mod process {
 pub struct Cred {pub euid:u32}
 pub struct Process {pub fb_mmap:Option<super::Pane>,pub cred:Cred}
 pub struct Manager {p:Process}
 impl Manager {pub fn find_process_by_thread(&self,_:u64)->Option<(u64,&Process)>{Some((1,&self.p))}}
 pub fn manager()->Box<Option<Manager>>{Box::new(Some(Manager{p:Process{fb_mmap:Some(super::Pane{whole_screen:true,user_addr:4096,user_stride:32,height:4,width:8}),cred:Cred{euid:if super::ROOT_USER.load(super::Ordering::SeqCst){0}else{99}}}}))}
}
mod syscall {
 pub mod memory_common {pub fn get_current_thread_id()->Option<u64>{Some(1)}}
 pub mod userptr {pub fn copy_from_user<T:Copy>(p:*const T)->Result<T,()>{Ok(unsafe{*p})}}
}
mod console {
 pub fn display_geometry()->Option<(usize,usize,usize,usize,bool)>{Some((8,4,8,4,true))}
 pub fn read_display_pixel(_:usize,_:usize)->Option<u32>{panic!("GPU read used CPU framebuffer")}
 pub unsafe fn copy_to_display(_: *const u8,_:usize,_:std::ops::Range<usize>,_:std::ops::Range<usize>)->bool{false}
 pub mod vt {pub struct Presentation;impl Presentation {pub fn begin(_:bool)->Option<Self>{Some(Self)}}}
}
mod gpu {
 pub mod console {pub fn active()->bool{super::super::RETIRED.load(super::super::Ordering::SeqCst)}}
 pub fn display_active()->bool{super::ACTIVE.load(super::Ordering::SeqCst)}
 pub mod framebuffer {
  pub fn flush(_:super::super::Pane,rows:std::ops::Range<usize>,cols:std::ops::Range<usize>)->Result<(),i64>{
   assert_eq!(rows,2..4);assert_eq!(cols,6..8);super::super::CALLS.fetch_add(1,super::super::Ordering::SeqCst);Ok(())}
  pub fn pixel(x:usize,y:usize)->Result<u32,i64>{assert_eq!((x,y),(6,2));Ok(0x123456)}
 }
}
// SYSCALL
fn main(){
 let mut cmd=FbDrawCmd{op:6,p1:6,p2:2,p3:2,p4:2};
 let call=|c:&FbDrawCmd|sys_fbdraw(c as *const _ as u64);
 assert_eq!(call(&cmd),SyscallResult::Ok(0),"GPU owner flush must present its private pane");
 assert_eq!(CALLS.load(Ordering::SeqCst),1);
 OWNER.store(false,Ordering::SeqCst);assert_eq!(call(&cmd),SyscallResult::Err(1));
 assert_eq!(CALLS.load(Ordering::SeqCst),1);OWNER.store(true,Ordering::SeqCst);
 cmd.op=32;assert_eq!(call(&cmd),SyscallResult::Ok(0x123456));
 ROOT_USER.store(false,Ordering::SeqCst);assert_eq!(call(&cmd),SyscallResult::Err(1));
 ROOT_USER.store(true,Ordering::SeqCst);cmd.op=6;ACTIVE.store(false,Ordering::SeqCst);
 assert_eq!(call(&cmd),SyscallResult::Err(16),"pre-admission CPU-busy result remains");
 RETIRED.store(true,Ordering::SeqCst);
 assert_eq!(call(&cmd),SyscallResult::Ok(0),"retirement keeps GPU routing even when admission is lost");
 let _=(gpu::display_active as fn()->bool,gpu::framebuffer::pixel as fn(usize,usize)->Result<u32,i64>);
 let _=gpu::framebuffer::flush as fn(Pane,std::ops::Range<usize>,std::ops::Range<usize>)->Result<(),i64>;
}
'''
        # The production function is a child of syscall, whose parent exports these types.
        function = item(source,'pub fn sys_fbdraw(').replace('super::ErrorCode','ErrorCode')
        fixture=fixture.replace('// SYSCALL',function)
        prefix=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'route.rs').write_text(fixture)
            subprocess.run(prefix+['rustc','--edition=2021','-Dwarnings',str(p/'route.rs'),'-o',str(p/'route')],check=True)
            subprocess.run([str(p/'route')],check=True)

    def test_pitched_rows_keep_other_pixels_and_copy_faults_fail(self):
        source=(ROOT/'kernel/src/gpu/framebuffer.rs').read_text()
        fixture=r'''
use std::ops::Range;
use std::cell::RefCell;
const EFAULT:i64=14;
struct Pages(RefCell<Vec<u8>>);
impl Pages {fn write(&self,at:usize,data:&[u8]){self.0.borrow_mut()[at..at+data.len()].copy_from_slice(data);}}
#[derive(Clone,Copy)] struct FbMmapInfo {user_addr:u64,user_stride:usize}
mod syscall {pub mod userptr {
 pub fn read_user_bytes(dst:*mut u8,src:u64,n:usize)->Result<(),u64>{
  if src==0 {return Err(14);}
  unsafe{std::ptr::copy_nonoverlapping(src as *const u8,dst,n);}Ok(())
 }
}}
// COPY
fn main(){
 let input:Vec<u8>=(0..80).collect();let pages=Pages(RefCell::new(vec![0x77;128]));
 let pane=FbMmapInfo{user_addr:input.as_ptr() as u64,user_stride:20};
 assert_eq!(copy_rows(&pages,32,pane,1..3,1..3),Ok(()));
 let out=pages.0.borrow();
 assert_eq!(&out[36..44],&[24,25,26,255,28,29,30,255]);
 assert_eq!(&out[68..76],&[44,45,46,255,48,49,50,255]);
 assert!(out[..36].iter().chain(&out[44..68]).chain(&out[76..]).all(|b|*b==0x77));drop(out);
 assert_eq!(copy_rows(&pages,32,FbMmapInfo{user_addr:0,user_stride:20},0..1,0..1),Err(EFAULT));
 assert_eq!(&pages.0.borrow()[..4],&[0x77;4]);
}
'''
        fixture=fixture.replace('// COPY',item(source,'fn copy_rows('))
        prefix=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'rows.rs').write_text(fixture)
            subprocess.run(prefix+['rustc','--edition=2021','-Dwarnings',str(p/'rows.rs'),'-o',str(p/'rows')],check=True)
            subprocess.run([str(p/'rows')],check=True)

if __name__=='__main__':unittest.main()
