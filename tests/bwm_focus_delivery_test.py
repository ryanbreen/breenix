"""Exercise focus setup when host transport outlives the client delivery wait."""
from pathlib import Path
import subprocess
import tempfile
import unittest
from gt710_gr_live_unwind_test import item
ROOT = Path(__file__).resolve().parents[1]

class FocusDelivery(unittest.TestCase):
    def test_focus_waits_for_transport_before_timing_client_delivery(self):
        source = item((ROOT / 'userspace/programs/src/wm_suite/desktop.rs').read_text(), 'pub fn focus(')
        harness = r'''
use std::cell::{Cell, RefCell};
thread_local! {
    static NOW: Cell<u64> = const { Cell::new(0) };
    static PENDING: Cell<bool> = const { Cell::new(false) };
    static EVENTS: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
    static LIMITS: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}
mod abi {pub mod event_kind {pub const POINTER_BUTTON:u32=7;}}
#[derive(Clone,Copy,Debug)]pub struct Event {kind:u32,pressed:u32,button:u32,serial:u64}
pub struct Info {x:i32,y:i32}
pub struct Client;
impl Client {
    pub fn info(&self)->Result<Info,String>{Ok(Info{x:32,y:72})}
    pub fn clear(&self)->Result<(),String>{EVENTS.with(|e|e.borrow_mut().clear());Ok(())}
    pub fn await_event(&self,_:&str,p:impl Fn(&Event)->bool)->Result<Event,String>{
        EVENTS.with(|e|{let mut e=e.borrow_mut();if let Some(i)=e.iter().position(p){Ok(e.remove(i))}
        else{NOW.with(|n|n.set(n.get()+1_004_251_000));Err("client wait expired while focus press remained in host transport".into())}})
    }
}
pub struct Desktop {pub width:u32,pub height:u32}
impl Desktop {
    pub fn click(&self,_:i32,_:i32)->Result<(),String>{
        PENDING.with(|p|p.set(true));NOW.with(|n|n.set(n.get()+200_000_000));Ok(())
    }
    SOURCE
}
fn now()->u64{NOW.with(Cell::get)}
fn until(_:u64,_:&str,mut test:impl FnMut()->Result<bool,String>)->Result<(),String>{if test()?{Ok(())}else{Err("geometry absent".into())}}
fn elapsed(_:&str,_:u64,limit:u64){LIMITS.with(|v|v.borrow_mut().push(limit));}
pub mod breenix_suite {pub mod suite {
    pub fn hostpointer_acked(x:u32,y:u32,width:u32,height:u32,buttons:u32,wheel:i32)->Result<u64,String>{
        assert_eq!((x,y,width,height,wheel),(52,92,1280,1024,0));
        // Force transport work past the client's one-second deadline on each pair.
        super::super::NOW.with(|n|n.set(n.get()+700_000_000));
        let serial=if buttons==1{1}else{2};
        super::super::EVENTS.with(|e|e.borrow_mut().push(super::super::Event{kind:7,pressed:buttons,button:1,serial}));
        super::super::PENDING.with(|p|p.set(false));Ok(serial)
    }
}}
#[test]fn delayed_transport_is_setup_and_leaves_no_press_for_the_next_case(){
    let result=Desktop{width:1280,height:1024}.focus(&mut Client);
    assert!(result.is_ok(),"{result:?}");
    assert!(now()>=1_400_000_000,"the transport delay was not forced");
    assert!(!PENDING.with(Cell::get),"a stale press can raise the next case's lower client");
    assert!(EVENTS.with(|e|e.borrow().is_empty()));
    assert_eq!(LIMITS.with(|v|v.borrow().clone()),vec![1_000_000_000;2]);
}
'''.replace('SOURCE', source)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'test.rs').write_text(harness)
            for command in [
                ['taskpolicy', '-b', 'rustc', '--edition=2021', '--test', '-Dwarnings', str(path / 'test.rs'), '-o', str(path / 'test')],
                [str(path / 'test')],
            ]:
                result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

if __name__ == '__main__': unittest.main()
