"""Exercise the procfs stage-control producer used by native live panel checks."""
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class StageControl(unittest.TestCase):
    def test_x86_stage_fault_delivery(self):
        source = (ROOT / 'kernel/src/fs/procfs/mod.rs').read_text()
        start = source.index('        ProcEntryType::StageControl => {')
        end = source.index('        ProcEntryType::BootRibbon => {', start)
        route = source[source.rfind('\n', 0, start - 1):end]
        self.assertNotIn('#[cfg(target_arch="aarch64")]', route,
                         'x86 must receive the stage fault bits, not ENOSYS')
        self.assertNotIn('ProcEntryType::StageControl => Err(-38)', route)
        body = source[start:source.index('\n        }', start) + len('\n        }')]
        constant = re.search(r'pub const PANEL_CURSOR_NO_RESTORE: u32 = (\d+);',
                             (ROOT / 'libs/breengel-abi/src/lib.rs').read_text())[1]
        fixture = r'''
extern crate alloc;
extern crate self as spin;
extern crate self as breengel_abi;
pub struct Mutex<T>(std::sync::Mutex<T>);
impl<T> Mutex<T> {
    pub const fn new(v: T) -> Self { Self(std::sync::Mutex::new(v)) }
    pub fn lock(&self) -> std::sync::MutexGuard<'_, T> { self.0.lock().unwrap() }
}
pub mod tc_point { pub const PANEL_CURSOR_NO_RESTORE: u32 = CONSTANT; }
#[path="ROOT/kernel/src/test_control/point.rs"] pub mod point;
mod test_control {
    use std::sync::atomic::{AtomicBool, Ordering};
    pub static ENABLED: AtomicBool = AtomicBool::new(false);
    pub static POINTS: [crate::point::Point; 15] = [const { crate::point::Point::new() }; 15];
    pub fn enabled() -> bool { ENABLED.load(Ordering::SeqCst) }
    pub fn take(point: u32, target: u64) -> Option<u64> {
        Some(POINTS[point as usize].reserve(target, |_| true)?.fire())
    }
}
mod window {
    use std::sync::atomic::{AtomicBool, Ordering};
    pub static ROOT: AtomicBool = AtomicBool::new(true);
    pub struct Caller { pub tgid: u64 }
    impl Caller {
        pub fn current() -> Result<Self, i64> { Ok(Self { tgid: 7 }) }
        pub fn is_root(&self) -> bool { ROOT.load(Ordering::SeqCst) }
    }
}
enum ProcEntryType { StageControl }
fn text(s: String) -> Result<Vec<u8>, i64> { Ok(s.into_bytes()) }
fn read() -> Result<Vec<u8>, i64> {
    match ProcEntryType::StageControl { PRODUCTION_ARM }
}
#[test]
fn stage_faults_are_gated_targeted_and_one_shot() {
    use std::sync::atomic::Ordering::SeqCst;
    for p in [10, 12, 13, 14] { test_control::POINTS[p].arm(7, 0); }
    assert_eq!(read(), Err(-38));
    test_control::ENABLED.store(true, SeqCst);
    window::ROOT.store(false, SeqCst);
    assert_eq!(read(), Err(-1));
    window::ROOT.store(true, SeqCst);
    assert_eq!(read().unwrap(), b"29696\n");
    for p in [10, 12, 13, 14] { assert_eq!(test_control::POINTS[p].fired(), 1); }
    assert_eq!(read().unwrap(), b"0\n");
    // A different process cannot spend an arm; an unrelated control is untouched.
    test_control::POINTS[13].arm(8, 0);
    test_control::POINTS[1].arm(7, 0);
    assert_eq!(read().unwrap(), b"0\n");
    assert_eq!(test_control::POINTS[13].fired(), 1);
    assert!(test_control::take(13, 8).is_some());
    assert!(test_control::take(1, 7).is_some());
}
'''.replace('CONSTANT', constant).replace('ROOT/kernel', str(ROOT / 'kernel'))
        fixture = fixture.replace('PRODUCTION_ARM', body)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory(prefix='x86-stage-control-') as directory:
            path = Path(directory)
            (path / 'test.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings',
                                       str(path / 'test.rs'), '-o', str(path / 'test')], check=True)
            subprocess.run([str(path / 'test')], check=True, timeout=10)


if __name__ == '__main__':
    unittest.main()
