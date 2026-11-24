import datetime
import json
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RECORDS = ROOT / 'toolchain/rust-host/x0'
TREE = Path.home() / 'fun/code/rust.worktrees/x0-666252fd'
BASE = '706f244db'
PIN = '666252fd61839b9e9b1bc9e777a0c84c5cf3dc6d'


def git(*args):
    return subprocess.run(['git', '-C', str(TREE), *args], text=True,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True).stdout.strip()


class RustcPort(unittest.TestCase):
    def test_fork_compiler_tree_is_upstream(self):
        record = json.loads((RECORDS / '1.json').read_text())
        self.assertEqual(git('rev-parse', 'HEAD'), PIN)
        self.assertEqual(git('status', '--porcelain', '--', 'compiler', 'src/llvm-project'), '')
        expected = {}
        for ref in (BASE, PIN):
            expected[ref] = {
                'compiler': git('rev-parse', ref + ':compiler'),
                'llvm_project': git('ls-tree', ref, 'src/llvm-project').split()[2],
            }
        diff = subprocess.run(['git', '-C', str(TREE), 'diff', '--quiet', BASE, PIN,
                               '--', 'compiler', 'src/llvm-project']).returncode
        self.assertIn(diff, (0, 1))
        self.assertEqual(record['output'], {'trees': expected, 'diff_status': diff})
        self.assertEqual(record['probe_tree_status'], '')
        self.assertEqual(record['verdict'], 'same' if diff == 0 else 'different')

    def test_licenses_recorded_before_use(self):
        licenses = json.loads((RECORDS / 'licenses.json').read_text())
        expected = {'rustc', 'cargo', 'LLVM', 'libc++', 'libc++abi', 'libunwind', 'compiler-rt'}
        self.assertEqual(set(licenses), expected)
        records = list(RECORDS.glob('[0-9]*.json'))
        self.assertTrue(records)
        for component, entry in licenses.items():
            self.assertTrue(entry['spdx'], component)
            self.assertTrue(entry['license_file'], component)
            recorded = datetime.datetime.fromisoformat(entry['recorded_at'])
            for path in records:
                probe = json.loads(path.read_text())
                self.assertLess(recorded, datetime.datetime.fromisoformat(probe['time']), component)


if __name__ == '__main__':
    unittest.main()
