"""E24 520(s): production observations survive failure and stay off success serial."""
from pathlib import Path
import subprocess
import tempfile
import unittest
from ui_idle_capture_test import run_rust, function

ROOT = Path(__file__).resolve().parents[1]


class PrelatchObservations(unittest.TestCase):
    def test_failure_only_bounded_records_survive_a_killed_case(self):
        source = r'''#![allow(dead_code)]
extern crate self as libbreenix;
use std::sync::atomic::{AtomicU64, Ordering};
static NOW: AtomicU64 = AtomicU64::new(100);
pub mod time {
    pub struct Stamp(u64);
    impl Stamp { pub fn as_nanos(&self) -> i128 { i128::from(self.0) * 1_000_000 } }
    pub fn now_monotonic() -> Result<Stamp, String> { Ok(Stamp(super::NOW.load(super::Ordering::Relaxed))) }
}
#[path="CONTROL"] mod control;
extern "C" { fn fork()->i32; fn waitpid(p:i32,s:*mut i32,f:i32)->i32;
    fn kill(p:i32,s:i32)->i32; fn getpid()->i32; fn _exit(s:i32)->!; }
fn main() {
    let scope = control::Scope::new().unwrap();
    let pid = unsafe { fork() }; assert!(pid >= 0);
    if pid == 0 {
        let span = control::observations::Span::new("configure-fixtures");
        NOW.store(125, Ordering::Relaxed); drop(span);
        for _ in 0..1000 {
            let span = control::observations::Span::new("window-registry");
            NOW.store(130, Ordering::Relaxed); drop(span);
        }
        control::observations::stamp("window-registry-match", 127_000_000);
        let _inflight = control::observations::Span::new("discard-root-delete");
        assert_eq!(unsafe { kill(getpid(), 9) }, 0);
        unsafe { _exit(99) }
    }
    let mut status=0; assert_eq!(unsafe {waitpid(pid,&mut status,0)},pid);
    assert_eq!(status & 0x7f,9);
    assert!(scope.failure_records("ui-testing","tree/names-values",false).is_empty());
    let lines=scope.failure_records("ui-testing","tree/names-values",true);
    assert_eq!(lines.len(),4,"{lines:?}");
    assert!(lines.iter().all(|s|s.len()<320 && s.starts_with("CASE_OBSERVATION ui-testing tree/names-values ")));
    assert!(lines.iter().any(|s|s.contains("op=configure-fixtures started=1 completed=1 total_ns=25000000")),"{lines:?}");
    assert!(lines.iter().any(|s|s.contains("op=window-registry started=1000 completed=1000")),"{lines:?}");
    assert!(lines.iter().any(|s|s.contains("op=discard-root-delete started=1 completed=0")),"{lines:?}");
    assert!(lines.iter().any(|s|s.contains("op=window-registry-match") && s.contains("first_ns=127000000")),"{lines:?}");
    assert!(scope.failure_records("other","case",true).is_empty());
    drop(scope);
    let scope=control::Scope::new().unwrap();
    let wire=b"OBS readdir-setup-done 101\nOBS readdir-opendir-reached 102\nOBS readdir-enumeration-start 103\nOBS readdir-enumeration-end 104\nPASS\n";
    for end in 0..=wire.len() { control::observations::libc_phases(&wire[..end]); }
    assert!(scope.failure_records("libc","dirent-glob/readdir-many",false).is_empty());
    let lines=scope.failure_records("libc","dirent-glob/readdir-many",true);
    assert_eq!(lines.len(),4,"{lines:?}");
    assert!(lines.iter().any(|s|s.contains("op=readdir-enumeration-end") && s.contains("first_ns=104")),"{lines:?}");
    assert!(lines.iter().all(|s|s.contains("started=1 completed=1")),"fragment replay must not duplicate stamps: {lines:?}");
}
'''.replace('CONTROL', str(ROOT / 'libs/libbreenix/src/suite_control.rs'))
        result = run_rust(source)
        self.assertEqual(result.returncode, 0, result.stderr)
        runner = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        self.assertIn('controls.failure_records(suite, name, matches!(outcome, Outcome::Fail { .. }))', runner)
        self.assertIn('suite == "libc" && name == "dirent-glob/readdir-many"', runner)
        session = (ROOT / 'libs/libuitest/src/session.rs').read_text()
        supervisor = (ROOT / 'libs/libuitest/src/supervisor_runtime.rs').read_text()
        for name in ['window-registry', 'window-composites', 'window-registry-match', 'window-timed-composite']:
            self.assertIn('"' + name + '"', session)
        for name in ['configure-parent', 'configure-fixtures', 'configure-verify', 'configure-trace', 'configure-arena',
                     'discard-outputs', 'discard-children', 'discard-devices', 'discard-display',
                     'discard-root-delete', 'discard-remnants', 'discard-reply']:
            self.assertIn('"' + name + '"', supervisor)
        libc = (ROOT / 'userspace/programs/src/suite_libc.rs').read_text()
        self.assertIn('observations::libc_phases(buf)', libc)

    def test_c_guest_marks_real_readdir_phases_without_changing_result(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            # Exercise the actual case and harness, substituting only the case table.
            source = root / 'probe.c'
            source.write_text('#include "case.h"\n#include <dirent.h>\n#include <unistd.h>\n'
                              'static int make(const char *name) { return h_put_file(name, "x"); }\n'
                              + function('userspace/libc-suite/dirent_glob.c', 'static const char *readdir_many(')
                              + '\nconst struct libc_case libc_cases[] = {{"readdir-many", readdir_many}, {0, 0}};\n')
            build = subprocess.run(['taskpolicy', '-b', 'cc', '-std=c17', '-D_DEFAULT_SOURCE',
                                    '-Wall', '-Wextra', '-Werror', '-Wno-unused-function',
                                    '-I', str(ROOT / 'userspace/libc-suite'), str(source),
                                    str(ROOT / 'userspace/libc-suite/harness.c'), '-lm', '-o', str(root / 'probe')],
                                   capture_output=True, text=True)
            self.assertEqual(build.returncode, 0, build.stderr)
            case_dir = root / 'case'
            case_dir.mkdir()
            result = subprocess.run([str(root / 'probe'), 'readdir-many', '1'], cwd=case_dir,
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            lines = result.stdout.splitlines()
            self.assertEqual(lines[-1], 'PASS')
            self.assertEqual([line.split()[1] for line in lines[:-1]], [
                'readdir-setup-done', 'readdir-opendir-reached',
                'readdir-enumeration-start', 'readdir-enumeration-end'])
            self.assertTrue(all(line.startswith('OBS ') and int(line.split()[2]) > 0 for line in lines[:-1]))


if __name__ == '__main__':
    unittest.main()
