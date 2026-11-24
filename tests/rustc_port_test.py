import datetime
import json
import subprocess
import tomllib
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
    def test_bootstrap_hosted_host_recorded(self):
        record_path = RECORDS / '2.json'
        self.assertTrue(record_path.is_file(), 'X0.2 bootstrap result has not been recorded')
        record = json.loads(record_path.read_text())
        target_name = 'x86_64-breenix-hosted.probe.json'
        hosted = json.loads((RECORDS / target_name).read_text())
        base = json.loads((ROOT / 'x86_64-breenix-c.json').read_text())
        self.assertEqual(hosted, {**base, 'panic-strategy': 'unwind'})

        scratch = Path(record['environment']['RUST_TARGET_PATH'])
        self.assertTrue(scratch.is_absolute())
        self.assertEqual(record['command'], [
            'taskpolicy', '-b', 'nice', '-n', '15',
            'python3', 'x.py', 'build', '--stage', '1',
            '--config', str(scratch / 'bootstrap.toml'),
        ])
        self.assertEqual(record['timeout_seconds'], 40 * 60)
        self.assertEqual(record['environment']['CARGO_BUILD_JOBS'], '4')
        config = tomllib.loads(record['bootstrap_toml'])
        for key in ('host', 'target'):
            self.assertEqual(config['build'][key], [str(scratch / target_name)])
        self.assertEqual(config['build']['build-dir'], str(scratch / 'build'))
        self.assertEqual(record['probe_tree_commit'], PIN)
        self.assertEqual(record['probe_tree_status_before'], '')
        self.assertEqual(record['probe_tree_status'], '')
        self.assertTrue(record['last_completed_step'].strip())
        self.assertTrue(record['prediction'].strip())
        self.assertLess(datetime.datetime.fromisoformat(record['prediction_time']),
                        datetime.datetime.fromisoformat(record['time']))
        self.assertIn(record['verdict'], {'route-holds', 'route-fails', 'incomplete'})
        if record['timed_out']:
            self.assertEqual(record['verdict'], 'incomplete')
        if record['verdict'] != 'route-holds':
            self.assertTrue(record['first_error'].strip())

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
