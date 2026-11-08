"""An eligible graphics-lock holder must run before a waiter exhausts a timeslice."""
from pathlib import Path
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust
ROOT=Path(__file__).resolve().parents[1]
class Contention(unittest.TestCase):
    def test_waiter_schedules_holder_and_preserves_fork_ownership(self):
        source=(ROOT/'libs/libgfx/src/device.rs').read_text()
        acquire=item(source,'pub(crate) fn acquire(').replace('core::hint::spin_loop();','spin_tick();')
        run_rust(self,r'''
use std::sync::atomic::{AtomicU64,AtomicUsize,Ordering};
static OWNER:AtomicU64=AtomicU64::new(85);
static YIELDS:AtomicUsize=AtomicUsize::new(0);
static SPINS:AtomicUsize=AtomicUsize::new(0);
fn spin_tick(){assert!(SPINS.fetch_add(1,Ordering::Relaxed)<2,"graphics waiter spun while the holder needed scheduling");}
mod libbreenix{pub mod process{pub fn yield_now()->Result<(),()>{super::super::YIELDS.fetch_add(1,super::super::Ordering::Relaxed);super::super::OWNER.store(84,super::super::Ordering::Release);Ok(())}}}
@ACQUIRE@
#[test]fn held_across_a_device_wait(){
 let _=spin_tick;let _=libbreenix::process::yield_now;
 assert!(!acquire(&OWNER,42));assert_eq!(OWNER.load(Ordering::Acquire),85);assert_eq!(YIELDS.load(Ordering::Relaxed),1);
 OWNER.store(84,Ordering::Release);assert!(!acquire(&OWNER,42));assert_eq!(YIELDS.load(Ordering::Relaxed),1);
 // A fork abandons a foreign held owner without waiting for a vanished thread.
 OWNER.store(85,Ordering::Release);assert!(acquire(&OWNER,43));assert_eq!(OWNER.load(Ordering::Acquire),87);assert_eq!(YIELDS.load(Ordering::Relaxed),1);
}
'''.replace('@ACQUIRE@',acquire))
if __name__=='__main__':unittest.main()
