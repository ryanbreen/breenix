"""Run the production device backend with descriptor and owner observations faked."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class DevicesRuntime(unittest.TestCase):
    def test_terminal_source_does_not_wait_for_future_settle_ack(self):
        source = (ROOT / 'libs/libuitest/src/pointer_live.rs').read_text()
        function = source.split('pub fn wait_pointer_idle(', 1)[1].split('struct Mapping', 1)[0]
        harness = r'''
use std::time::{Duration,Instant};
pub struct Status {pub handled_serial:u64}
pub fn status()->Result<Status,String>{Ok(Status{handled_serial:0})}
fn outcome(_:u64)->Result<Option<(String,u64,String)>,String>{Ok(Some(("wm-handled".into(),0,"none".into())))}
fn pause()->Result<(),String>{Err("polled after terminal source outcome".into())}
fn main(){assert_eq!(wait_pointer_idle(17),Ok(()));}
''' + 'pub fn wait_pointer_idle(' + function
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'idle.rs').write_text(harness)
            result = subprocess.run(['rustc', '--edition=2021', '-Dwarnings', str(path / 'idle.rs'), '-o', str(path / 'idle')], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            result = subprocess.run([str(path / 'idle')], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_grab_gates_writes_and_releases_precede_descriptor_close(self):
        source = (ROOT / 'libs/libuitest/src/devices.rs').read_text()
        source = source.replace('#[cfg(target_os = "breenix")]', '')
        harness = r'''
extern crate self as libbreenix;
use std::sync::Mutex;
#[derive(Default)] struct State {
    owner: u32, composites: u64, next: u32, fail_grab: bool,
    caps: Vec<Vec<input::Capability>>, effects: Vec<String>,
}
static STATE: Mutex<State> = Mutex::new(State { owner:0, composites:0, next:0, fail_grab:false, caps:Vec::new(), effects:Vec::new() });
pub mod errno { #[derive(Debug)] pub enum Errno { EAGAIN, EINTR, ENOENT } }
pub mod error {
    #[derive(Debug)] pub enum Error { Os(super::errno::Errno) }
    impl std::fmt::Display for Error {fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {write!(f,"{self:?}")}}
}
pub mod ui_clock {pub fn now_ns()->u64 {0}}
pub mod window {
    pub mod abi {
        pub mod op {pub const COMPOSITOR_STATUS:u32=1;}
        #[derive(Default)] pub struct CompositorStatus {pub owner:u32,pub composites:u64}
    }
    pub unsafe fn raw_call(_:u32,ptr:*mut u8,_:usize)->i64 {
        let s=super::STATE.lock().unwrap();
        if s.owner==0 {return -(super::errno::Errno::ENOENT as i64);}
        *ptr.cast::<abi::CompositorStatus>()=abi::CompositorStatus{owner:s.owner,composites:s.composites};0
    }
}
pub mod input {
    use super::{STATE,error::Error,errno::Errno};
    pub mod key {pub const BUTTON_LEFT:u16=272;pub const BUTTON_RIGHT:u16=273;pub const BUTTON_MIDDLE:u16=274;}
    pub mod kind {pub const KEY:u16=1;pub const REL:u16=2;pub const TEXT:u16=6;}
    pub mod axis {pub const X:u16=0;pub const Y:u16=1;pub const WHEEL:u16=8;pub const HWHEEL:u16=6;}
    pub mod disposition {pub const WM_HANDLED:u32=1;}
    #[derive(Clone,Debug)] pub struct Capability {pub kind:u16,pub code:u16,pub minimum:i32,pub maximum:i32}
    #[derive(Default)] pub struct Disposition {pub serial:u64,pub disposition:u32,pub window:u64}
    pub struct InputEvent {pub serial:u64}
    pub struct Record {pub event:InputEvent}
    pub struct Batch<'a> {pub events:Vec<(u16,u16,i32)>, storage:&'a mut [u8],payload:usize}
    impl<'a> Batch<'a> {
        pub fn new(storage:&'a mut [u8])->Self{Self{events:Vec::new(),storage,payload:0}}
        pub fn event(&mut self,k:u16,c:u16,v:i32,_:u64)->Result<&mut Self,Error>{self.events.push((k,c,v));Ok(self)}
        pub fn text(&mut self,text:&str,_:u64)->Result<&mut Self,Error>{self.payload+=text.len();self.events.push((6,0,text.len() as i32));Ok(self)}
        pub fn finish(&mut self,_:u64)->Result<&mut Self,Error>{assert_eq!(self.storage.len(),(self.events.len()+1)*32+self.payload);Ok(self)}
    }
    pub struct Injector(u32);
    impl Injector {
        pub fn create(caps:&[Capability],clock:bool)->Result<Self,Error>{
            assert!(clock);let mut s=STATE.lock().unwrap();s.next+=1;
            s.caps.push(caps.to_vec());Ok(Self(s.next))
        }
        pub fn id(&self)->u32{self.0}
        pub fn inject(&self,b:&Batch<'_>)->Result<(),Error>{
            let mut s=STATE.lock().unwrap();
            for (k,c,v) in &b.events {s.effects.push(format!("write {} {k} {c} {v}",self.0));}Ok(())
        }
    }
    impl Drop for Injector {fn drop(&mut self){STATE.lock().unwrap().effects.push(format!("close {}",self.0));}}
    pub struct Reader;
    impl Reader {
        pub fn open(nonblocking:bool)->Result<Self,Error>{assert!(nonblocking);Ok(Self)}
        pub fn grab(&self,ids:&[u32])->Result<(),Error>{
            let mut s=STATE.lock().unwrap();s.effects.push(format!("grab {ids:?}"));
            if s.fail_grab {Err(Error::Os(Errno::EAGAIN))}else{Ok(())}
        }
        pub fn read(&self,_:&mut [u8])->Result<std::vec::IntoIter<Result<Record,Error>>,Error>{Err(Error::Os(Errno::EAGAIN))}
        pub fn dispose(&self,_:&Disposition)->Result<(),Error>{Ok(())}
    }
    impl Drop for Reader {fn drop(&mut self){STATE.lock().unwrap().effects.push("ungrab".into());}}
}
mod devices { SOURCE }
fn owner(owner:u32,composites:u64){let mut s=STATE.lock().unwrap();s.owner=owner;s.composites=composites;}
#[test] fn production_backend() {
    use devices::{runtime::Devices,RawInput};
    let mut pair=Devices::create().unwrap();
    assert!(!pair.text("λ🙂").unwrap());
    assert_eq!(pair.ids(),[1,2]);
    assert_eq!(pair.environment(),b"BREENIX_INPUT_DEVICES=1,2");
    {
        let s=STATE.lock().unwrap();assert_eq!(s.caps.len(),2);
        assert_eq!(s.caps[0].iter().map(|c|(c.kind,c.code)).collect::<Vec<_>>(),(1..=255).map(|c|(1,c)).chain([(6,0)]).collect::<Vec<_>>());
        assert_eq!(s.caps[1].iter().map(|c|(c.kind,c.code)).collect::<Vec<_>>(),[(1,272),(1,273),(1,274),(2,0),(2,1),(2,8),(2,6)]);
    }
    let key=RawInput{kind:1,code:42,value:1};
    owner(7,0);pair.observe(Some(7),false).unwrap();assert!(!pair.inject(key).unwrap());assert!(!pair.motion(7,-9).unwrap());
    owner(7,1);pair.observe(Some(7),false).unwrap();assert!(pair.inject(key).unwrap());assert!(pair.motion(7,-9).unwrap());assert!(pair.text("λ🙂").unwrap());
    owner(0,0);assert!(!pair.inject(key).unwrap());
    owner(8,0);pair.observe(Some(7),true).unwrap();assert!(!pair.inject(key).unwrap());
    STATE.lock().unwrap().fail_grab=true;assert!(pair.recover(8).is_err());assert!(!pair.inject(key).unwrap());
    STATE.lock().unwrap().fail_grab=false;pair.recover(8).unwrap();
    assert!(pair.inject(RawInput{kind:1,code:272,value:1}).unwrap());
    assert!(pair.inject(RawInput{kind:2,code:8,value:-2}).unwrap());
    pair.destroy().unwrap();
    let s=STATE.lock().unwrap();
    assert_eq!(s.effects, ["write 1 1 42 1","write 2 2 0 7","write 2 2 1 -9","write 1 6 0 6","grab [1, 2]","ungrab","grab [1, 2]",
        "write 2 1 272 1","write 2 2 8 -2","write 1 1 42 0","write 2 1 272 0","close 1","close 2","ungrab"]);
}
'''.replace('SOURCE', source)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'check.rs').write_text(harness)
            command = ['rustc', '--edition=2021', '--test', '-Dwarnings', str(path / 'check.rs'), '-o', str(path / 'check')]
            if sys.platform == 'darwin':
                command = ['taskpolicy', '-b', *command]
            result = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            result = subprocess.run([str(path / 'check')], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == '__main__':
    unittest.main()
