"""Run the measured-size cases' production setup against display catalogues."""
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class PerformanceMode(unittest.TestCase):
    def test_measured_extent_selects_smallest_containing_mode_or_fails(self):
        source = (ROOT / 'userspace/programs/src/suite_gpu/performance.rs').read_text()
        constants = '\n'.join(re.findall(r'^const (?:W|H|MODE_2560):[^;]+;', source, re.M))
        setup = item(source, 'fn take_display_2560(')
        code = r'''
extern crate gpu_abi;
use gpu_abi::{Mode, Modes, Plain};
type R<T> = Result<T, String>;
fn fail_msg(s: String) -> String { s }
fn errno(r: i64) -> String { r.to_string() }
fn ensure(ok: bool, msg: impl FnOnce() -> String) -> R<()> { if ok { Ok(()) } else { Err(msg()) } }
mod libbreenix { pub mod graphics { pub fn take_over_display() -> Result<(), i64> { Ok(()) } } }
mod gpu {
 use super::*;
 pub static LIST: std::sync::Mutex<Modes> = std::sync::Mutex::new(Modes::ZERO);
 pub static SELECTED: std::sync::Mutex<Option<u32>> = std::sync::Mutex::new(None);
 pub fn modes() -> (i64, Modes) { (0, *LIST.lock().unwrap()) }
 pub fn set_mode(i: u32) -> i64 { *SELECTED.lock().unwrap() = Some(i); 0 }
}
// CONSTANTS
// SETUP
fn catalogue(entries: &[(u32,u32,u32)]) {
 let mut m = Modes::ZERO; m.count = entries.len() as u32;
 for (slot, &(width,height,scale)) in m.modes.iter_mut().chain(m.extra_modes.iter_mut()).zip(entries) {
  *slot = Mode { width,height,scale,reserved:0 };
 }
 *gpu::LIST.lock().unwrap() = m; *gpu::SELECTED.lock().unwrap() = None;
}
fn main() {
 assert_eq!((W,H),(2560,1600));
 for (entries, expected) in [
  (vec![(1024,768,1),(1280,1024,1),(2560,2048,2)], 2),
  (vec![(1280,800,1),(2560,1600,2)], 1),
  (vec![(3840,2160,1),(4096,1200,1),(2560,1600,1),(2560,2048,2)], 2),
 ] {
  catalogue(&entries); assert_eq!(gpu::modes().1.count, entries.len() as u32);
  take_display_2560().unwrap();
  assert_eq!(*gpu::SELECTED.lock().unwrap(), Some(expected), "catalogue {entries:?}");
 }
 catalogue(&[(4096,1200,1),(1280,1024,1)]);
 let error = take_display_2560().unwrap_err();
 assert!(error.contains("2560") && error.contains("1600") && error.contains("4096")
  && error.contains("1200") && error.contains("1280") && error.contains("1024"), "{error}");
 assert_eq!(*gpu::SELECTED.lock().unwrap(), None);
}
'''.replace('// CONSTANTS', constants).replace('// SETUP', setup)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as d:
            p = Path(d)
            (p / 'check.rs').write_text(code)
            lib = p / 'libgpu_abi.rlib'
            subprocess.run(priority + ['rustc', '--edition=2021', '--crate-name', 'gpu_abi', '--crate-type', 'rlib',
                            str(ROOT / 'libs/gpu-abi/src/lib.rs'), '-o', str(lib)], check=True)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(p / 'check.rs'),
                            '--extern', f'gpu_abi={lib}', '-o', str(p / 'check')], check=True)
            subprocess.run([str(p / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
