"""Replay the retained boot's control-close, descriptor and input mechanisms."""
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


class Repair(unittest.TestCase):
    def test_unread_startup_reply_reset_is_case_peer_death(self):
        body = item(source('libs/libuitest/src/channel.rs'), 'pub fn receive(')
        run_rust(self, r'''
#[derive(Debug)]enum Errno{EINTR,EAGAIN,ECONNRESET,EIO}
#[derive(Debug)]enum Error{Os(Errno)}
impl std::fmt::Display for Error{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{write!(f,"{self:?}")}}
enum Message{Ready}
struct Decoder;impl Decoder{fn next(&mut self)->Result<Option<Message>,String>{Ok(None)}fn push(&mut self,_:&[u8])->Result<(),String>{Ok(())}}
mod io{use super::*;pub fn read(fd:u64,_:&mut[u8])->Result<usize,Error>{match fd{0=>Ok(0),1=>Err(Error::Os(Errno::ECONNRESET)),2=>Err(Error::Os(Errno::EIO)),_=>Err(Error::Os(Errno::EAGAIN))}}}
struct Channel{fd:u64,decoder:Decoder}
impl Channel{BODY}
#[test]fn independent_collector_survives_case_close(){
 let _=(Errno::EINTR,Message::Ready);
 for fd in [0,1]{let mut c=Channel{fd,decoder:Decoder};
 assert_eq!(c.receive().err().as_deref(),Some("session supervisor/control peer closed"));}
 assert!(Channel{fd:2,decoder:Decoder}.receive().unwrap_err_for_test().contains("EIO"));
 assert!(Channel{fd:3,decoder:Decoder}.receive().unwrap().is_none());
}
trait ErrorText{fn unwrap_err_for_test(self)->String;}impl ErrorText for Result<Option<Message>,String>{fn unwrap_err_for_test(self)->String{self.err().expect("expected I/O error")}}
'''.replace('BODY', body))

    def test_spawn_preserves_only_targets_without_scanning_empty_fds(self):
        body = item(source('libs/libbreenix/src/suite_spawn.rs'), 'fn arrange(')
        run_rust(self, r'''
use std::{collections::BTreeMap,sync::Mutex};
#[derive(Clone,Copy)]struct Fd(u64);impl Fd{fn from_raw(n:u64)->Self{Self(n)}fn raw(self)->u64{self.0}}
#[derive(Debug)]struct Error;
type Keep<'a>=&'a[(Fd,Fd)];
fn descriptor_limit()->u64{4096}
static TABLE:Mutex<BTreeMap<u64,u64>>=Mutex::new(BTreeMap::new());
static CALLS:Mutex<usize>=Mutex::new(0);
mod io{use super::*;
 pub fn dup2(a:Fd,b:Fd)->Result<Fd,Error>{*CALLS.lock().unwrap()+=1;let mut t=TABLE.lock().unwrap();let value=*t.get(&a.raw()).ok_or(Error)?;t.insert(b.raw(),value);Ok(b)}
 pub fn close(fd:Fd)->Result<(),Error>{*CALLS.lock().unwrap()+=1;TABLE.lock().unwrap().remove(&fd.raw());Ok(())}
 pub fn close_range(first:u32,last:u32)->Result<(),Error>{*CALLS.lock().unwrap()+=1;TABLE.lock().unwrap().retain(|&n,_|n<u64::from(first)||n>u64::from(last));Ok(())}
}
BODY
#[test]fn sparse_high_fds_and_overlapping_moves(){
 // A lowered soft limit must not leak a descriptor opened before the lowering.
 for keep in [vec![],vec![(Fd(4),Fd(3)),(Fd(3),Fd(4))],vec![(Fd(4000),Fd(7)),(Fd(5),Fd(1))]]{
  let before=BTreeMap::from([(0,10),(1,11),(2,12),(3,13),(4,14),(5,15),(4000,16)]);
  *TABLE.lock().unwrap()=before.clone();*CALLS.lock().unwrap()=0;
  arrange(&keep).unwrap();let mut expected=before.clone();
  expected.retain(|&n,_|n<3);for &(from,to) in &keep{expected.insert(to.raw(),before[&from.raw()]);}
  assert_eq!(*TABLE.lock().unwrap(),expected);
  assert!(*CALLS.lock().unwrap()<=3*keep.len()+1,"spawn issued thousands of empty close syscalls");
 }
 // Both APIs belong to the shim even on the historical implementation.
 let _=(io::close as fn(Fd)->Result<(),Error>,io::close_range as fn(u32,u32)->Result<(),Error>);
}
'''.replace('BODY', body))

    def test_kiosk_consumes_and_disposes_grabbed_session_input(self):
        text = source('userspace/programs/src/kiosk_test.rs')
        self.assertIn('fn drain_session_input(', text, 'session kiosk grabs input but never consumes it')
        body = item(text, 'fn drain_session_input(')
        run_rust(self, r'''
use std::sync::Mutex;
extern crate self as libbreenix;
pub mod error{#[derive(Debug)]pub enum Error{Os(crate::errno::Errno)}}
pub mod errno{#[derive(Debug)]pub enum Errno{EAGAIN,EINTR,EIO}}
pub mod input{
 use super::*;pub mod disposition{pub const WM_HANDLED:u32=1;}
 #[derive(Default)]pub struct Disposition{pub serial:u64,pub disposition:u32}
 pub struct Event{pub serial:u64}pub struct Record{pub event:Event}
 pub struct Reader{pub calls:Mutex<u32>,pub disposed:Mutex<Vec<u64>>}
 impl Reader{
  pub fn read(&self,_:&mut[u8])->Result<std::vec::IntoIter<Result<Record,error::Error>>,error::Error>{let mut n=self.calls.lock().unwrap();*n+=1;if *n>1{return Err(error::Error::Os(errno::Errno::EAGAIN))}Ok(vec![Ok(Record{event:Event{serial:9}}),Ok(Record{event:Event{serial:10}})].into_iter())}
  pub fn dispose(&self,d:&Disposition)->Result<(),error::Error>{assert_eq!(d.disposition,1);self.disposed.lock().unwrap().push(d.serial);Ok(())}
 }
}
BODY
#[test]fn every_grabbed_edge_has_a_terminal_record(){let r=input::Reader{calls:Mutex::new(0),disposed:Mutex::new(Vec::new())};drain_session_input(&r).unwrap();assert_eq!(*r.disposed.lock().unwrap(),[9,10]);}
'''.replace('BODY', body))
        self.assertIn('drain_session_input(reader)', text[text.index('fn main()'):])
        self.assertIn('reader.fd()', text[text.index('fn main()'):])


if __name__ == '__main__':
    unittest.main()
