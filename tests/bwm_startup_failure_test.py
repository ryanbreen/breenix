"""An initial pixel or fill mismatch terminates desktop startup."""
from pathlib import Path
import os
import subprocess
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class StartupFailure(unittest.TestCase):
 def test_pixel_and_fill_refusals_exit_before_client_composition(self):
  old=os.environ.get('BWM_STARTUP_OLD_REV')
  s=subprocess.check_output(['git','show',old+':userspace/programs/src/bwm.rs'],cwd=ROOT,text=True) if old else (ROOT/'userspace/programs/src/bwm.rs').read_text()
  frames=item(s,'match first {');fills=item(s,'match fills {')
  run_rust(self,r'''
use std::sync::atomic::{AtomicI32,Ordering};static EXIT:AtomicI32=AtomicI32::new(-1);
mod process{pub fn exit(status:i32)->!{super::EXIT.store(status,super::Ordering::SeqCst);panic!("startup exited")}}
enum CheckError{Moved(String),Failed(String)}
fn frame(first:Result<u64,CheckError>){@FRAME@}
fn fills(fills:Result<String,String>){@FILLS@}
#[test]fn startup_mismatches_are_fatal(){let runtime_exit:fn(i32) -> ! = process::exit;let _=runtime_exit;frame(Ok(4096000));fills(Ok("verified".into()));assert_eq!(EXIT.load(Ordering::SeqCst),-1);for e in [CheckError::Moved("changed".into()),CheckError::Failed("pixel (2559,1599)".into())]{assert!(std::panic::catch_unwind(||frame(Err(e))).is_err(),"pixel mismatch was merely printed");assert_eq!(EXIT.swap(-1,Ordering::SeqCst),1);}assert!(std::panic::catch_unwind(||fills(Err("wrong scanout pixel".into()))).is_err(),"fill mismatch was merely printed");assert_eq!(EXIT.load(Ordering::SeqCst),1);}
'''.replace('@FRAME@',frames).replace('@FILLS@',fills))
if __name__=='__main__':unittest.main()
