"""Namespace removal must not drain unrelated disk allocations under space pressure."""
from pathlib import Path
import re
import unittest
from gt710_gr_live_unwind_test import item
from ui_idle_capture_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class RemovalPressure(unittest.TestCase):
    def test_removal_keeps_exclusion_shrink_wait_and_errors_on_both_mounts(self):
        method = item((ROOT / 'kernel/src/fs/namei.rs').read_text(), '    pub fn write_removal(')
        guard = item((ROOT / 'kernel/src/fs/ext2/mod.rs').read_text(), 'pub(crate) fn fs_sync_guard(')
        adapter = item((ROOT / 'kernel/src/fs/ext2/adapter.rs').read_text(), '    pub fn exclusive(')
        sync = item((ROOT / 'kernel/src/fs/ext2/mod.rs').read_text(), 'pub(super) fn sync_mount_guard(')
        result = run_rust(r'''
use std::sync::{Mutex,atomic::{AtomicBool,Ordering}};
static CALLS:Mutex<Vec<(bool,&str)>>=Mutex::new(Vec::new());
static FAIL:AtomicBool=AtomicBool::new(false);
#[derive(Debug,PartialEq)]struct Errno(u64);
fn errno(error:&str)->Errno{assert_eq!(error,"interrupted");Errno(4)}
struct Ext2Adapter{mount:ext2::Pin}
impl Ext2Adapter{ADAPTER}
use ext2::sync_mount_guard;
mod mounts{
 pub fn ext2_adapter(id:usize)->Result<crate::Ext2Adapter,crate::Errno>{
  assert!(id<2);Ok(crate::Ext2Adapter{mount:crate::ext2::Pin(id==1)})
 }
}
mod ext2{
 #[derive(Clone,Copy)]pub struct Pin(pub bool);
 impl Pin{fn verify(self,fs:&Ext2WriteGuard)->Result<(),&'static str>{assert_eq!(self.0,fs.home);Ok(())}}
 fn pin_is_home(pin:Pin)->Result<bool,&'static str>{Ok(pin.0)}
 pub mod live_inode{pub use super::Pin as MountPin;}
 pub struct Ext2WriteGuard{pub home:bool}
 impl Ext2WriteGuard{
  fn as_ref(&self)->Option<&Self>{Some(self)}
  fn wait_for_shrink_writer(self)->Result<Self,&'static str>{
   crate::CALLS.lock().unwrap().push((self.home,"shrink-wait"));
   if crate::FAIL.load(crate::Ordering::Relaxed){Err("interrupted")}else{Ok(self)}
  }
 }
 fn fs_write_raw(home:bool)->Result<Ext2WriteGuard,&'static str>{
  crate::CALLS.lock().unwrap().push((home,"write-lock"));Ok(Ext2WriteGuard{home})
 }
 GUARD
 SYNC
}
#[derive(Clone,Copy)]struct Mount(usize);
impl Mount{METHOD}
fn main(){
 for (mount,home) in [(Mount(0),false),(Mount(1),true)]{
  CALLS.lock().unwrap().clear();let guard=mount.write_removal().unwrap();assert_eq!(guard.home,home);
  assert_eq!(*CALLS.lock().unwrap(),[(home,"write-lock"),(home,"shrink-wait")]);
  FAIL.store(true,Ordering::Relaxed);assert_eq!(mount.write_removal().err(),Some(Errno(4)));
  FAIL.store(false,Ordering::Relaxed);
 }
}
'''.replace('METHOD', method).replace('GUARD', guard).replace('ADAPTER', adapter).replace('SYNC', sync))
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_unlink_and_rmdir_use_a_nonallocating_guard(self):
        text = (ROOT / 'kernel/src/syscall/fs.rs').read_text()
        functions = []
        for operation in ['unlink', 'rmdir']:
            body = item(text, f'fn sys_{operation}_from(')
            guards = re.findall(r'let mut (\w+) = match mount\.\w+\(\) \{[^\n]+\};', body)
            self.assertEqual(len(guards), 1)
            line = next(line.strip() for line in body.splitlines() if f'let mut {guards[0]} = match mount.' in line)
            functions.append(f'fn {operation}(mount:Mount)->SyscallResult{{{line} let _={guards[0]}.as_mut();SyscallResult::Ok}}')
        result = run_rust(r'''
use std::sync::atomic::{AtomicUsize,Ordering};
static DRAINS:AtomicUsize=AtomicUsize::new(0);
#[derive(Clone,Copy)]enum Mount{Root,Home}
impl Mount{
 fn write(self)->Result<Option<u8>,u64>{DRAINS.fetch_add(1,Ordering::Relaxed);Err(5)}
 fn write_removal(self)->Result<Option<u8>,u64>{Ok(Some(1))}
}
#[derive(Debug,PartialEq)]enum SyscallResult{Ok,Err(u64)}
FUNCTIONS
fn main(){
 assert!(Mount::Root.write().is_err(),"allocation-pressure control must drain");
 assert!(Mount::Home.write_removal().is_ok());DRAINS.store(0,Ordering::Relaxed);
 for mount in [Mount::Root,Mount::Home]{
  assert_eq!(unlink(mount),SyscallResult::Ok,"unlink drained unrelated orphan disk work");
  assert_eq!(rmdir(mount),SyscallResult::Ok,"rmdir drained unrelated orphan disk work");
 }
 assert_eq!(DRAINS.load(Ordering::Relaxed),0);
}
'''.replace('FUNCTIONS', '\n'.join(functions)))
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
