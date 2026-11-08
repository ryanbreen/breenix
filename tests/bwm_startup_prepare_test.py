"""Cold title preparation overlaps the exact desktop check and both finish first."""
from pathlib import Path
import unittest
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class Prepare(unittest.TestCase):
 def test_overlap_keeps_validation_and_preparation_errors(self):
  p=ROOT/'userspace/programs/src/bwm/startup_prepare.rs'
  body=p.read_text().replace('//!','//') if p.exists() else 'pub fn during<R>(prepare:impl FnOnce()->Result<(),String>+Send,check:impl FnOnce()->R)->(Result<(),String>,R){let checked=check();(prepare(),checked)}'
  run_rust(self,body+r'''
#[test]fn both_complete_before_return(){
 use std::sync::{mpsc,Arc,atomic::{AtomicBool,Ordering}};
 let (started,ready)=mpsc::channel();let (resume,held)=mpsc::channel();let done=Arc::new(AtomicBool::new(false));let flag=done.clone();
 let (prepared,checked)=during(move||{started.send(()).unwrap();held.recv().unwrap();flag.store(true,Ordering::Release);Ok(())},||{ready.recv_timeout(std::time::Duration::from_millis(100)).expect("title preparation did not run alongside the desktop check");assert!(!done.load(Ordering::Acquire));resume.send(()).unwrap();Err::<(),_>("pixel mismatch")});
 assert!(prepared.is_ok());assert_eq!(checked,Err("pixel mismatch"));assert!(done.load(Ordering::Acquire));
 let (prepared,checked)=during(||Err("glyph device failure".into()),||Ok::<_,String>(4096000u64));assert_eq!(prepared,Err("glyph device failure".into()));assert_eq!(checked,Ok(4096000));
 let (prepared,checked)=during(||panic!("worker fault"),||17);assert!(prepared.is_err());assert_eq!(checked,17);
}
''')
if __name__=='__main__':unittest.main()
