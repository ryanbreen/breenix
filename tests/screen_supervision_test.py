"""Exercise the production parent wait with a frozen checkpoint and a dead writer."""
from pathlib import Path
import subprocess
import sys
import tempfile
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from screen_evidence_test import function
s=(ROOT/'libs/libbreenix/src/suite.rs').read_text()
c=(ROOT/'libs/libbreenix/src/suite_screen.rs').read_text()
fixture=r'''
#![allow(dead_code)]
use std::sync::atomic::{AtomicU64,Ordering};
static NOW:AtomicU64=AtomicU64::new(1);
fn monotonic_ns()->Option<i128>{Some(NOW.fetch_add(1_000_000,Ordering::SeqCst) as i128)}
fn ms_since(start:Option<i128>)->u64{((monotonic_ns().unwrap()-start.unwrap())/1_000_000) as u64}
mod screen {
 use super::*;
 CLOCK_STRUCT
 CLOCK_IMPL
 pub static CLOCK:CheckpointClock=CheckpointClock::new();
 pub fn excluded_ns(now:u64)->u64{CLOCK.excluded_at(now)}
 pub fn window_excluded()->u64{0}
 pub fn overdue(now:u64)->bool{CLOCK.overdue(now)}
}
#[derive(Clone,Copy)]struct Fd(u64);
impl Fd{fn from_raw(v:u64)->Self{Self(v)}fn raw(self)->u64{self.0}}
#[derive(Clone,Copy)]struct PollFd{fd:i32,revents:i32}
impl PollFd{fn new(fd:Fd,_:i32)->Self{Self{fd:fd.raw() as i32,revents:0}}}
mod poll_events{pub const POLLIN:i32=1;}
#[derive(Debug)]struct Error;
const WNOHANG:i32=1;
mod process{use super::*;pub fn waitpid(_:i32,_:&mut i32,_:i32)->Result<Fd,Error>{Ok(Fd(0))}}
mod io{use super::*;pub fn read(_:Fd,_:&mut [u8])->Result<usize,Error>{Ok(0)}pub fn poll(_:&mut [PollFd],_:i32)->Result<i32,Error>{Ok(0)}}
const CLOCK_STALL_WAKES:u32=1000;
const FRAME_MS:i128=66;
enum Waited{Exited(i32),TimedOut(u64),ClockStopped(u64),Failed(Error),Published([u8;24])}
fn read_message(_:Fd)->Option<[u8;1]>{None}
fn write_message(_:Fd,_:&[u8;1])->bool{true}
mod crate_stage{pub fn serve(_:&[u8;1])->[u8;1]{[0]}}
struct ResultPipe<'a>(std::marker::PhantomData<&'a ()>);
impl ResultPipe<'_>{fn drain(&mut self){} fn poll_fd(&self)->Option<Fd>{None}}
WAIT
#[test] fn frozen_observation_and_dead_child_are_bounded(){
 screen::CLOCK.start(1);
 let waited=wait_exit(7,Some(1),10,&mut ||{},None,None,Some(Fd(1)),None,None);
 assert!(matches!(waited,Waited::TimedOut(_)));
 assert!(NOW.load(Ordering::SeqCst)>=120_000_000_001);
 let start=monotonic_ns();
 let waited=wait_exit(7,start,1000,&mut ||{},None,None,None,None,None);
 assert!(matches!(waited,Waited::TimedOut(ms) if ms>=1000));
 assert!(NOW.load(Ordering::SeqCst)<122_000_000_000);
}
'''.replace('CLOCK_STRUCT',function(c,'pub(super) struct CheckpointClock')).replace('CLOCK_IMPL',function(c,'impl CheckpointClock')).replace('WAIT',function(s,'fn wait_exit(').replace('crate::suite_stage','crate_stage'))
with tempfile.TemporaryDirectory() as directory:
    path=Path(directory)
    (path/'test.rs').write_text(fixture)
    priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
    subprocess.run(priority+['rustc','--edition=2021','--test','-Dwarnings',str(path/'test.rs'),'-o',str(path/'test')],check=True)
    subprocess.run([str(path/'test')],check=True,timeout=10)
