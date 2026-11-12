"""A kill between completed readback bands stops work and retains outstanding DMA."""
from pathlib import Path
import unittest
from suite_collection_test import run_rust
ROOT = Path(__file__).resolve().parents[1]

class ReadbackCancel(unittest.TestCase):
    def test_kill_between_ready_bands_uses_the_error_cleanup(self):
        source = (ROOT/'kernel/src/gpu/mod.rs').read_text()
        begin = source.index('    let mut lanes: [Option<Band>; 2]')
        end = source.index('    let (_, fence, answer)', begin)
        run_rust(self, r'''
use std::sync::{Mutex,atomic::{AtomicBool,Ordering}};
const EINTR:i64=4;
mod readback_reason{pub const NONE:u32=0;}
static KILLED:AtomicBool=AtomicBool::new(false);
static STARTS:Mutex<Vec<(usize,usize)>>=Mutex::new(Vec::new());
static ABANDONED:Mutex<Vec<u64>>=Mutex::new(Vec::new());
mod syscall{pub fn check_signals_for_wait()->Option<i32>{crate::KILLED.load(crate::Ordering::Relaxed).then_some(9)}}
struct Sent{serial:u64,other:Option<u32>}
struct Band{index:usize,by:usize,rows:usize,pass:usize,sent:Option<Sent>}
mod format{pub const R8G8B8A8:u32=99;}
struct Pages;impl Pages{fn try_spans(&self,_at:usize,_len:usize,_f:impl FnMut(usize,*mut u8,usize)->Result<(),()>)->Result<(),()>{panic!("not an RGBA fixture")}}
struct Core;impl Core{fn abandon(&mut self,serial:u64){ABANDONED.lock().unwrap().push(serial);}}
fn with_core<R>(f:impl FnOnce(&mut Core)->R)->Option<R>{Some(f(&mut Core))}
fn release_other(rid:Option<u32>){assert_eq!(rid,Some(7));}
fn observe()->Result<(),(i64,u32)>{
 let (bands,band_rows,height,row,fmt)=(4usize,1usize,4usize,4usize,0u32);let staging=Pages;
 let start=|b:&mut Band|{STARTS.lock().unwrap().push((b.index,b.pass));b.sent=Some(Sent{serial:10+b.index as u64,other:Some(7)});Ok(())};
 let finish=|b:&mut Band|{b.sent.take();Ok(())};
 // Every transfer is ready immediately: no sleep path inspects pending signals.
 let read=|_:&Band|{KILLED.store(true,Ordering::Relaxed);};
 @BODY@
 Ok(())
}
#[test]fn pending_kill_stops_the_complement_and_abandons_the_other_lane(){
 let _=EINTR;let _=readback_reason::NONE;let _=syscall::check_signals_for_wait;
 assert_eq!(observe(),Err((EINTR,readback_reason::NONE)),"ready completions hid SIGKILL until the whole readback ended");
 assert_eq!(*STARTS.lock().unwrap(),vec![(0,0),(1,0)],"readback sent fresh work after SIGKILL");
 assert_eq!(*ABANDONED.lock().unwrap(),vec![11],"outstanding transfer must retain its DMA holds via abandon");
}
'''.replace('@BODY@',source[begin:end]))
if __name__=='__main__':unittest.main()
