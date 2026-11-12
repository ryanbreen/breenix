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
        scanout = source('userspace/programs/src/suite_breengel/window_cases/scanout.rs')
        helper = item(capture, 'pub fn readback_until(') if 'pub fn readback_until(' in capture else ''
        code = r'''
extern crate self as libbreenix;
extern crate self as libuitest;
use std::sync::Mutex;
#[derive(Default)] pub struct State { busy: bool, now: u64, calls: usize, polls: usize, closed: usize, fatal: bool, stall: bool, race: bool }
static STATE: Mutex<State> = Mutex::new(State { busy: false, now: 0, calls: 0, polls: 0, closed: 0, fatal: false, stall: false, race: false });
pub mod errno { #[derive(Debug, PartialEq)] pub enum Errno { EAGAIN, EINTR, EIO } }
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
mod breengel_caller {
 pub use crate::window::abi::{self,op};
 pub const EAGAIN:i32=11;
 fn screen_size()->Result<(usize,usize),String>{Ok((1,1))}
 mod super_handoff {pub type Readback=crate::window::abi::Readback;}
 pub fn call(op:u32,rb:&mut abi::Readback)->i64{match crate::window::call(op,rb){Ok(n)=>n as i64,Err(crate::error::Error::Os(crate::errno::Errno::EAGAIN))=>-(EAGAIN as i64),Err(_)=>-5}}
 RAW
 pub fn run()->Result<i64,String>{Ok(readback_raw()?.0)}
}
#[test]fn busy_capture_and_continuous_commit_ratio(){
 use window::ObservationKind;
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
        code = code.replace('BYTE_LEN', item(capture, 'fn byte_len(')).replace('HELPER', helper).replace('SCREEN', item(capture, 'pub fn screen(')).replace('RAW', raw)
        # Public data fixtures are intentionally available to both production callers.
        code = code.replace('#[derive(Default,Clone)]pub struct Record', '#[derive(Debug,Default,Clone)]pub struct Record')
        run_rust(self, code)


if __name__ == '__main__':
    unittest.main()
