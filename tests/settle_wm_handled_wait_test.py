"""Execute case 3 after WM disposition with launcher registration delayed."""
from pathlib import Path
import os
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class WmHandledWait(unittest.TestCase):
    def test_launcher_registration_precedes_tree_assertion(self):
        source = (ROOT / 'userspace/programs/src/uitest_suite/settle_composite.rs').read_text()
        start = source.index('"hotkey lacks its WM-handled disposition",')
        start = source.index(')?;', start) + len(')?;')
        end = source.index('                require(\n                    snapshot(id)? == before,', start)
        observed = source[start:end]
        harness = r'''
use std::cell::Cell;
#[derive(Debug)] pub struct Ready { pub id: u64, pub presented_ns: u64 }
#[derive(Debug)] pub struct Info { pub owner: u64 }
pub struct Node { pub name: String, pub key: String }
pub struct Root { pub value: u64 }
pub struct Tree { pub root: Root, pub children: Vec<Node> }
pub struct Live { registered: Cell<bool>, missing: bool, snapshots: Cell<u32> }
impl Live {
 pub fn session(&mut self) -> &mut Self { self }
 pub fn wait_window_until(&mut self, title: &str, deadline: u64) -> Result<Ready, String> {
  assert_eq!(title, "\u{1}Launcher", "registry title includes the chromeless marker");
  assert_eq!(deadline, 9_000_000_000, "wait must use the remaining case deadline");
  if self.missing { return Err("wait-window-timeout title=Launcher".into()); }
  self.registered.set(true);
  Ok(Ready { id: 42, presented_ns: 100 })
 }
 pub fn info(&self, id: u64) -> Result<Info, String> {
  assert_eq!(id, 42); Ok(Info { owner: 77 })
 }
 pub fn desktop_tree(&self) -> Result<Tree, String> {
  self.snapshots.set(self.snapshots.get() + 1);
  Ok(Tree { root: Root { value: 8 }, children: if self.registered.get() {
   vec![Node { name: "Launcher".into(), key: "window-42".into() }]
  } else { Vec::new() } })
 }
}
pub fn now_ns() -> Result<u64, String> { Ok(200) }
fn require(ok: bool, why: &str) -> Result<(), String> {
 if ok { Ok(()) } else { Err(why.into()) }
}
fn observe(live: &mut Live) -> Result<(), String> {
 let case_deadline_ns: u64 = 9_000_000_000;
 let mut wm_trace = String::new();
 let last = 123;
 // The old fragment uses none of these diagnostic inputs.
 std::hint::black_box((&case_deadline_ns, &mut wm_trace, &last));
 // OBSERVED
 Ok(())
}
fn main() {
 let mut delayed = Live { registered: Cell::new(false), missing: false, snapshots: Cell::new(0) };
 observe(&mut delayed).expect("tree assertion raced asynchronous Launcher registration");
 assert_eq!(delayed.snapshots.get(), 1);
 let mut missing = Live { registered: Cell::new(false), missing: true, snapshots: Cell::new(0) };
 let error = observe(&mut missing).expect_err("missing launcher must fail");
 assert!(error.starts_with("wait-window-timeout"), "lost wait diagnostic: {error}");
 assert_eq!(missing.snapshots.get(), 0, "read tree after failed registration wait");
}
'''.replace('// OBSERVED', observed)
        with tempfile.TemporaryDirectory(prefix='wm-handled-wait-') as tmp:
            path = Path(tmp)
            (path / 'case.rs').write_text(harness)
            build = subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings',
                                    str(path / 'case.rs'), '-o', str(path / 'case')],
                                   env=dict(os.environ, CARGO_BUILD_JOBS='4'), capture_output=True, text=True)
            self.assertEqual(build.returncode, 0, build.stdout + build.stderr)
            result = subprocess.run([str(path / 'case')], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == '__main__':
    unittest.main()
