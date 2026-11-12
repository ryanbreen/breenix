"""GPU admission alone must not release init's first desktop client."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT = Path(__file__).resolve().parents[1]

class DesktopReady(unittest.TestCase):
    def test_waits_for_the_compositors_first_scene(self):
        function = item((ROOT/'userspace/programs/src/init.rs').read_text(), 'fn desktop_gpu_ready(')
        run_rust(self, r'''
use std::sync::atomic::{AtomicBool,AtomicUsize,Ordering};
static PRESENTED:AtomicBool=AtomicBool::new(false);
static WAITS:AtomicUsize=AtomicUsize::new(0);
pub mod libbreenix {
 pub mod gpu {
  pub mod abi {pub mod status {pub const ADMITTED:u32=1;}}
  pub struct Caps {pub status:u32}
  pub fn caps()->(i64,Caps){(0,Caps{status:abi::status::ADMITTED})}
 }
 pub mod window {
  pub mod abi {
   #[derive(Default)]pub struct CompositorStatus {pub presented:u64}
   pub mod op {pub const COMPOSITOR_STATUS:u32=1;}
  }
  pub fn call(_:u32,s:&mut abi::CompositorStatus)->Result<u64,()> {
   s.presented=u64::from(crate::PRESENTED.load(crate::Ordering::Relaxed));Ok(0)
  }
 }
 pub mod time {pub fn sleep_ms(_:u64)->Result<(),()> {
  crate::WAITS.fetch_add(1,crate::Ordering::Relaxed);
  crate::PRESENTED.store(true,crate::Ordering::Relaxed);Ok(())
 }}
}
@FUNCTION@
#[test]fn first_scene_before_client(){
 assert!(desktop_gpu_ready());
 assert_eq!(WAITS.load(Ordering::Relaxed),1,"admission released the client before the first composed scene");
}
'''.replace('@FUNCTION@',function))
if __name__=='__main__':unittest.main()
