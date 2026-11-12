"""Run the production capture and BreenGL readback callers at a forced busy present."""
from pathlib import Path
import os
import subprocess
import unittest

from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


def source(path):
    old = os.environ.get('UITEST_OLD_COMMIT')
    return subprocess.check_output(['git', 'show', f'{old}:{path}'], cwd=ROOT, text=True) if old else (ROOT / path).read_text()


class CaptureBusy(unittest.TestCase):
    def test_capture_waits_for_completion_without_resetting_deadline(self):
        capture = source('libs/libuitest/src/capture.rs')
        window = source('libs/libbreenix/src/window.rs')
        scanout = source('userspace/programs/src/suite_breengel/window_cases/scanout.rs')
        owner = window if 'pub fn readback_until(' in window else capture
        helper = item(owner, 'pub fn readback_until(') if 'pub fn readback_until(' in owner else ''
        direct = item(window, 'pub fn readback(') if 'pub fn readback(' in window else 'pub fn readback(rb: &mut abi::Readback) -> Result<(), Error> { call(abi::op::SCANOUT_READBACK, rb).map(|_| ()) }'
        code = r'''
extern crate self as libbreenix;
extern crate self as libuitest;
use std::sync::Mutex;
#[derive(Default)] pub struct State { busy: bool, now: u64, calls: usize, polls: usize, closed: usize, fatal: bool, stall: bool, race: bool }
static STATE: Mutex<State> = Mutex::new(State { busy: false, now: 0, calls: 0, polls: 0, closed: 0, fatal: false, stall: false, race: false });
pub mod errno { #[derive(Debug, PartialEq)] pub enum Errno { EAGAIN = 11, EINTR = 4, EIO = 5, ETIMEDOUT = 110 } }
pub mod error { #[derive(Debug, PartialEq)] pub enum Error { Os(super::errno::Errno) } impl std::fmt::Display for Error { fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{write!(f,"{self:?}")} } }
pub mod types { #[derive(Clone,Copy)] pub struct Fd(pub u64); impl Fd {pub fn from_raw(v:u64)->Self{Self(v)} pub fn raw(self)->u64{self.0}} pub struct OwnedFd(Fd); impl OwnedFd {pub fn new(fd:Fd)->Self{Self(fd)} pub fn raw(&self)->u64{self.0.raw()} pub fn fd(&self)->Fd{self.0}} impl Drop for OwnedFd {fn drop(&mut self){super::STATE.lock().unwrap().closed+=1;}} }
pub mod time {pub struct Stamp(u64);impl Stamp{pub fn as_nanos(&self)->i128{self.0 as i128}}pub fn now_monotonic()->Result<Stamp,super::error::Error>{Ok(Stamp(super::STATE.lock().unwrap().now))}}
pub mod io {
 pub mod poll_events {pub const POLLIN:i16=1;}
 pub struct PollFd {pub fd:i32,pub events:i16,pub revents:i16}
 pub fn read(_:super::types::Fd,_:&mut[u8])->Result<usize,super::error::Error>{Ok(8)}
 pub fn poll_ns(_: &mut[PollFd],left:Option<u64>)->Result<usize,super::error::Error>{
  let mut s=super::STATE.lock().unwrap();s.polls+=1;
  if s.stall{s.now+=left.unwrap();return Ok(0)}
  assert!(s.busy || s.race,"wait must follow a transient refusal");s.busy=false;s.race=false;s.now+=10;Ok(1)
 }
}
pub mod gpu {pub mod abi {pub mod record{pub const SERIAL:usize=8;pub const COMPOSITE_SERIAL:usize=176;pub const COMMIT:usize=32;}} pub struct Mode{pub width:u32,pub height:u32} pub fn current_mode()->Result<Mode,super::error::Error>{Ok(Mode{width:1,height:1})}}
pub mod window {
 use crate::error::Error;
 DIRECT
 SHARED_HELPER
 pub mod abi {
  pub mod op{pub const SCANOUT_READBACK:u32=22;pub const PRESENTATION_FD:u32=36;}
  #[derive(Default)] pub struct WinId{pub id:u64}
  #[derive(Default)] pub struct Readback{pub dst:u64,pub dst_len:u64,pub record:u64,pub record_len:u32,pub serial:u64}
  pub trait Arg{fn invoke(&mut self,op:u32)->Result<u64,super::super::error::Error>;}
  impl Arg for WinId{fn invoke(&mut self,op:u32)->Result<u64,super::super::error::Error>{assert_eq!(op,op::PRESENTATION_FD);Ok(7)}}
  impl Arg for Readback{fn invoke(&mut self,op:u32)->Result<u64,super::super::error::Error>{
   use super::super::{STATE,error::Error,errno::Errno};assert_eq!(op,op::SCANOUT_READBACK);
   let mut s=STATE.lock().unwrap();s.calls+=1;
   if s.fatal{return Err(Error::Os(Errno::EIO))}
   if s.busy {if s.race{s.busy=false;} return Err(Error::Os(Errno::EAGAIN))}
   self.serial=s.calls as u64;
   // The fake device writes a completed record and its matching pixel.
   unsafe {let rec=std::slice::from_raw_parts_mut(self.record as *mut u8,1024);rec.fill(0);rec[8..16].copy_from_slice(&self.serial.to_le_bytes());rec[176..184].copy_from_slice(&self.serial.to_le_bytes());*(self.dst as *mut u8)=self.serial as u8;}
   Ok(0)
  }}
 }
 #[derive(Clone,Copy)] pub enum ObservationKind{Composed}
 pub fn validate_scanout_observation(_: &[u8;1024],_:u32,_:u32,_:ObservationKind)->bool{true}
 pub fn call<T:abi::Arg>(op:u32,arg:&mut T)->Result<u64,super::error::Error>{arg.invoke(op)}
}
pub mod arena {
 #[derive(Default,Clone)]pub struct Record{pub check:String,pub width:u32,pub height:u32,pub composite:u64,pub commit:u64,pub observation:Vec<u8>}
 pub enum Slot{Screen}
 pub struct Arena{pub bytes:Vec<u8>,pub published:bool}
 pub struct Writer<'a>(&'a mut Arena);
 impl Arena{pub fn begin(&mut self,_:Slot)->Result<Writer<'_>,String>{Ok(Writer(self))}}
 impl Writer<'_>{pub fn bytes_mut(&mut self)->&mut[u8]{&mut self.0.bytes}pub fn publish(self,_:Record,_:usize)->Result<(),String>{self.0.published=true;Ok(())}}
}
pub mod settle {pub const BOUND_NS:u64=2_000_000_000;}
pub mod capture {
 use crate::{arena::{Arena,Record,Slot},window::{self,abi,ObservationKind},gpu};
 BYTE_LEN
 HELPER
 SCREEN
 fn screen_windows(_: &Record)->Result<Vec<(u64,u64)>,String>{Ok(vec![])}
}
pub mod breengel_caller {
 pub use crate::window::abi::{self,op};
 pub const EAGAIN:i32=11;
 fn screen_size()->Result<(usize,usize),String>{Ok((1,1))}
 pub mod super_handoff {pub type Readback=crate::window::abi::Readback;}
 pub fn call(op:u32,rb:&mut abi::Readback)->i64{match crate::window::call(op,rb){Ok(n)=>n as i64,Err(crate::error::Error::Os(crate::errno::Errno::EAGAIN))=>-(EAGAIN as i64),Err(_)=>-5}}
 RAW
 pub fn run()->Result<i64,String>{Ok(readback_raw()?.0)}
}
#[test]fn busy_capture_and_continuous_commit_ratio(){
 use window::ObservationKind;
 *STATE.lock().unwrap()=State{busy:true,..Default::default()};
 let mut pixels=[0u8;4];let mut record=[0u8;1024];
 let mut rb=window::abi::Readback{dst:pixels.as_mut_ptr() as u64,dst_len:4,record:record.as_mut_ptr() as u64,record_len:1024,..Default::default()};
 assert!(window::readback(&mut rb).is_ok(),"direct scanout consumers must survive present.busy");
 assert_eq!(STATE.lock().unwrap().polls,1);
 *STATE.lock().unwrap()=State{busy:true,..Default::default()};
 let mut a=arena::Arena{bytes:vec![0;4],published:false};
 let result=capture::screen(&mut a,"busy",ObservationKind::Composed);
 assert!(result.is_ok(),"capture must survive present.busy: {result:?}");
 assert!(a.published);assert_eq!(STATE.lock().unwrap().calls,2);assert_eq!(STATE.lock().unwrap().polls,1);
 // Twenty requests each arrive during a present; every completion wakes the caller.
 let mut checked=0;
 for _ in 0..20 {STATE.lock().unwrap().busy=true;if breengel_caller::run().unwrap()==0{checked+=1;}}
 assert!(checked>=10,"BreenGL 20.1 needs >=10/20 successful readbacks; got {checked}");
 // A completion between EAGAIN and ppoll must not be lost.
 *STATE.lock().unwrap()=State{busy:true,race:true,..Default::default()};
 assert!(capture::screen(&mut a,"race",ObservationKind::Composed).is_ok());
 // A stuck present consumes the original bound once; it cannot restart it.
 *STATE.lock().unwrap()=State{busy:true,stall:true,..Default::default()};
 a.published=false;assert!(capture::screen(&mut a,"timeout",ObservationKind::Composed).is_err());assert!(!a.published);
 {let s=STATE.lock().unwrap();assert_eq!(s.now,settle::BOUND_NS);assert_eq!(s.polls,1);assert_eq!(s.closed,1);}
 *STATE.lock().unwrap()=State{fatal:true,..Default::default()};
 assert!(capture::screen(&mut a,"fatal",ObservationKind::Composed).is_err());assert_eq!(STATE.lock().unwrap().calls,1);assert_eq!(STATE.lock().unwrap().polls,0);
}
'''
        # Adapt only the fixture's namespace for the real raw caller's shared ABI.
        raw = item(scanout, 'fn readback_raw(').replace('super::handoff::Readback::ZERO', 'super_handoff::Readback::default()')
        code += '\npub mod phases {\n' + (ROOT / 'libs/libuitest/src/phases.rs').read_text().replace('//!', '//') + '\n}\n'
        code = code.replace('DIRECT', direct).replace('SHARED_HELPER', helper if owner == window else '')
        code = code.replace('BYTE_LEN', item(capture, 'fn byte_len(')).replace('HELPER', helper).replace('SCREEN', item(capture, 'pub fn screen(')).replace('RAW', raw)
        # Public data fixtures are intentionally available to both production callers.
        code = code.replace('#[derive(Default,Clone)]pub struct Record', '#[derive(Debug,Default,Clone)]pub struct Record')
        run_rust(self, code)

    def test_production_completion_descriptor_retains_wake(self):
        events = source('kernel/src/window/events.rs')
        completion = events[events.index('static COMPLETIONS:'):]
        scene = source('kernel/src/window/scene.rs')
        release = item(scene, 'impl Drop for Admission')
        code = r'''
#![feature(allocator_api)]
use std::sync::Arc;
pub mod spin {pub struct Once<T>(std::sync::OnceLock<T>);impl<T> Once<T>{pub const fn new()->Self{Self(std::sync::OnceLock::new())}pub fn get(&self)->Option<&T>{self.0.get()}pub fn call_once(&self,f:impl FnOnce()->T)->&T{self.0.get_or_init(f)}}impl<T> Default for Once<T>{fn default()->Self{Self::new()}}}
pub mod breengel_abi {pub mod errno {pub const ENOMEM:i32=12;pub const EINVAL:i32=22;pub const EFAULT:i32=14;}}
pub struct WaitQueueHead(std::sync::atomic::AtomicUsize);
impl Default for WaitQueueHead{fn default()->Self{Self::new()}}
impl WaitQueueHead {pub fn new()->Self{Self(std::sync::atomic::AtomicUsize::new(0))}pub fn wake_up(&self){assert!(!REGISTRY.lock().unwrap().present.busy);self.0.fetch_add(1,std::sync::atomic::Ordering::Relaxed);}}
pub mod ipc {
 pub mod poll {pub mod events {pub const POLLIN:i16=1;}}
 pub mod object {pub trait IpcFile {fn as_any(&self)->&dyn core::any::Any;fn fill_stat(&self,st:&mut crate::syscall::fs::Stat);fn poll_events(&self)->i16;fn poll_queue(&self)->Option<std::sync::Arc<crate::WaitQueueHead>>;fn read(&self,buf:u64,count:usize)->Result<u64,u64>;}}
}
pub mod syscall {pub mod fs {pub struct Stat{pub st_mode:u32,pub st_nlink:u32,pub st_blksize:u32}}pub mod userptr {pub fn copy_to_user(dst:*mut u64,src:&u64)->Result<(),u64>{if dst.is_null(){Err(14)}else{unsafe{dst.write(*src)};Ok(())}}}}
struct Present{busy:bool}impl Present{fn request_full(&mut self){}}
struct Registry{present:Present}
static REGISTRY:std::sync::Mutex<Registry>=std::sync::Mutex::new(Registry{present:Present{busy:false}});
mod events {
 use super::*;
 COMPLETION
}
struct Admission{serial:u64}
RELEASE
#[test]fn completion_is_level_triggered_after_busy_clears(){
 use ipc::object::IpcFile;
 let file=events::PresentationFile::new().unwrap();
 assert_eq!(file.poll_events(),0);
 REGISTRY.lock().unwrap().present.busy=true;
 let admission=Admission{serial:7};
 assert_eq!(file.poll_events(),0);
 drop(admission);
 // Finish before ppoll registers: the generation retains readiness.
 assert_eq!(file.poll_events(),1);
 assert_eq!(file.poll_queue().unwrap().0.load(std::sync::atomic::Ordering::Relaxed),1);
 assert_eq!(file.read(0,8),Err(14));assert_eq!(file.poll_events(),1);
 let mut generation=0u64;assert_eq!(file.read(&mut generation as *mut u64 as u64,8),Ok(8));
 assert_eq!(generation,1);assert_eq!(file.poll_events(),0);
 // Releasing a failed admission is also a reason to recheck, not a pixel result.
 REGISTRY.lock().unwrap().present.busy=true;drop(Admission{serial:0});
 assert_eq!(file.poll_events(),1);
}
'''
        code = code.replace('COMPLETION', completion).replace('RELEASE', release.replace('REGISTRY.lock()', 'REGISTRY.lock().unwrap()').replace('super::events::completed()', 'events::completed()'))
        run_rust(self, code)


if __name__ == '__main__':
    unittest.main()
