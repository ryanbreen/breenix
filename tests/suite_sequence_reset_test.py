"""Exercise the production sequence's per-path reset comparison."""
from pathlib import Path
import subprocess
import tempfile
import unittest

from suite_sequence_roles_test import ROOT, STUBS

CHECKS = r'''
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
