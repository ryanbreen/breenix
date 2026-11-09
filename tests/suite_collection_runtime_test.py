"""Run the production collector loop against deterministic I/O and clock events."""
from pathlib import Path
import os
import subprocess
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class CollectionRuntime(unittest.TestCase):
    def test_absolute_deadline_partial_io_exit_race_and_sigpipe(self):
        rel = 'libs/libbreenix/src/suite_session.rs'
        baseline = os.environ.get('UITEST_OLD_COMMIT')
        source = subprocess.check_output(['git', 'show', f'{baseline}:{rel}'], cwd=ROOT, text=True) if baseline else (ROOT / rel).read_text()
        body = item(source, 'pub fn collect_until(')
        send = item(source, 'fn send_collector(')
        harness = r'''
extern crate self as libbreenix;
use std::{collections::VecDeque,sync::Mutex};
#[path="RESULT"]pub mod result;
#[path="COLLECTION"]pub mod collection;
use collection::{Handshake,Request,State,Trigger};
#[derive(Clone,Copy)]pub struct Fd(u64);impl Fd{pub fn raw(self)->u64{self.0}}
fn fd(f:&Fd)->Fd{*f}
#[derive(Clone,Copy)]pub struct Pid(u64);impl Pid{pub fn raw(self)->u64{self.0}}
#[derive(Debug)]pub enum Errno{EINTR,EAGAIN,ECHILD,EPIPE}
#[derive(Debug)]pub enum Error{Os(Errno)}
impl std::fmt::Display for Error{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{write!(f,"{:?}",self)}}
impl Error{pub fn from_syscall(n:i64)->Result<u64,Self>{match n{-4=>Err(Self::Os(Errno::EINTR)),-11=>Err(Self::Os(Errno::EAGAIN)),-32=>Err(Self::Os(Errno::EPIPE)),n if n>=0=>Ok(n as u64),_=>panic!("unexpected errno")}}}
struct Sim{now:i128,die:Option<i128>,epipe:bool,blocked:bool,writes:Vec<u8>,reply:VecDeque<u8>,ready:i128}
static SIM:Mutex<Sim>=Mutex::new(Sim{now:0,die:None,epipe:false,blocked:false,writes:Vec::new(),reply:VecDeque::new(),ready:0});
pub mod syscall{
    pub mod nr{pub const SENDTO:u64=44;}
    pub mod raw{pub unsafe fn syscall6(n:u64,_:u64,p:u64,len:u64,flags:u64,a:u64,b:u64)->u64{
        assert_eq!((n,flags,a,b),(super::nr::SENDTO,0x4000,0,0));
        let mut s=super::super::SIM.lock().unwrap();
        if s.epipe{return (-32i64)as u64;}if s.blocked{return (-11i64)as u64;}
        let n=(len as usize).min(2);s.writes.extend_from_slice(std::slice::from_raw_parts(p as *const u8,n));n as u64
    }}
}
pub mod process{
    use super::*;pub const WNOHANG:i32=1;
    pub fn waitpid(_:i32,_:&mut i32,flags:i32)->Result<Pid,Error>{assert_eq!(flags,WNOHANG);let s=SIM.lock().unwrap();Ok(Pid(if s.die.is_some_and(|t|t<=s.now){7}else{0}))}
}
pub mod io{
    use super::*;
    pub fn read(_:Fd,bytes:&mut[u8])->Result<usize,Error>{
        let mut s=SIM.lock().unwrap();if s.now>=s.ready && !s.reply.is_empty(){let n=bytes.len().min(3).min(s.reply.len());for b in &mut bytes[..n]{*b=s.reply.pop_front().unwrap();}return Ok(n);}
        if s.die.is_some_and(|t|t<=s.now) && s.reply.is_empty(){Ok(0)}else{Err(Error::Os(Errno::EAGAIN))}
    }
}
pub mod time{
    use super::*;pub struct Stamp(i128);impl Stamp{pub fn as_nanos(self)->i128{self.0}}
    pub fn now_monotonic()->Result<Stamp,Error>{Ok(Stamp(SIM.lock().unwrap().now))}
    pub fn sleep_ms(ms:u64)->Result<(),Error>{SIM.lock().unwrap().now+=i128::from(ms)*1_000_000;Ok(())}
}
enum Wait{CollectorReply}
struct Watchdog(i128);impl Watchdog{fn new(_:Wait)->Result<Self,String>{Ok(Self(time::now_monotonic().unwrap().as_nanos()+10_000_000_000))}fn deadline_ns(&self)->u64{self.0 as u64}}
COMMAND_IMPL
struct RunnerSession{pid:Option<Pid>,collector:Option<Fd>,control:bool}
impl RunnerSession{fn in_parent(&mut self){self.control=false;}
METHOD_IMPL
}
#[test]fn production_loop_obeys_deadlines_and_drains_exit(){
    let record="SUITE test COLLECT cat/case trigger=fail screen=none:unavailable windows=0 remnants=0 artifacts=none:unavailable";
    for scenario in ["live-done","exit-done","silent","dead","write-blocked","epipe","discard"]{
        let collecting=scenario!="discard";
        let reply=if scenario.ends_with("done"){format!("{record}\ndone\n")}else if !collecting{"done\n".into()}else{String::new()};
        *SIM.lock().unwrap()=Sim{now:1_500_000_000,die:match scenario{"exit-done"=>Some(1_601_000_000),"dead"=>Some(1_600_000_000),_=>None},
            epipe:scenario=="epipe",blocked:scenario=="write-blocked",writes:Vec::new(),reply:reply.bytes().collect(),ready:1_600_000_000};
        let mut runner=RunnerSession{pid:Some(Pid(7)),collector:Some(Fd(4)),control:true};let mut records=Vec::new();
        let request=if collecting{Request::Collect(Trigger::Fail)}else{Request::Discard};
        let result=runner.collect_until(Some(3_000_000_000),request,"test","cat/case",|r|{records.push(r.to_owned());Ok(())});
        let s=SIM.lock().unwrap();assert!(!runner.control);
        match scenario{
            "live-done"|"exit-done"|"discard"=>{assert!(result.is_ok(),"{scenario}: {result:?}");assert!(s.now<3_000_000_000);assert_eq!(s.writes,request.command().as_bytes());assert_eq!(records,if collecting{vec![record]}else{vec![]});},
            "silent"|"write-blocked"=>{assert_eq!(result.unwrap_err(),"collector-timeout");assert_eq!(s.now,3_000_000_000);},
            "dead"|"epipe"=>{assert_eq!(result.unwrap_err(),"session supervisor died");assert!(s.now<3_000_000_000);},
            _=>unreachable!()
        }
    }
}
'''.replace('RESULT', str(ROOT / 'libs/libbreenix/src/suite_result.rs')).replace('COLLECTION', str(ROOT / 'libs/libbreenix/src/suite_collection.rs')).replace('METHOD_IMPL', body).replace('COMMAND_IMPL', send)
        run_rust(self, harness)


if __name__ == '__main__':
    unittest.main()
