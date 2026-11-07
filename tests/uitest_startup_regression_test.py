"""Force closed-before-startup control and invisible registered kiosk windows."""
from pathlib import Path
import os
import subprocess
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT = Path(__file__).resolve().parents[1]
def source(rel):
    old = os.environ.get('UITEST_OLD_COMMIT')
    return subprocess.check_output(['git', 'show', f'{old}:{rel}'], cwd=ROOT, text=True) if old else (ROOT/rel).read_text()
class Startup(unittest.TestCase):
    def test_dead_case_does_not_abort_supervisor_startup(self):
        text = source('libs/libuitest/src/supervisor_runtime.rs')
        start = text.index('        devices = Some(Devices::create()?);')
        body = text[start:text.index('        let mut ended', start)]
        run_rust(self, r'''
#[derive(Debug)]enum Message{Devices([u32;2]),Ready(u64)}
struct Devices;impl Devices{fn create()->Result<Self,String>{Ok(Self)}fn ids(&self)->[u32;2]{[11,12]}}
struct Pid;impl Pid{fn raw(self)->u64{23}}
mod process{pub fn getpid()->Result<super::Pid,String>{Ok(super::Pid)}}
struct Channel{queued:Vec<Message>,closed:bool}
impl Channel{
 fn queue(&mut self,m:&Message)->Result<(),String>{self.queued.push(match m{Message::Devices(ids)=>Message::Devices(*ids),Message::Ready(pid)=>Message::Ready(*pid)});Ok(())}
 fn send(&mut self,m:&Message)->Result<(),String>{self.queue(m)?;if self.closed{Err("EPIPE".into())}else{Ok(())}}
}
fn startup(channel:&mut Channel)->Result<(),String>{let devices:Option<Devices>;
BODY
assert!(devices.is_some());Ok(())}
#[test]fn fast_logic_exit(){
 for closed in [false,true]{let mut channel=Channel{queued:Vec::new(),closed};
 assert!(startup(&mut channel).is_ok(),"case exit prevented independent collection");
 assert!(matches!(&channel.queued[..],[Message::Devices([11,12]),Message::Ready(23)]));}
 // Exercise the transport failure explicitly too.
 assert!(Channel{queued:Vec::new(),closed:true}.send(&Message::Ready(23)).is_err());
}
'''.replace('BODY', body))
    def test_session_kiosk_places_before_checking_visible_damage(self):
        text = source('userspace/programs/src/kiosk_test.rs')
        body = item(text, 'fn compose_damage(')
        place = item(text, 'fn place_session_windows(') if 'fn place_session_windows(' in text else ''
        run_rust(self, r'''
use std::sync::{Arc,Mutex};
static INFO:Mutex<abi::WinInfo>=Mutex::new(abi::WinInfo{id:17,flags:1,commit_id:9});
mod abi{
 pub mod win_flags{pub const REGISTERED:u32=1;pub const VISIBLE:u32=2;pub const MINIMIZED:u32=4;}
 pub mod geometry_flags{pub const VISIBLE:u32=1;}
 #[derive(Default,Clone,Copy)]pub struct WinInfo{pub id:u64,pub flags:u32,pub commit_id:u64}
}
#[derive(Debug)]struct Error;
mod libbreenix{pub mod error{pub type Error=super::super::Error;}}
mod window{
 use super::*;
 pub fn list(infos:&mut[abi::WinInfo])->Result<usize,Error>{infos[0]=*INFO.lock().unwrap();Ok(1)}
 pub fn set_geometry(id:u64,x:i32,y:i32,z:u32,flags:u32)->Result<u64,Error>{assert_eq!((id,x,y,z,flags),(17,0,0,0,1));INFO.lock().unwrap().flags|=abi::win_flags::VISIBLE;Ok(0)}
}
struct Frame{serial:u64}struct Compositor;
impl Compositor{fn pending(&self)->Result<bool,i64>{Ok(true)}fn compose(&mut self)->Result<Frame,i64>{assert_ne!(INFO.lock().unwrap().flags&2,0);Ok(Frame{serial:5})}}
struct Kiosk{session:bool,compositor:Option<Arc<Mutex<Compositor>>>,last:u64}
impl Kiosk{fn compositor(&mut self)->Result<Arc<Mutex<Compositor>>,i64>{let c=self.compositor.get_or_insert_with(||Arc::new(Mutex::new(Compositor)));Ok(c.clone())}}
fn custody(){}
PLACE
BODY
#[test]fn registered_window_becomes_composited(){
 let mut kiosk=Kiosk{session:true,compositor:None,last:0};assert!(kiosk.session);compose_damage(&mut kiosk);assert_eq!(kiosk.last,5);
 // Existing non-session kiosk protocol leaves geometry under its caller's control.
 *INFO.lock().unwrap()=abi::WinInfo{id:17,flags:1,commit_id:9};
 let mut kiosk=Kiosk{session:false,compositor:None,last:0};compose_damage(&mut kiosk);assert_eq!(kiosk.last,0);
 // Visible or minimized geometry must survive subsequent session passes.
 for flags in [3,5]{INFO.lock().unwrap().flags=flags;
 if !PLACE_ABSENT {PLACE_CALL}
 assert_eq!(INFO.lock().unwrap().flags,flags);}
 assert_eq!(INFO.lock().unwrap().id,17);
 let _: Option<libbreenix::error::Error> = None;
 let _ = window::set_geometry as fn(u64,i32,i32,u32,u32)->Result<u64,Error>;
 let _ = (abi::win_flags::REGISTERED,abi::win_flags::MINIMIZED,abi::geometry_flags::VISIBLE);
}
'''.replace('PLACE_ABSENT', str(not bool(place)).lower()).replace('PLACE_CALL', 'place_session_windows().unwrap();' if place else '').replace('PLACE', place).replace('BODY', body))
if __name__ == '__main__': unittest.main()
