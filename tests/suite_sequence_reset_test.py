"""Exercise the production sequence's per-path reset comparison."""
from pathlib import Path
import subprocess
import tempfile
import unittest

from suite_sequence_roles_test import ROOT, STUBS

CHECKS = r'''
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
