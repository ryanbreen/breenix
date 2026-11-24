import datetime
import json
import subprocess
import sys
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
    def test_libcxx_runtime_inventory_recorded(self):
        record = self.x0_record(5)
        self.assertRegex(record['llvm_project'], r'^[0-9a-f]{40}$')
        llvm_project = git('ls-tree', PIN, 'src/llvm-project').split()[2]
        self.assertEqual(record['llvm_project'], llvm_project)
        configure = record['configure']
        for arg in (
            '<llvm-project>/runtimes', '-DLLVM_ENABLE_RUNTIMES=libcxx;libcxxabi;libunwind',
            '-DLIBCXX_CXX_ABI=libcxxabi', '-DLIBCXX_ENABLE_LOCALIZATION=OFF',
            '-DLIBCXX_ENABLE_THREADS=ON', '-DTHREADS_PREFER_PTHREAD_FLAG=ON',
            '-DCMAKE_CXX_FLAGS=-nostdlibinc -isystem <repo>/toolchain/include',
        ):
            self.assertIn(arg, configure['command'])
        build = record['build']
        for stage in (configure, build) if build is not None else (configure,):
            self.assertRegex(stage['log_sha256'], r'^[0-9a-f]{64}$')
            self.assertIs(type(stage['exit_status']), int)
        if configure['exit_status'] != 0:
            self.assertTrue(configure['first_error'].strip())
            self.assertIsNone(build)
        else:
            self.assertIsNone(configure['first_error'])
            self.assertIn('--build', build['command'])
            if build['exit_status'] != 0:
                self.assertTrue(build['first_error'].strip())
            else:
                self.assertIsNone(build['first_error'])
                inventory = record['inventory']
                undefined = inventory['undefined_by_archive']
                self.assertEqual(set(undefined), {'libc++.a', 'libc++abi.a', 'libunwind.a'})
                definitions = inventory['defined_by_provider']
                self.assertEqual(set(definitions), {'libc.a', 'compiler_builtins'})
                self.assertTrue(all(definitions.values()))
                for symbols in (*undefined.values(), *definitions.values()):
                    self.assertEqual(symbols, sorted(set(symbols)))
                self.assertEqual(inventory['remaining_undefined'], sorted(
                    set().union(*undefined.values()) - set().union(*definitions.values())))
                return
        self.assertIsNone(record['inventory'])

    def test_static_and_panic_answers_cited(self):
        for number, question in ((6, 'static-rustc'), (7, 'panic-abort-host')):
            with self.subTest(probe=number):
                record = self.x0_record(number)
                self.assertEqual(record['question'], question)
                self.assertIn(record['verdict'], ('documented-yes', 'documented-no', 'undocumented'))
                self.assertTrue(record['assessment'].strip())
                self.assertTrue(record['citations'])
                for citation in record['citations']:
                    path, line = citation['location'].rsplit(':', 1)
                    self.assertTrue(path == 'bootstrap.example.toml' or path.startswith('src/doc/'))
                    self.assertGreater(int(line), 0)
                    documented = git('show', f'{PIN}:{path}').splitlines()[int(line) - 1]
                    self.assertTrue(citation['text'].strip())
                    self.assertEqual(citation['text'], documented.strip())

    def test_aarch64_float_passing_recorded(self):
        record = self.x0_record(8)
        measured = subprocess.run(
            [sys.executable, str(ROOT / 'scripts/rust-host/x0.py'), 'measure-float'],
            cwd=ROOT, capture_output=True, text=True, timeout=60,
        )
        self.assertEqual(measured.returncode, 0, measured.stderr)
        measurement = json.loads(measured.stdout)
        self.assertEqual(record['measurement'], measurement)
        self.assertIn(measurement['verdict'], {'vector-registers', 'integer-registers'})
        self.assertTrue(measurement['instructions'])

    def x0_record(self, number):
        path = RECORDS / f'{number}.json'
        self.assertTrue(path.is_file(), f'X0.{number} result has not been recorded')
        record = json.loads(path.read_text())
        self.assertEqual(record['probe_tree_commit'], PIN)
        self.assertTrue(record['prediction'].strip())
        self.assertLess(datetime.datetime.fromisoformat(record['prediction_time']),
                        datetime.datetime.fromisoformat(record['time']))
        return record

    def test_linux_profile_per_crate(self):
        path = RECORDS / '9.json'
        self.assertTrue(path.is_file(), 'X0.9 Linux rustc profile has not been recorded')
        record = json.loads(path.read_text())
        self.assertEqual(record['executor'], 'linux-build-container')
        self.assertEqual(record['target'], 'x86_64-breenix')
        self.assertEqual(record['profile'], 'selfhost')
        self.assertEqual(record['toolchain'], 'nightly-2025-06-24')
        self.assertTrue(record['prediction'].strip())
        self.assertLess(datetime.datetime.fromisoformat(record['prediction_time']),
                        datetime.datetime.fromisoformat(record['time']))
        self.assertRegex(record['workspace_commit'], r'^[0-9a-f]{40}$')
        self.assertIn('--profile', record['command'])
        self.assertIn('selfhost', record['command'])
        self.assertTrue(record['rustc_wrapper'].endswith('/x0-rustc-probe.sh'))

        crates = record['crates']
        self.assertEqual(set(crates), {'bobj', 'bas', 'bld', 'bar', 'bnm', 'bcc', 'bmake'})
        self.assertIn('std', record['std_information_only'])
        self.assertIn('core', record['std_information_only'])
        self.assertFalse(set(crates) & set(record['std_information_only']))
        for name, units in crates.items():
            with self.subTest(crate=name):
                self.assertTrue(units)
                self.assertIn('lib', {unit['kind'] for unit in units})
                if name != 'bobj':
                    self.assertIn('bin', {unit['kind'] for unit in units})
                for unit in units:
                    for field in ('peak_rss_kib', 'peak_threads', 'largest_anonymous_mapping_bytes',
                                  'largest_file_mapping_bytes', 'scratch_bytes'):
                        self.assertIs(type(unit[field]), int, field)
                        self.assertGreater(unit[field], 0, field)
                    self.assertGreater(unit['wall_seconds'], 0)
                    syscalls = unit['syscall_families']
                    self.assertTrue(syscalls['memory'])
                    self.assertTrue(syscalls['process_and_threads'])
                    self.assertTrue(syscalls['file_metadata'])
                    self.assertEqual(set(syscalls), {'memory', 'limits', 'process_and_threads',
                                                     'pipes', 'wait', 'locks', 'file_metadata'})
                    limit = unit['min_stack_kib']
                    self.assertTrue(64 <= limit <= 65536)
                    trials = unit['stack_exit_status']
                    self.assertEqual(trials['65536'], 0)
                    self.assertEqual(trials[str(limit)], 0)
                    if limit > 64:
                        self.assertNotEqual(trials[str(limit - 1)], 0)

        for target in ('x86_64-breenix', 'aarch64-breenix'):
            digests = record['release_parity'][target]
            self.assertRegex(digests['parent_commit'], r'^[0-9a-f]{40}$')
            self.assertEqual(digests['parent_sha256'], digests['profile_sha256'])
            self.assertEqual(set(digests['parent_sha256']), set(crates))
            for digest in digests['parent_sha256'].values():
                self.assertRegex(digest, r'^[0-9a-f]{64}$')

    def test_llvm_config_route_recorded(self):
        path = RECORDS / '3.json'
        self.assertTrue(path.is_file(), 'X0.3 LLVM configuration result has not been recorded')
        record = json.loads(path.read_text())
        base = json.loads((RECORDS / '2.json').read_text())
        config = tomllib.loads(record['bootstrap_toml'])
        self.assertEqual(config['build'], tomllib.loads(base['bootstrap_toml'])['build'])
        self.assertEqual(record['command'][5:10], ['python3', 'x.py', 'build', '--stage', '1'])
        target = Path(config['build']['host'][0]).stem
        llvm = record['llvm_config']
        self.assertEqual(config['target'][target]['llvm-config'], llvm['path'])
        self.assertTrue(llvm['version'])
        self.assertTrue(Path(llvm['libdir']).is_absolute())
        self.assertEqual(llvm['libdir_entries_before'], [])
        self.assertIsInstance(record['configuration_accepted'], bool)
        self.assertTrue(record['configuration_evidence'].strip())
        self.assertTrue(record['first_failing_step'].strip())
        self.assertTrue(record['first_error'].strip())
        self.assertEqual(record['time'], record['run']['started'])
        self.assert_probe_run(record['run'])

    def test_dependency_failures_classed(self):
        path = RECORDS / '4.json'
        self.assertTrue(path.is_file(), 'X0.4 dependency results have not been recorded')
        record = json.loads(path.read_text())
        base = json.loads((RECORDS / '2.json').read_text())
        config = tomllib.loads(record['bootstrap_toml'])
        self.assertEqual(config['build'], tomllib.loads(base['bootstrap_toml'])['build'])
        rustc, cargo = record['runs']
        self.assertEqual(rustc['command'][5:8], ['python3', 'x.py', 'check'])
        self.assertIn('compiler/rustc', rustc['command'])
        self.assertEqual(cargo['command'][6:11],
                         ['check', '-Z', 'build-std', '--target', config['build']['host'][0]])
        self.assertTrue(cargo['working_directory'].endswith('/src/tools/cargo'))
        self.assertEqual(record['time'], rustc['started'])
        for run in (rustc, cargo):
            self.assert_probe_run(run)
            failures = run['failures']
            self.assertCountEqual([f['crate'] for f in failures], run['failed_crates'])
            if run['exit_status'] != 0 and not failures:
                self.assertTrue(run['blocking_diagnostic'].strip())
            for failure in failures:
                self.assertTrue(failure['crate'].strip())
                self.assertTrue(failure['text'].strip())
                self.assertIn(failure['class'], {'breenix-side', 'dependency-change'})
                if failure['class'] == 'breenix-side':
                    self.assertTrue(failure['item'].strip())
                else:
                    self.assertTrue(failure['stand_in_crate'].strip())
                    self.assertTrue(failure['published_api_url'].startswith('https://'))

    def assert_probe_run(self, run):
        self.assertEqual(run['timeout_seconds'], 40 * 60)
        self.assertEqual(run['probe_tree_commit'], PIN)
        self.assertEqual(run['probe_tree_status_before'], '')
        self.assertEqual(run['probe_tree_status'], '')
        self.assertTrue(run['prediction'].strip())
        self.assertLess(datetime.datetime.fromisoformat(run['prediction_time']),
                        datetime.datetime.fromisoformat(run['started']))
        self.assertRegex(run['log_sha256'], r'^[0-9a-f]{64}$')

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
