"""A reader ahead of the upgrader must not impose the 200ms park timeout."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class UpgradeWake(unittest.TestCase):
 def test_last_reader_releases_eligible_upgrader_in_shared_queue(self):
  source=(ROOT/'kernel/src/fs/ext2/mod.rs').read_text()
  drop=item(item(source,'impl Drop for Ext2ReadGuard'),'fn drop(')
  run_rust(self,r'''
use std::{collections::VecDeque,sync::{Mutex,atomic::{AtomicUsize,AtomicBool,Ordering}}};
static READERS:AtomicUsize=AtomicUsize::new(1);static WRITER:AtomicBool=AtomicBool::new(false);
#[derive(Clone,Copy)]enum Kind{Reader,Upgrade}
struct Queue(Mutex<VecDeque<Kind>>);impl Queue{
 fn wake(&self,k:Kind){assert_eq!(READERS.load(Ordering::SeqCst),0,"woke while inner guard remained held");if matches!(k,Kind::Upgrade){WRITER.store(true,Ordering::SeqCst);}}
 fn wake_up_one(&self){let next=self.0.lock().unwrap().pop_front();if let Some(k)=next{self.wake(k)}}
 fn wake_up(&self){while let Some(k)=self.0.lock().unwrap().pop_front(){self.wake(k)}}
}
struct Inner;impl Drop for Inner{fn drop(&mut self){READERS.fetch_sub(1,Ordering::SeqCst);}}
struct Ext2ReadGuard<'a>{inner:Option<Inner>,waiters:&'a Queue}
impl Drop for Ext2ReadGuard<'_>{@DROP@}
#[test]fn reader_ahead_of_upgrader(){let wake_methods:[fn(&Queue);2]=[Queue::wake_up_one,Queue::wake_up];assert_eq!(wake_methods.len(),2);for kinds in [[Kind::Reader,Kind::Upgrade],[Kind::Upgrade,Kind::Reader]]{READERS.store(1,Ordering::SeqCst);WRITER.store(false,Ordering::SeqCst);let q=Queue(Mutex::new(VecDeque::from(kinds)));drop(Ext2ReadGuard{inner:Some(Inner),waiters:&q});assert!(WRITER.load(Ordering::SeqCst),"eligible upgrader remains parked until EXT2_LOCK_PARK_TIMEOUT_NS=200000000");}}
'''.replace('@DROP@',drop))
if __name__=='__main__':unittest.main()
