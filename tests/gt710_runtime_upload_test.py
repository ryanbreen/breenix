"""Require the native suite to upload runtime recovery evidence once."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tests'))
from gt710_gr_live_unwind_test import item

class RuntimeUpload(unittest.TestCase):
    def test_suite_entry_points_upload_their_complete_evidence_once(self):
        harness = r'''
use std::sync::{Mutex, OnceLock};
static EVENTS: Mutex<Vec<&str>> = Mutex::new(Vec::new());
static LOGIN_CREDENTIALS: OnceLock<()> = OnceLock::new();
fn login_credentials() {}
pub mod libbreenix { pub mod gpu {
    pub mod abi { pub mod backend { pub const GK208: u32 = 6; } }
    pub struct Caps { pub backend: u32 }
    pub fn caps() -> (i64, Caps) {
        super::super::EVENTS.lock().unwrap().push("startup-row"); (0, Caps {backend: 6})
    }
} }
pub mod gt710_wait_probe {
    pub fn run() {
        let mut events = super::EVENTS.lock().unwrap();
        assert!(!events.contains(&"startup-row"), "CE checks need their original owner");
        events.push("CE-checks");
    }
}
pub mod gt710_artifacts {
    pub fn upload(resources: Option<&super::libbreenix::gpu::Caps>) -> Result<(), &'static str> {
        let mut events = super::EVENTS.lock().unwrap();
        assert!(events.contains(&"startup-row"), "one-use upload missed first-CAPS startup");
        if resources.is_some() {
            assert!(events.contains(&"runtime-reprobe"), "one-use upload missed runtime probe evidence");
        }
        assert!(!events.contains(&"upload"), "upload route already consumed");
        events.push("upload"); Ok(())
    }
}
struct Suite;
static SUITE: Suite = Suite;
impl Suite {
    #[allow(dead_code)] // Harness accepts either production entry-point API.
    fn run(&self) -> ! { self.run_with_finalizer(|| {}) }
    fn run_with_finalizer(&self, finalize: impl FnOnce()) -> ! {
        EVENTS.lock().unwrap().push("runtime-reprobe");
        finalize();
        assert_eq!(EVENTS.lock().unwrap().iter().filter(|&&e| e == "upload").count(), 1);
        std::process::exit(0)
    }
}
// ENTRY
fn main() { producer_main(); }
'''
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        for name in ['gpu']:
            with self.subTest(suite=name), tempfile.TemporaryDirectory() as tmp:
                source = (ROOT / f'userspace/programs/src/suite_{name}.rs').read_text()
                entry = item(source, 'fn main()').replace('fn main()', 'fn producer_main()')
                entry = entry.replace('#[cfg(target_arch="x86_64")]', '')
                entry = entry.replace('#[cfg(not(target_arch="x86_64"))]\n    SUITE.run()', '')
                fixture = harness
                if name == 'gpu':
                    # These items belong only to the console's entry point.
                    fixture = fixture.replace('use std::sync::{Mutex, OnceLock};', 'use std::sync::Mutex;')
                    fixture = fixture.replace('static LOGIN_CREDENTIALS: OnceLock<()> = OnceLock::new();\nfn login_credentials() {}\n', '')
                    fixture = fixture.replace('pub ' + item(fixture, 'mod gt710_wait_probe'), '')
                path = Path(tmp)
                (path / 'entry.rs').write_text(fixture.replace('// ENTRY', entry))
                subprocess.run(priority + ['rustc', '--edition=2021', '-Dwarnings', str(path / 'entry.rs'),
                                           '-o', str(path / 'entry')], check=True)
                subprocess.run([str(path / 'entry')], check=True)


if __name__=='__main__':unittest.main()
