"""Exercise reset requalification with the production native CAPS policy."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests"))
from gt710_gr_live_unwind_test import item

class RestartAdmission(unittest.TestCase):
    def test_new_epoch_requires_fresh_executable_probes(self):
        method = item((ROOT / "kernel/src/gpu/gk208/adapter.rs").read_text(),
                      "    pub(crate) fn reprobe(")
        source = r'''
extern crate alloc;
#[path="CAPS"] pub mod policy;
pub mod gk208 {
 pub mod caps {
  pub use crate::policy::*;
  pub fn run_reprobe(d:&mut super::adapter::Device,epoch:u64)->Result<(),&'static str> {
   assert_eq!(epoch,18);
   assert_eq!(d.board.channel,(0x123000,18));
   assert_eq!(Profile::from_board(&d.board,d.channel.identity()).admission(None),
     Err((gpu_abi::status::MISSING_CAPABILITY,gpu_abi::capability::BGRA_RENDER)),
     "old executable results must be cleared before requalification");
   if d.fail_probe {return Err("fresh hardware probe failed");}
   for id in gpu_abi::capability::BGRA_RENDER..=gpu_abi::capability::REGION_COPY {
    let bytes=probe_expected(id).unwrap();d.board.probes.record(id,true,bytes,bytes);
   }
   Ok(())
  }
 }
 pub mod adapter {
  pub struct Channel;
  impl Channel {pub fn identity(&self)->(u64,u64){(0x123000,18)}}
  pub struct Device {pub board:super::caps::BoardState,pub channel:Channel,pub fail_probe:bool}
  impl Device {
   fn admission(&self,masked:Option<u32>)->Result<(),(u32,u32)> {
    super::caps::Profile::from_board(&self.board,self.channel.identity()).admission(masked)
   }
   // METHOD
  }
 }
}
fn main(){
 use gk208::{caps::{BoardState,Profile,probe_expected},adapter::{Device,Channel}};
 for fail_probe in [false,true] {
  let mut board=BoardState::empty();board.chipset=0x106;board.channel=(0x123000,17);
  board.graphics_ready=true;board.created_classes=gpu_abi::gk208::CAPSET_IDS;
  for id in gpu_abi::capability::BGRA_RENDER..=gpu_abi::capability::REGION_COPY {
   let b=probe_expected(id).unwrap();board.probes.record(id,true,b,b);
  }
  assert!(Profile::from_board(&board,board.channel).admission(None).is_ok());
  let mut d=Device {board,channel:Channel,fail_probe};
  assert_eq!(d.reprobe().is_err(),fail_probe);
  assert_eq!(Profile::from_board(&d.board,d.channel.identity()).admission(None).is_ok(),!fail_probe);
 }
}
'''.replace('"CAPS"', '"' + str(ROOT / "kernel/src/gpu/gk208/caps.rs") + '"').replace("// METHOD", method)
        priority = ["taskpolicy", "-b"] if sys.platform == "darwin" else []
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / "reprobe.rs").write_text(source)
            subprocess.run(priority + ["rustc", "--edition=2021", "--crate-name", "gpu_abi", "--crate-type", "rlib", str(ROOT / "libs/gpu-abi/src/lib.rs"), "-o", str(path / "libgpu_abi.rlib")], check=True)
            subprocess.run(priority + ["rustc", "--edition=2021", "-Dwarnings", "--extern", f"gpu_abi={path / 'libgpu_abi.rlib'}", str(path / "reprobe.rs"), "-o", str(path / "reprobe")], check=True)
            subprocess.run([str(path / "reprobe")], check=True)

if __name__ == "__main__": unittest.main()
