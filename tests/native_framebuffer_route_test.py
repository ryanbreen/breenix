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
static ACTIVE:AtomicBool=AtomicBool::new(true);static CALLS:AtomicUsize=AtomicUsize::new(0);
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

if __name__=='__main__':unittest.main()
