"""Keep unrelated kernel functions out of the host statfs fixture."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class StatfsFixtureTest(unittest.TestCase):
    def test_appended_kernel_function_is_not_extracted(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            work = root / 'tools/vfs-host-tests'
            work.mkdir(parents=True)
            source = root / 'kernel/src/syscall/fs.rs'
            source.parent.mkdir(parents=True)
            source.write_text((ROOT / 'kernel/src/syscall/fs.rs').read_text() +
                              '\nfn unrelated_kernel_only() { missing_kernel_api(); }\n')
            binary = root / 'build-fixture'
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021',
                            str(ROOT / 'tools/vfs-host-tests/build.rs'), '-o', str(binary)],
                           check=True)
            subprocess.run([str(binary)], cwd=work,
                           env={**os.environ, 'OUT_DIR': str(root)}, check=True,
                           capture_output=True, text=True)
            fixture = (root / 'statfs_dispatch.rs').read_text()
            self.assertIn('fn user_statfs(', fixture)
            self.assertNotIn('unrelated_kernel_only', fixture)


if __name__ == '__main__':
    unittest.main()
