"""E24 coordinator 558(y): guest differential and failure-only marker receipts."""
from pathlib import Path
import unittest
from ui_idle_capture_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class FreeCountDifferential(unittest.TestCase):
    def test_guest_uses_control_and_keeps_ext2(self):
        source = (ROOT / 'userspace/programs/src/suite_files_io.rs').read_text()
        case = source.split('fn descriptors_unlink_reclaim_breenfs()', 1)[1].split('\nfn descriptors_unlink_reclaim()', 1)[0]
        self.assertIn('let mut control = orphan_fixture()?;', case)
        self.assertIn('unlink_delta - control_delta == i128::from(data_blocks)', case)
        self.assertIn('expect_errno(stat(&f.path, false), 2,', case)
        self.assertIn('.ino() != ino', case)
        self.assertNotIn('fs::unlink(&control.path)', case)

    def test_marker_samples_survive_failure_but_emit_nothing_on_success(self):
        source = r'''#![allow(dead_code)]
extern crate self as libbreenix;
pub mod time {
    pub struct Stamp;
    impl Stamp { pub fn as_nanos(&self)->i128 { 1 } }
    pub fn now_monotonic()->Result<Stamp,String> { Ok(Stamp) }
}
#[path="CONTROL"] mod control;
fn main() {
    let scope = control::Scope::new().unwrap();
    for i in 0..12 {
        control::markers::sample(i, false, 16200+i as u64, 40);
        control::markers::sample(i, true, 16201+i as u64, 39);
    }
    assert!(scope.failure_records("files-io","metadata/ftruncate-large",false).is_empty());
    assert!(scope.failure_records("files-io","open/creat",true).is_empty());
    let rows = scope.failure_records("files-io","metadata/ftruncate-large",true);
    assert_eq!(rows.len(),12,"{rows:?}");
    assert!(rows[0].contains("marker=0 before_free=16200 before_pending=40 after_free=16201 after_pending=39"));
    assert!(rows[11].contains("marker=11 before_free=16211"));
    assert_eq!(scope.failure_records("files-io","metadata/ftruncate-large-open",true).len(),12);
    drop(scope);
    let scope = control::Scope::new().unwrap();
    control::markers::sample(0, false, 7, 8);
    let rows=scope.failure_records("files-io","metadata/ftruncate-large",true);
    assert_eq!(rows.len(),1);
    assert!(rows[0].contains("after_free=unreached after_pending=unreached"));
}
'''.replace('CONTROL', str(ROOT / 'libs/libbreenix/src/suite_control.rs'))
        result = run_rust(source)
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
