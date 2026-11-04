"""Exercise desktop terminal ordering against the shared GPU admission state."""
from pathlib import Path
import subprocess
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class DesktopAdmission(unittest.TestCase):
    def test_terminal_waits_for_admission_and_refuses_timeout(self):
        source = (ROOT / 'userspace/programs/src/init.rs').read_text()
        desktop = item(source, 'fn run_desktop_mode(')
        self.assertLess(desktop.index('desktop_gpu_ready()'), desktop.index('spawn(b"/usr/bin/bterm'))
        helper = item(source, 'fn desktop_gpu_ready(')
        fixture = r'''
use std::sync::atomic::{AtomicUsize, Ordering};
static CALLS: AtomicUsize = AtomicUsize::new(0);
static SLEEPS: AtomicUsize = AtomicUsize::new(0);
static ADMIT_AT: AtomicUsize = AtomicUsize::new(3);
mod libbreenix {
    pub mod gpu {
        pub mod abi { pub mod status { pub const ADMITTED: u32 = 1; } }
        pub struct Caps { pub status: u32 }
        pub fn caps() -> (i64, Caps) {
            let n = crate::CALLS.fetch_add(1, crate::Ordering::SeqCst) + 1;
            (0, Caps { status: u32::from(n >= crate::ADMIT_AT.load(crate::Ordering::SeqCst)) })
        }
    }
    pub mod time {
        pub fn sleep_ms(ms: u64) -> Result<(), ()> {
            assert_eq!(ms, 100);
            crate::SLEEPS.fetch_add(1, crate::Ordering::SeqCst); Ok(())
        }
    }
}
// HELPER
#[test]
fn pending_device_is_not_terminal_readiness() {
    assert!(desktop_gpu_ready());
    assert_eq!(CALLS.load(Ordering::SeqCst), 3);
    assert_eq!(SLEEPS.load(Ordering::SeqCst), 2);
    CALLS.store(0, Ordering::SeqCst);
    SLEEPS.store(0, Ordering::SeqCst);
    ADMIT_AT.store(usize::MAX, Ordering::SeqCst);
    assert!(!desktop_gpu_ready());
    assert_eq!(SLEEPS.load(Ordering::SeqCst), 50);
}
'''.replace('// HELPER', helper)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'admission.rs').write_text(fixture)
            subprocess.run(['rustc', '--edition=2021', '--test', str(path / 'admission.rs'),
                            '-o', str(path / 'admission')], check=True, cwd=ROOT)
            subprocess.run([str(path / 'admission')], check=True)


if __name__ == '__main__':
    unittest.main()
