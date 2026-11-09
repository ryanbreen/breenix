"""Namespace removal must not drain unrelated disk allocations under space pressure."""
from pathlib import Path
import re
import unittest
from gt710_gr_live_unwind_test import item
from ui_idle_capture_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class RemovalPressure(unittest.TestCase):
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
