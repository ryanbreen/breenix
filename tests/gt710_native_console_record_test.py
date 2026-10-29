"""Exercise MODES diagnostics against the native console's live binding."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests"))
from gt710_gr_live_unwind_test import item
class NativeConsoleRecord(unittest.TestCase):
    def test_live_geometry_binding_and_handoff_replace_arm_defaults(self):
        source = (ROOT / "kernel/src/gpu/console.rs").read_text()
        function = item(source, "pub(super) fn describe(").replace("pub(super)", "pub")
        function = function.replace('#[cfg(target_arch = "x86_64")]', "")
        fixture = r'''
#![allow(dead_code)] // Extracted diagnostics use controlled hardware/ARM boundaries.
use std::sync::atomic::{AtomicU64,AtomicBool,Ordering};
use gpu_abi as abi; use gpu_abi::Plain;
static FIXTURE:AtomicU64=AtomicU64::new(0);static HASH:AtomicU64=AtomicU64::new(0);
static HANDOFF:AtomicBool=AtomicBool::new(false);
fn active()->bool{false} fn caller()->Result<u64,()>{Ok(1)} fn firmware_hash()->u64{0}
mod display {pub fn owner(_:u64)->bool{false}}
mod arch {pub fn firmware_writes()->u64{29}}
mod screen {pub fn describe()->(u64,u64){(0,0)}}
mod gk208 {
 pub fn console_state()->Option<(gpu_abi::Mode,u64,bool,u64)>{
  Some((gpu_abi::Mode{width:1280,height:1024,scale:1,reserved:0},0xffff_ffff,true,0))
 }
}
mod console {use super::*;
// DESCRIBE
}
fn main(){
 let mut a=abi::Modes::ZERO;a.count=2;a.current=1;
 a.modes=[abi::Mode{width:1280,height:800,scale:1,reserved:0},abi::Mode{width:2560,height:1600,scale:2,reserved:0}];
 console::describe(&mut a);
 assert_eq!((a.count,a.current),(1,0));assert_eq!((a.modes[0].width,a.modes[0].height),(1280,1024));
 assert_eq!(a.modes[1],abi::Mode::ZERO);
 assert_eq!((a.handoff,a.firmware_writes_after_handoff),(1,0));
 assert_eq!((a.console_resource,a.scanout_resource),(0xffff_ffff,0xffff_ffff));assert_eq!(a.binding_known,1);
}
'''.replace("// DESCRIBE",function)
        priority=["taskpolicy","-b"] if sys.platform=="darwin" else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/"console.rs").write_text(fixture)
            subprocess.run(priority+["rustc","--edition=2021","--crate-name","gpu_abi","--crate-type","rlib",str(ROOT/"libs/gpu-abi/src/lib.rs"),"-o",str(p/"libgpu_abi.rlib")],check=True)
            subprocess.run(priority+["rustc","--edition=2021","-Dwarnings","--extern",f"gpu_abi={p / 'libgpu_abi.rlib'}",str(p/"console.rs"),"-o",str(p/"console")],check=True)
            subprocess.run([str(p/"console")],check=True)
if __name__=="__main__":unittest.main()
