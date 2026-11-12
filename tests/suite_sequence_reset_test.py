"""Exercise the production sequence's per-path reset comparison."""
from pathlib import Path
import subprocess
import tempfile
import unittest

from suite_sequence_roles_test import ROOT, STUBS

CHECKS = r'''
        #[test] fn records_appends_and_changes_separately() {
            let mut reset = Reset { removed: 0, changed: Vec::new(), appended: Vec::new() };
            assert_eq!(reset.record("time"), "SUITE_SEQUENCE RESET time removed=0 changed=0 appended=0");
            reset.appended.push(String::from("/var/log/auth.log"));
            assert_eq!(reset.record("time"), "SUITE_SEQUENCE RESET time removed=0 changed=0 appended=1 logs=/var/log/auth.log");
            reset.changed.push(String::from("/etc/hosts"));
            assert_eq!(reset.record("time"), "SUITE_SEQUENCE RESET time removed=0 changed=1 paths=/etc/hosts appended=1 logs=/var/log/auth.log");
        }

fn fixture_path(path: &str) -> std::path::PathBuf {
    match path {
        "/var/log/auth.log" | "/etc/hosts" | "/var/logs/x" =>
            std::path::PathBuf::from(std::env::var_os("SEQUENCE_FIXTURE").unwrap()),
        _ => std::path::PathBuf::from(path),
    }
}

        #[test] fn classifies_only_unchanged_log_prefix_appends() {
            use std::os::unix::fs::{symlink, PermissionsExt};
            let root = std::env::temp_dir().join(format!("sequence-append-{}", std::process::id()));
            std::fs::create_dir(&root).unwrap();
            let file = root.join("log");
            std::env::set_var("SEQUENCE_FIXTURE", &file);
            std::fs::write(&file, b"").unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
            let meta = std::fs::symlink_metadata(&file).unwrap();
            // The captured baseline is root:adm; host metadata supplies its own owners.
            let mut recorded = Entry { kind: 'f', mode: 0o640, uid: 0, gid: 4, size: 0, stamp: 0xcbf29ce484222325 };
            recorded.uid = meta.uid();
            recorded.gid = meta.gid();
            std::fs::write(&file, vec![b'a'; 844]).unwrap();
            let meta = std::fs::symlink_metadata(&file).unwrap();
            assert!(matches!(compare("/var/log/auth.log", &recorded, Some(&meta)), Comparison::Appended));
            assert!(matches!(compare("/etc/hosts", &recorded, Some(&meta)), Comparison::Changed));
            assert!(matches!(compare("/var/logs/x", &recorded, Some(&meta)), Comparison::Changed));
            let mut different = Entry { ..recorded };
            different.uid ^= 1;
            assert!(matches!(compare("/var/log/auth.log", &different, Some(&meta)), Comparison::Changed));
            different.uid ^= 1;
            different.gid ^= 1;
            assert!(matches!(compare("/var/log/auth.log", &different, Some(&meta)), Comparison::Changed));
            different.gid ^= 1;
            different.mode ^= 0o100;
            assert!(matches!(compare("/var/log/auth.log", &different, Some(&meta)), Comparison::Changed));

            std::fs::write(&file, b"prefix").unwrap();
            let original = describe(&file, &std::fs::symlink_metadata(&file).unwrap());
            for contents in [&b"pre"[..], &b"PREFIX"[..], &b"PREFIX plus"[..]] {
                std::fs::write(&file, contents).unwrap();
                let meta = std::fs::symlink_metadata(&file).unwrap();
                assert!(matches!(compare("/var/log/auth.log", &original, Some(&meta)), Comparison::Changed));
            }
            std::fs::write(&file, b"prefix plus").unwrap();
            let meta = std::fs::symlink_metadata(&file).unwrap();
            assert!(matches!(compare("/var/log/auth.log", &original, Some(&meta)), Comparison::Appended));
            std::fs::remove_file(&file).unwrap();
            symlink("missing", &file).unwrap();
            let meta = std::fs::symlink_metadata(&file).unwrap();
            assert!(matches!(compare("/var/log/auth.log", &original, Some(&meta)), Comparison::Changed));
            std::fs::remove_file(&file).unwrap();
            std::fs::remove_dir(root).unwrap();
        }

        #[test] fn compares_files_and_metadata() {
            use std::os::unix::fs::PermissionsExt;
            let root = std::env::temp_dir().join(format!("sequence-reset-{}", std::process::id()));
            std::fs::create_dir(&root).unwrap();
            let path = root.join("file");
            std::fs::write(&path, b"first").unwrap();
            let name = path.to_str().unwrap();
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            let mut recorded = describe(&path, &metadata);
            assert!(matches!(compare(name, &recorded, Some(&metadata)), Comparison::Same));

            std::fs::write(&path, b"other").unwrap();
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            assert!(matches!(compare(name, &recorded, Some(&metadata)), Comparison::Changed));
            std::fs::write(&path, b"first").unwrap();

            let original_mode = recorded.mode;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(original_mode ^ 0o100)).unwrap();
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            assert!(matches!(compare(name, &recorded, Some(&metadata)), Comparison::Changed));
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(original_mode)).unwrap();

            let metadata = std::fs::symlink_metadata(&path).unwrap();
            recorded.gid ^= 1;
            assert!(matches!(compare(name, &recorded, Some(&metadata)), Comparison::Changed));
            recorded.gid ^= 1;
            std::fs::remove_file(&path).unwrap();
            assert!(matches!(compare(name, &recorded, None), Comparison::Changed));
            std::fs::remove_dir(root).unwrap();
        }
    }
}
'''


class SequenceReset(unittest.TestCase):
    def test_reset_stops_at_mount_boundary(self):
        source = (ROOT / 'libs/libbreenix/src/suite_sequence.rs').read_text()
        source = '\n'.join(line for line in source.splitlines() if not line.startswith('//!'))
        # Adapt the old root-only API so the same production walker can run on a fixture.
        if 'fn reset()' in source:
            source = source.replace('fn reset()', 'fn reset(root: &str)')
            source = source.replace('fn record_baseline()', 'fn record_baseline(root: &str)')
            source = source.replace('record_baseline();', 'record_baseline("/");')
            source = source.replace('match reset() {', 'match reset("/") {')
            source = source.replace('fn walk(visit:', 'fn walk(root: &str, visit:')
            source = source.replace('for root in ["/", "/dev/shm", "/dev/mqueue"]', 'for root in [root]')
            source = source.replace('walk(&mut |path, meta|', 'walk(root, &mut |path, meta|')
        checks = r"""
        #[test] fn mount_is_never_reset() {
            let root = std::env::var("SEQUENCE_ROOT").unwrap();
            let mount = format!("{root}/mount");
            let baseline = std::env::var("SEQUENCE_BASELINE").unwrap();
            std::fs::write(&baseline, std::env::var("SEQUENCE_RECORDED").unwrap()).unwrap();
            let result = reset(&root).unwrap();
            assert!(std::path::Path::new(&format!("{mount}/keep")).exists(), "reset deleted a mounted file");
            assert!(result.changed.contains(&format!("{mount}(mounted)")));
            record_baseline(&root);
            let entries = load_baseline().unwrap();
            assert_eq!(entries[&mount].kind, 'm');
            assert!(!entries.contains_key(&format!("{mount}/keep")));
            let result = reset(&root).unwrap();
            assert!(result.changed.is_empty(), "baseline mount was reported changed: {:?}", result.changed);
            assert!(std::path::Path::new(&format!("{mount}/keep")).exists());
        }
    }
}
"""
        with tempfile.TemporaryDirectory() as scratch:
            scratch = Path(scratch)
            root = scratch / 'root'
            mount = root / 'mount'
            mount.mkdir(parents=True)
            baseline_dir = scratch / 'state'
            baseline_dir.mkdir()
            # Capture a normal directory before the foreign filesystem is mounted.
            meta = mount.stat()
            recorded = f'd {meta.st_mode & 0o7777:o} {meta.st_uid} {meta.st_gid} 0 0 {mount}\n'
            attached = subprocess.run(['hdiutil', 'attach', '-nomount', 'ram://16384'], check=True, capture_output=True, text=True)
            device = attached.stdout.split()[0]
            try:
                subprocess.run(['diskutil', 'eraseVolume', 'HFS+', 'SequenceFixture', device], check=True, capture_output=True)
                subprocess.run(['diskutil', 'unmount', device], check=True, capture_output=True)
                subprocess.run(['diskutil', 'mount', '-mountPoint', str(mount), device], check=True, capture_output=True)
                self.assertNotEqual(root.stat().st_dev, mount.stat().st_dev)
                (mount / 'keep').write_text('mounted data')
                source_path, binary = scratch / 'test.rs', scratch / 'test'
                source_path.write_text(STUBS + source + checks)
                compiled = subprocess.run(['rustc', '--edition=2021', '-Dwarnings', '--test', str(source_path), '-o', str(binary)], capture_output=True, text=True)
                self.assertEqual(compiled.returncode, 0, compiled.stderr)
                import os
                env = dict(os.environ, BREENIX_SUITE_SEQUENCE_DIR=str(baseline_dir), SEQUENCE_ROOT=str(root),
                           SEQUENCE_BASELINE=str(baseline_dir / 'suite-baseline'), SEQUENCE_RECORDED=recorded)
                result = subprocess.run([str(binary), '--exact', 'harness::sequence::mount_is_never_reset'], env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            finally:
                subprocess.run(['hdiutil', 'detach', device], check=True, capture_output=True)

    def test_production_comparison(self):
        source = (ROOT / 'libs/libbreenix/src/suite_sequence.rs').read_text()
        source = '\n'.join(line for line in source.splitlines() if not line.startswith('//!'))
        # Supply a temporary file for guest-absolute paths without changing classification.
        start = source.index('fn compare(')
        end = source.index('\n}', start) + 2
        comparison = source[start:end].replace('Path::new(path)', 'fixture_path(path).as_path()')
        source = source[:start] + comparison + source[end:]
        # Let the old implementation compile so the new assertion fails on its result.
        source = source.replace('enum Comparison { Same, Changed }', 'enum Comparison { Same, Changed, Appended }')
        # Give the old Reset storage for the new input, without changing its formatter.
        source = source.replace('    changed: Vec<String>,\n}', '    changed: Vec<String>,\n    appended: Vec<String>,\n}')
        source = source.replace('Reset { removed: 0, changed: Vec::new() }', 'Reset { removed: 0, changed: Vec::new(), appended: Vec::new() }')
        with tempfile.TemporaryDirectory() as scratch:
            source_path = Path(scratch) / 'test.rs'
            binary = Path(scratch) / 'test'
            source_path.write_text(STUBS + source + CHECKS)
            result = subprocess.run(['rustc', '--edition=2021', '-Dwarnings', '--test', str(source_path), '-o', str(binary)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            result = subprocess.run([str(binary)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == '__main__':
    unittest.main()
