"""Run the WM checkpoint with a stopped screen capture; earlier phases must survive."""
from pathlib import Path
import subprocess
import tempfile
import unittest

from ui_idle_capture_test import function

ROOT = Path(__file__).resolve().parents[1]


class PhaseRecords(unittest.TestCase):
    def test_completed_phases_survive_a_stopped_checkpoint(self):
        checkpoint = function('userspace/programs/src/uitest_suite/proof.rs', 'fn wm_checkpoint(').replace('std::time::Instant', 'Clock')
        binding = function('userspace/programs/src/uitest_suite/proof.rs', 'fn wm_front_matches(')
        front = function('userspace/programs/src/uitest_suite/proof_wm_checks.rs', 'pub fn front_matches(')
        source = (ROOT / 'tests/fixtures/wm_snapshot.rs').read_text()
        source = source[:source.index('fn main()')]
        source = source.replace('CHECKPOINT', checkpoint).replace('BINDING', binding).replace('FRONT', front)
        # The old checkpoint has no reporter. Run that body too, so red means
        # missing output before the stop rather than an unavailable new API.
        reporter = ROOT / 'libs/libuitest/src/phases.rs'
        scope = ''
        report_source = ''
        if reporter.exists():
            source = source.replace('mod libuitest {', 'mod libuitest { pub use crate::phases;')
            report_source = reporter.read_text().replace('//!', '//')
            report_source = report_source.replace('std::fs::', 'crate::evidence_fs::')
            scope = 'let _scenario = phases::Scenario::new("focus");'
        source += '\npub mod phases {\n' + report_source + '\n}\n'
        source = source.replace('s.screens+=1;', '''s.screens+=1;
            if STOP.load(std::sync::atomic::Ordering::Relaxed) { panic!("runner stopped case"); }''')
        source += r'''
static OUTPUT: std::sync::Mutex<Vec<u8>> = std::sync::Mutex::new(Vec::new());
static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub mod evidence_fs {
    pub struct OpenOptions;
    impl OpenOptions {
        pub fn new()->Self { Self }
        pub fn write(&mut self, _:bool)->&mut Self { self }
        pub fn open(&self, path:&str)->std::io::Result<Sink> {
            assert_eq!(path, "/dev/screen-evidence"); Ok(Sink)
        }
    }
    pub struct Sink;
    impl std::io::Write for Sink {
        fn write(&mut self, bytes:&[u8])->std::io::Result<usize> {
            super::OUTPUT.lock().unwrap().extend_from_slice(bytes); Ok(bytes.len())
        }
        fn flush(&mut self)->std::io::Result<()> { Ok(()) }
    }
}
fn rows()->Vec<String> {
    String::from_utf8(OUTPUT.lock().unwrap().clone()).unwrap().lines().map(str::to_string).collect()
}
fn main() {
    assert!(matches!(SlotRead::Incomplete, SlotRead::Incomplete));
    SCOPE
    STATE.with(|s| *s.borrow_mut()=State { generation:9, ..State::default() });
    STOP.store(true, std::sync::atomic::Ordering::Relaxed);
    let mut session=Session;
    let stopped=std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wm_checkpoint(&mut LivePointer(&mut session),[143,144],[1,2],1,[1280,800],1,["",""])
    }));
    assert!(stopped.is_err());
    let before=rows();
    for phase in ["park", "settle", "desktop-tree", "arena", "client", "tree"] {
        assert!(before.iter().any(|r| r.split_whitespace().nth(4)==Some(phase)),
                "completed {phase} must be emitted before the stopped screen capture: {before:?}");
    }
    assert!(!before.iter().any(|r| r.split_whitespace().nth(4)==Some("checkpoint")),
            "an unfinished checkpoint must not claim completion");
    STOP.store(false, std::sync::atomic::Ordering::Relaxed);
    wm_checkpoint(&mut LivePointer(&mut session),[143,144],[1,2],1,[1280,800],1,["",""]).unwrap();
    let after=rows();
    for phase in ["screen-capture", "screen-read-hash", "screen-compare", "checkpoint"] {
        assert!(after.iter().any(|r| r.split_whitespace().nth(4)==Some(phase)), "missing {phase}: {after:?}");
    }
    // A busy snapshot loop must never turn reporting into an unbounded export.
    for _ in 0..1000 {
        wm_checkpoint(&mut LivePointer(&mut session),[143,144],[1,2],1,[1280,800],1,["",""]).unwrap();
    }
    let bytes=OUTPUT.lock().unwrap();
    assert!(bytes.len()<=8192, "{} bytes exceeds the per-scenario report bound", bytes.len());
    assert!(bytes.ends_with(b"\n"), "records must be complete writes");
}
'''.replace('SCOPE', scope)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'check.rs').write_text(source)
            build = subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings',
                                    str(path / 'check.rs'), '-o', str(path / 'check')],
                                   capture_output=True, text=True)
            self.assertEqual(build.returncode, 0, build.stderr)
            result = subprocess.run([str(path / 'check')], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == '__main__':
    unittest.main()
