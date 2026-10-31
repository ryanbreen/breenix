"""Native panic output must preserve production reset and testing exit."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]
class PanicReset(unittest.TestCase):
    def test_native_output_returns_to_the_selected_terminal_policy(self):
        source=(ROOT/'kernel/src/main.rs').read_text()
        body=source[source.index('    if kernel::gpu::console::active() {',source.index('fn panic(info:')):]
        body=body[:body.index('    // Disable interrupts and halt')]
        fixture=r'''
#![allow(dead_code)] // Three build profiles exercise different terminal branches.
use std::sync::{Mutex,atomic::{AtomicBool,Ordering}};
static ACTIVE:AtomicBool=AtomicBool::new(true);static EVENTS:Mutex<Vec<&str>>=Mutex::new(Vec::new());
fn event(s:&'static str){EVENTS.lock().unwrap().push(s);}
mod kernel {
 pub mod gpu {
  pub mod console {pub fn active()->bool{crate::ACTIVE.load(crate::Ordering::Acquire)}}
  pub mod panic {
   pub fn show_panic(_: &(),_: &[u8]){crate::event("show");}
   pub fn park()->!{crate::event("park");panic!("park")}
  }
 }
 pub enum QemuExitCode{Failed}
 pub fn exit_qemu(_:QemuExitCode){crate::event("exit");panic!("exit")}
 pub mod arch_impl {pub mod x86_64 {pub mod platform {
  pub fn reset_available()->bool{true}
  pub fn reset()->!{crate::event("reset");panic!("reset")}
 }}}
}
fn handle(){let info=&();let recent=[0u8;1];let recent_len=1;
BODY
panic!("halt");}
fn main(){for active in [true,false] {
 ACTIVE.store(active,Ordering::Release);EVENTS.lock().unwrap().clear();
 assert!(std::panic::catch_unwind(handle).is_err());
 let mut expected=vec![];if active {expected.push("show");}
 expected.push(if active&&cfg!(feature="capture_panic_oracle"){"park"}else if cfg!(feature="testing"){"exit"}else{"reset"});
 assert_eq!(*EVENTS.lock().unwrap(),expected);
}}
'''.replace('BODY',body)
        priority=['taskpolicy','-b'] if sys.platform=='darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p=Path(tmp);(p/'panic.rs').write_text(fixture)
            for features in [[],['capture_panic_oracle'],['testing']]:
                cfg=[x for f in features for x in ['--cfg',f'feature="{f}"']]
                subprocess.run(priority+['rustc','--edition=2021','-Dwarnings',*cfg,str(p/'panic.rs'),'-o',str(p/'panic')],check=True)
                subprocess.run([str(p/'panic')],check=True,capture_output=True)
if __name__=='__main__':unittest.main()
