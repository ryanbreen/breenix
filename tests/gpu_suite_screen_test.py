"""Exercise the runner view against a display that retains its shown target."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class RunnerView(unittest.TestCase):
    def test_redraw_uses_a_target_other_than_the_displayed_frame(self):
        source = (ROOT / 'libs/libbreenix/src/suite_gpu_screen.rs').read_text()
        source = source[:source.index('/// Finish the display handoff')]
        harness = r'''
#![allow(dead_code)] // The runner's outcome variants are inputs to the extracted view.
use std::cell::RefCell;
enum Outcome { Pass { ms: u64 }, Fail { msg: String }, Skip { msg: String } }
enum State { Pending, Running, Done(Outcome) }
fn monotonic_ns() -> Option<i128> { Some(0) }
#[derive(Default)] struct Device { targets: Vec<u64>, bound: u64, frames: Vec<u64> }
thread_local! { static DEVICE: RefCell<Device> = RefCell::new(Device::default()); }
mod graphics { pub fn take_over_display()->Result<(),()> { Ok(()) } }
mod time { pub fn sleep_ms(_:u64)->Result<(),()> { Ok(()) } }
mod suite_gpu_backend {
    pub struct Values {pub success:u32}
    pub fn backend_values(_:u32)->Option<Values>{Some(Values{success:0x60000000})}
}
mod gpu {
    use super::DEVICE;
    pub mod abi {
        pub mod context_state {pub const LIVE:u32=1;pub const WARMING:u32=3;}
        pub mod format {pub const B8G8R8A8:u32=1;}
        pub mod pipeline {pub const SOLID:u32=0;}
        pub mod stream {pub mod encode {
            pub fn begin(s:&mut Vec<u32>,t:u64){s.push(t as u32);}
            pub fn clear(_: &mut Vec<u32>,_:u32){}
            pub fn draw(_: &mut Vec<u32>,_:u32,_:u32,_:&[[i32;4]]){}
        }}
    }
    pub struct Counters {pub state:u32}
    pub struct Submit {pub fence:u64}
    pub struct Wait {pub status:u32}
    pub struct Present {pub answer:u32,pub fence:u64}
    pub struct Caps {pub backend:u32}
    pub fn tc_disarm_all()->i64{0}
    pub fn counters(_:u64)->(i64,Counters){(0,Counters{state:1})}
    pub fn caps()->(i64,Caps){(0,Caps{backend:6})}
    pub fn context_create()->Result<u64,i64>{Ok(1)}
    pub fn context_destroy(_:u64)->i64{0}
    pub fn resource_destroy(_:u64)->i64{0}
    pub fn scanout_target_create(_:u32,_:u32,_:u32)->Result<u64,i64>{
        DEVICE.with(|d|{let mut d=d.borrow_mut();let id=d.targets.len() as u64+10;d.targets.push(id);Ok(id)})
    }
    pub fn submit(_:u64,s:&[u32])->(i64,Submit){
        DEVICE.with(|d|{let d=d.borrow();(if u64::from(s[0])==d.bound {-16}else{0},Submit{fence:1})})
    }
    pub fn wait(_:u64,_:u64,_:u64)->(i64,Wait){(0,Wait{status:0})}
    pub fn present(_:u64,t:u64)->(i64,Present){
        DEVICE.with(|d|{let mut d=d.borrow_mut();d.bound=t;d.frames.push(t);});
        (0,Present{answer:0x60000000,fence:1})
    }
}
mod view { SOURCE }
#[test] fn runner_redraws_without_writing_the_shown_surface(){
    let mut view=view::GpuScreen::new();
    view.open().unwrap();
    let held=DEVICE.with(|d|d.borrow().targets.len());
    let states=vec![vec![State::Running]];
    for _ in 0..4 {view.draw(&states).expect("runner tried to overwrite its displayed target");}
    DEVICE.with(|d|{let d=d.borrow();
        assert_eq!(held,2,"both targets must precede the memory baseline");
        assert_eq!(d.targets.len(),2,"redrawing must reuse its targets");
        assert_eq!(d.frames,[10,11,10,11]);
    });
}
'''.replace('SOURCE', source)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'view.rs').write_text(harness)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                                      str(path / 'view.rs'), '-o', str(path / 'view')], check=True)
            subprocess.run([str(path / 'view'), '--nocapture'], check=True)


if __name__ == '__main__':
    unittest.main()
