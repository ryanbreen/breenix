"""Exercise the production C3 readback closure after the lock-test window."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class LockWindow(unittest.TestCase):
    def test_post_sync_status_and_frequency_guards(self):
        runtime = (ROOT / 'kernel/src/gpu/gk208/c2/runtime.rs').read_text()
        # Compile the actual runtime validation, including its source/control
        # predicate, against synthetic MMIO. The programmer's lock poll/order
        # and timeout refusal are exercised by gt710_c2_test.py.
        validation = runtime.split('let result=result.and_then(|()| {', 1)[1].split('\n    });', 1)[0]
        source = r'''
struct Io { select:u32, control:u32 }
impl Io { fn read(&mut self,a:u32)->u32 {match a {0x137100=>self.select,0x137000=>self.control,_=>panic!("unexpected read")}} }
fn validate(actual:u32,requested:u32,select:u32,control:u32)->Result<(),&'static str> {
    let c3=true;let phase=1;let readback:Result<u32,&'static str>=Ok(actual);
    let mut io=Io {select,control};
    (|| {VALIDATION})()
}
fn main() {
    // Prior hardware capture: lock poll succeeded, bit17 clears after sync.
    assert_eq!(validate(810000,810000,1,0x10015),Ok(()));
    assert_eq!(validate(810000,810000,1,0x30015),Ok(()));
    for bit in [1,4,0x10] {
        assert!(validate(810000,810000,1,0x10015 & !bit).is_err());
    }
    assert!(validate(810000,810000,0,0x10015).is_err());
    assert!(validate(801899,810000,1,0x10015).is_err());
    assert!(validate(818101,810000,1,0x10015).is_err());
    assert_eq!(validate(801900,810000,1,0x10015),Ok(()));
    assert_eq!(validate(818100,810000,1,0x10015),Ok(()));
}
'''.replace('VALIDATION', validation)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / 'check.rs').write_text(source)
            subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(path / 'check.rs'), '-o', str(path / 'check')], check=True)
            subprocess.run([str(path / 'check')], check=True)


if __name__ == '__main__':
    unittest.main()
