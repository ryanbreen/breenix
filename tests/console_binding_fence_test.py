"""Execute the kernel screen's binding function with device completions."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class ConsoleBindingFence(unittest.TestCase):
    def test_completed_kernel_binding_has_a_context_fence(self):
        source = r'''
use std::{cell::RefCell, collections::{BTreeMap, BTreeSet}};
const EIO: i64 = 5;
const ENODEV: i64 = 19;
#[derive(Clone, Copy)] struct Binding { id: u32, fence: u64, answer: u32 }
struct Resource { width: u32, height: u32, lost: bool, holds: usize }
struct Context { ns: u32, issued: u32, done_upto: u32, early: BTreeSet<u32> }
struct Display { known: bool, holds: BTreeSet<u32>, binding: Option<Binding> }
struct Core { resources: BTreeMap<u32, Resource>, contexts: BTreeMap<u32, Context>, display: Display }
impl Core { fn device_fence(&mut self) -> u64 { 91 } }
thread_local! {
 static CORE: RefCell<Core> = RefCell::new(Core {
  resources: [(8, Resource { width: 64, height: 32, lost: false, holds: 0 })].into(),
  contexts: [(7, Context { ns: 12, issued: 3, done_upto: 3, early: BTreeSet::new() })].into(),
  display: Display { known: true, holds: BTreeSet::new(), binding: None },
 });
 static BAD: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}
fn with_core<R>(f: impl FnOnce(&mut Core) -> R) -> Option<R> { Some(CORE.with(|c| f(&mut c.borrow_mut()))) }
struct Done { answer: u32, echoed: u64 }
impl Done { fn matches_fence(&self, f: u64) -> bool { self.echoed == f } }
fn exec_ok(build: impl FnOnce(()) -> Option<u64>, _: u32, _: &[u32]) -> Result<Done, i64> {
 let f = build(()).ok_or(EIO)?;
 if f != 0 && BAD.with(|b| b.get()) == 1 { return Err(EIO); }
 Ok(Done { answer: 0x60000000, echoed: if BAD.with(|b| b.get()) == 2 { f+1 } else { f } })
}
mod dev {
 pub fn set_scanout(_: (), _: u32, _: u32, _: u32, _: u32) -> Option<u64> { Some(0) }
 pub fn flush_resource(_: (), _: u32, fence: u64, _: u32, _: u32, _: u32) -> Option<u64> { Some(fence) }
}
mod display {
 pub fn restored(b: super::Binding) { super::with_core(|c| { c.display.binding = Some(b); c.display.known = true; }); }
}
struct Target { context: u32 }
impl Target {
// BIND
}
#[test] fn completed_console_flush_publishes_a_context_fence() {
 Target { context: 7 }.bind(Binding { id: 8, fence: 0, answer: 0 }).unwrap();
 with_core(|c| {
  let b = c.display.binding.unwrap();
  assert_eq!(b.fence, (12u64 << 32) | 4);
  assert_eq!(b.answer, 0x60000000);
  assert_eq!((c.contexts[&7].issued, c.contexts[&7].done_upto), (4, 4));
  assert_eq!(c.contexts[&7].ns, 12);
  assert!(c.contexts[&7].early.is_empty());
 });
}
#[test] fn saved_owner_fence_is_preserved() {
 Target { context: 7 }.bind(Binding { id: 8, fence: (20u64 << 32) | 9, answer: 0 }).unwrap();
 with_core(|c| assert_eq!(c.display.binding.unwrap().fence, (20u64 << 32) | 9));
}
#[test] fn failed_or_mismatched_flush_does_not_publish_a_fence() {
 for bad in [1, 2] {
  BAD.with(|b| b.set(bad));
  assert_eq!(Target { context: 7 }.bind(Binding { id: 8, fence: 0, answer: 0 }), Err(EIO));
  with_core(|c| {
   assert!(!c.display.known);
   assert!(c.display.binding.is_none());
   assert_eq!((c.contexts[&7].issued, c.contexts[&7].done_upto), (3, 3));
  });
 }
}
'''.replace('// BIND', item((ROOT / 'kernel/src/gpu/screen.rs').read_text(), 'fn bind('))
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='console-binding-') as directory:
            out = Path(directory)
            (out / 'check.rs').write_text(source)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                str(out / 'check.rs'), '-o', str(out / 'check')], check=True)
            subprocess.run([str(out / 'check'), '--test-threads=1'], check=True)


if __name__ == '__main__':
    unittest.main()
