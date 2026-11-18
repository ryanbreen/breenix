"""E24 coordinator 567: guest lower bound and failure-only marker receipts."""
from pathlib import Path
import unittest
from ui_idle_capture_test import function, run_rust

ROOT = Path(__file__).resolve().parents[1]


class FreeCountDifferential(unittest.TestCase):
    def test_guest_uses_extent_backed_lower_bound_and_keeps_ext2(self):
        source = (ROOT / 'userspace/programs/src/suite_files_io.rs').read_text()
        case = source.split('fn descriptors_unlink_reclaim_breenfs()', 1)[1].split('\nfn descriptors_unlink_reclaim()', 1)[0]
        self.assertNotIn('control_delta', case)
        self.assertIn('after.checked_sub(before).is_some_and(|gain| gain >= data_blocks)', case)
        self.assertIn('expect_errno(stat(&f.path, false), 2,', case)
        self.assertIn('.ino() != ino', case)
        self.assertNotIn('fs::unlink(&control.path)', case)

    def test_marker_calls_bracket_the_unchanged_commit(self):
        source = (ROOT / 'userspace/programs/src/suite_files_io.rs').read_text()
        marker = source.split('impl RootMarker {', 1)[1].split('\nfn metadata_ftruncate_large()', 1)[0]
        before = marker.index('markers::sample_root(index, false)')
        commit = marker.index('sync_fd(self.0.fd(), false)?;')
        after = marker.index('markers::sample_root(index, true)')
        self.assertLess(before, commit)
        self.assertLess(commit, after)
        for case in ('descriptors/unlink-reclaim', 'metadata/ftruncate-large', 'metadata/ftruncate-large-open'):
            self.assertIn(case, marker)
        unlink = source.split('fn descriptors_unlink_reclaim_breenfs()', 1)[1].split('\nfn descriptors_unlink_reclaim()', 1)[0]
        self.assertLess(unlink.index('markers::sample_root(index, false)'), unlink.index('sync_fd(marker.0.fd(), false)?;'))
        self.assertLess(unlink.index('sync_fd(marker.0.fd(), false)?;'), unlink.index('markers::sample_root(index, true)'))
        report = (ROOT / 'kernel/src/fs/breenfs/vfs.rs').read_text()
        self.assertIn('free_blocks={} pending_blocks={}', report)

    def test_files_io_cases_get_the_fork_shared_scope(self):
        source = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        scope = source.split('    let controls = if ', 1)[1].split('control::Scope::new()', 1)[0]
        self.assertIn('suite == "files-io"', scope)
        for case in ('descriptors/unlink-reclaim', 'metadata/ftruncate-large', 'metadata/ftruncate-large-open'):
            self.assertIn(case, scope)

    def test_raw_marker_parser_selects_only_the_root(self):
        parse = function('libs/libbreenix/src/suite_markers.rs', 'fn parse(')
        result = run_rust(parse + r'''
fn main() {
    let root = "root breenfs durable_generation=19 dirty_bytes=0 free_blocks=0 pending_blocks=42";
    let text = format!("home breenfs durable_generation=20 free_blocks=3 pending_blocks=4\n{root}\nmutation_cost mount=1 op=write");
    assert_eq!(parse(&text), Some((19, 0, 42)));
    assert_eq!(parse("root ext2 free_blocks=3 pending_blocks=4"), None);
    assert_eq!(parse("root breenfs durable_generation=19 free_blocks=3"), None);
    assert_eq!(parse(&format!("{root} free_blocks=8")), None);
    assert_eq!(parse(&format!("{root}\n{root}")), None);
}
''')
        self.assertEqual(result.returncode, 0, result.stderr)

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
        control::markers::sample(i, false, 100+i as u64, 16200+i as u64, 40);
        control::markers::sample(i, true, 101+i as u64, 16201+i as u64, 39);
    }
    assert!(scope.failure_records("files-io","metadata/ftruncate-large",false).is_empty());
    assert!(scope.failure_records("files-io","open/creat",true).is_empty());
    let rows = scope.failure_records("files-io","metadata/ftruncate-large",true);
    assert_eq!(rows.len(),12,"{rows:?}");
    assert!(rows[0].contains("marker=0 before_free=16200 before_pending=40 after_free=16201 after_pending=39"));
    assert!(rows[11].contains("marker=11 before_free=16211"));
    assert_eq!(scope.failure_records("files-io","metadata/ftruncate-large-open",true).len(),12);
    assert!(rows[0].contains("before_generation=100 after_generation=101"));
    assert_eq!(scope.failure_records("files-io","descriptors/unlink-reclaim",true).len(),12);
    control::markers::sample(12, false, 300, 1, 2);
    assert_eq!(scope.failure_records("files-io","metadata/ftruncate-large",true).len(),12);
    drop(scope);
    let scope = control::Scope::new().unwrap();
    extern "C" { fn fork()->i32; fn waitpid(pid:i32,status:*mut i32,flags:i32)->i32; fn _exit(status:i32)->!; }
    let child = unsafe { fork() };
    assert!(child >= 0);
    if child == 0 {
        control::markers::sample(0, false, 20, 7, 8);
        unsafe { _exit(0) };
    }
    let mut status = -1;
    assert_eq!(unsafe { waitpid(child, &mut status, 0) }, child);
    assert_eq!(status, 0);
    let rows=scope.failure_records("files-io","metadata/ftruncate-large",true);
    assert_eq!(rows.len(),1);
    assert!(rows[0].contains("before_free=7 before_pending=8"));
    assert!(rows[0].contains("after_free=unreached after_pending=unreached"));
}
'''.replace('CONTROL', str(ROOT / 'libs/libbreenix/src/suite_control.rs'))
        result = run_rust(source)
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
