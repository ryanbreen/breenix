"""Pending scanout polls retain one full-mode pixel buffer until observation succeeds."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]

class ReadbackStorage(unittest.TestCase):
    def test_pending_polls_reuse_the_full_mode_allocation(self):
        source = (ROOT / 'userspace/programs/src/wm_suite/desktop.rs').read_text()
        screen = item(source, 'impl Screen {')
        methods = item(screen, 'pub fn read(')
        if 'fn read_into(' in screen:
            methods += item(screen, 'fn read_into(')
        observation = item(source, 'struct WaitObservation {')
        observer_read = item(item(source, 'impl WaitObservation {'), 'fn read(')
        run_rust(self, r'''
use std::sync::atomic::{AtomicUsize, Ordering};
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static CALLS: AtomicUsize = AtomicUsize::new(0);
macro_rules! vec { ($value:expr; $len:expr) => {{
    ALLOCATIONS.fetch_add(1, Ordering::Relaxed); std::vec![$value; $len]
}} }
mod gpu {
    pub struct Mode {pub width:u32,pub height:u32}
    pub fn current_mode()->Result<Mode,String>{Ok(Mode{width:8,height:8})}
}
mod abi {
    pub struct Readback {pub dst:u64,pub dst_len:u64,pub record:u64,pub record_len:u32,pub reason:u32}
    impl Readback {pub const ZERO:Self=Self{dst:0,dst_len:0,record:0,record_len:0,reason:0};}
}
mod op {pub const SCANOUT_READBACK:u32=22;}
mod window {
    pub enum ObservationKind {Composed}
    pub fn validate_scanout_observation(_: &[u8],w:u32,h:u32,_:ObservationKind)->bool{(w,h)==(8,8)}
    pub fn call(op:u32,r:&mut super::abi::Readback)->Result<(),String>{
        assert_eq!(op,22);assert_ne!(r.dst,0);assert_eq!(r.dst_len,256);
        assert_ne!(r.record,0);assert_eq!(r.record_len,1024);
        if super::CALLS.fetch_add(1,super::Ordering::Relaxed)<4 {Err("EAGAIN".into())}else{Ok(())}
    }
}
fn quad(_: &[u8],_:usize)->u64{1}fn word(_: &[u8],_:usize)->u32{0}
struct Screen {pixels:Vec<u8>,record:[u8;1024],width:u32,height:u32}
impl Screen {
    @METHODS@
    fn serial(&self)->u64{quad(&self.record,176)}
    fn pixel(&self,_:i32,_:i32)->Result<u32,String>{Ok(0)}
}
#[derive(Default)] @OBSERVATION@
impl WaitObservation { @OBSERVER_READ@ }
#[test]fn one_buffer_across_pending_readbacks(){
    // Keep the one-shot entry point compiled too; waits use the retaining reader.
    let _ = Screen::read;
    let mut observer=WaitObservation::default();
    for _ in 0..4 {assert!(observer.read().is_err());}
    let screen=observer.read().expect("the completed readback still validates");
    assert_eq!(screen.pixels.len(),256);
    assert_eq!(ALLOCATIONS.load(Ordering::Relaxed),1,
        "pending polls repeatedly allocate and zero an entire display");
    assert!(observer.record.is_some());assert_eq!(observer.pixel,Some(0));
    assert!(observer.error.is_some());
}
'''.replace('@METHODS@', methods).replace('@OBSERVATION@', observation)
   .replace('@OBSERVER_READ@', observer_read))

if __name__ == '__main__': unittest.main()
