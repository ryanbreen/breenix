"""Shared close-range syscall: inclusive bounds and the ordinary close path."""
from pathlib import Path
import unittest
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class CloseRange(unittest.TestCase):
    def test_sparse_table_boundaries_and_refusals(self):
        path = ROOT / 'kernel/src/syscall/close_range.rs'
        self.assertTrue(path.exists(), 'no shared close-range implementation')
        run_rust(self, r'''
extern crate self as alloc;
pub mod vec{pub use std::vec::Vec;}
use std::{collections::BTreeSet,sync::Mutex};
#[derive(Debug,PartialEq)]pub enum SyscallResult{Ok(u64),Err(u64)}
pub mod errno{pub const EINVAL:u32=22;pub const ESRCH:u32=3;}
pub mod ipc{pub mod fd{pub const MAX_FDS:usize=4096;}}
static TABLE:Mutex<BTreeSet<i32>>=Mutex::new(BTreeSet::new());
static CLOSED:Mutex<Vec<i32>>=Mutex::new(Vec::new());
pub mod task{pub mod scheduler{pub fn current_thread_id()->Option<u64>{Some(1)}}}
pub struct Table;impl Table{fn get(&self,n:i32)->Option<()>{TABLE.lock().unwrap().contains(&n).then_some(())}}
pub struct Process{fd_table:Table}pub struct Manager;
impl Manager{fn find_process_by_thread(&self,_:u64)->Option<(u64,Process)>{Some((1,Process{fd_table:Table}))}}
pub mod process{pub fn manager()->Option<super::Manager>{Some(super::Manager)}}
pub mod pipe{use super::*;pub fn sys_close(fd:i32)->SyscallResult{assert!(TABLE.lock().unwrap().remove(&fd));CLOSED.lock().unwrap().push(fd);SyscallResult::Ok(0)}}
#[path="IMPL"]mod implementation;
#[test]fn inclusive_bounds_and_ordinary_cleanup(){
 for (first,last,flags,expected) in [(3,u32::MAX,0,vec![3,7,4095]),(7,7,0,vec![7]),(4096,u32::MAX,0,vec![])]{
  *TABLE.lock().unwrap()=BTreeSet::from([0,1,2,3,7,4095]);CLOSED.lock().unwrap().clear();
  assert_eq!(implementation::sys_close_range(first,last,flags),SyscallResult::Ok(0));
  assert_eq!(*CLOSED.lock().unwrap(),expected);
 }
 for (first,last,flags) in [(8,7,0),(0,u32::MAX,1)]{
  let before=TABLE.lock().unwrap().clone();assert_eq!(implementation::sys_close_range(first,last,flags),SyscallResult::Err(22));assert_eq!(*TABLE.lock().unwrap(),before);
 }
}
'''.replace('IMPL', str(path)))
        dispatch = (ROOT / 'kernel/src/syscall/mod.rs').read_text()
        self.assertIn('436 => close_range::sys_close_range(a[0] as u32, a[1] as u32, a[2] as u32)', dispatch)
        numbers = (ROOT / 'libs/libbreenix/src/syscall.rs').read_text()
        self.assertEqual(numbers.count('pub const CLOSE_RANGE: u64 = 436;'), 2)


if __name__ == '__main__': unittest.main()
