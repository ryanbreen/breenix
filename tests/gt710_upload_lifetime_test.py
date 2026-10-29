"""Exercise suite entry points and disk staging across the one-use evidence upload."""
import hashlib
import json
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item
from metal_disk_test import DiskTest
import metal_disk
import metal_root

API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
CORE = runpy.run_path(str(ROOT / 'tests/gt710_core_report_test.py'))


class UploadLifetime(unittest.TestCase):
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
        for name in ['console', 'gpu']:
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

    def test_disk_stages_upload_for_named_and_combined_native_suites(self):
        fixture = DiskTest()
        fixture.setUp()
        try:
            original = (fixture.inputs / 'ext2.img').read_bytes()
            for mode, suite, wanted in [('probe', None, True), ('suite', 'console', True),
                                        ('suite', 'gpu', True), ('suite', 'console,gpu', True),
                                        ('suite', 'smoke,gpu', True), ('suite', 'smoke', False),
                                        ('console', None, False)]:
                with self.subTest(mode=mode, suite=suite), \
                     patch.object(metal_root, 'stage_root') as root, \
                     patch.object(metal_disk.gt710_upload, 'prepare', return_value='fixture-route') as prepare:
                    (fixture.inputs / 'ext2.img').write_bytes(original)
                    metal_disk.stage(fixture.directory, fixture.inputs, 'suite', CORE['SHA'],
                                     fixture.site, fixture.build, mode=mode, suite=suite)
                    self.assertEqual(prepare.call_count, int(wanted))
                    self.assertEqual(root.call_args.kwargs['gt710_upload'], 'fixture-route' if wanted else None)
        finally:
            fixture.tearDown()

    def test_gpu_companion_authenticates_and_retains_startup_bulk(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            row = b'GT710_CORE_STARTUP {"schema":1,"irq_masked_max_us":37,"ready":true}\n'
            anchor = f'{len(row)} {hashlib.sha256(row).hexdigest()}\n'
            serial = 'GT710_CORE_STARTUP_BULK ' + anchor + 'GT710_BULK ' + anchor + CORE['gpu_serial']()
            files = {'run.json': json.dumps(CORE['gpu_boot']()), 'serial.txt': serial,
                     'events.jsonl': '{"event":"final-marker"}\n', 'ended': '1',
                     'exit-status': '1', 'dirty': '0', 'commit': CORE['SHA']}
            for name, text in files.items():
                (path / name).write_text(text)
            (path / 'gt710-pixels.rle').write_bytes(row)
            capture = API['core_producer_capture'](path, CORE['SHA'])
            self.assertEqual(API['core_startup'](capture['serial']), 37)
            (path / 'gt710-pixels.rle').write_bytes(row.replace(b'37', b'36'))
            with self.assertRaises(ValueError):
                API['core_producer_capture'](path, CORE['SHA'])


if __name__ == '__main__':
    unittest.main()
