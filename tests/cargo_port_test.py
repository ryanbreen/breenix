import json
import hashlib
import os
import re
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[1]
CACHE = Path.home() / 'Library/Caches/breenix-e25/k7'
PIN = '84709f085062cbf3c51fa507527c1b2334015178'


class CargoPort(unittest.TestCase):
    def test_x1_c_dependencies_resolve_in_libc(self):
        run = Path((CACHE / 'g2.path').read_text().strip())
        messages = Path(os.environ.get(
            'BREENIX_X1_CARGO_MESSAGES', run / 'bootstrap-x86-inventory.cargo.jsonl'))
        target = 'x86_64-breenix-hosted'
        libc = Path(os.environ.get('BREENIX_X1_LIBC', ROOT /
                                  'userspace/programs/target/kd/libc.so'))
        sysroot = Path(os.environ.get('BREENIX_X1_SYSROOT', run / 'stage1'))
        record = json.loads((ROOT / 'toolchain/rust-host/x1.json').read_text())
        nm = os.environ.get('LLVM_NM', 'llvm-nm')
        readelf = os.environ.get('LLVM_READELF', 'llvm-readelf')

        def symbols(paths, *flags):
            output = subprocess.run(
                [nm, '--format=posix', '--extern-only', *flags, *paths],
                capture_output=True, text=True, check=True)
            for line in output.stderr.splitlines():
                self.assertTrue(line.endswith('no symbols'), line)
            return {fields[0] for line in output.stdout.splitlines()
                    if len(fields := line.split()) == 4 and len(fields[1]) == 1}

        header = subprocess.check_output([readelf, '-h', '-d', libc], text=True)
        self.assertRegex(header, r'Type:\s+DYN\b')
        self.assertRegex(header, r'Machine:\s+Advanced Micro Devices X86-64')
        self.assertRegex(header, r'\(SONAME\).*\[libc\.so\]')
        exports = symbols([libc], '--dynamic', '--defined-only')
        self.assertTrue(set(record['libc']['exports']) <= exports,
                        'the rebuilt shared libc lost existing exports')

        build_dirs = {}
        rust_archives = {}
        for line in messages.read_text().splitlines():
            if not line.startswith('{'):
                continue
            message = json.loads(line)
            if message['reason'] == 'build-script-executed':
                path = Path(message['out_dir'])
                if target in path.parts:
                    package = message['package_id'].rsplit('#', 1)[-1]
                    build_dirs[package] = path
            if message['reason'] == 'compiler-artifact':
                for file in map(Path, message['filenames']):
                    if target in file.parts and file.suffix == '.rlib':
                        rust_archives[message['target']['name']] = file

        archives = {}
        for crate in record['crates']:
            name = crate['name']
            self.assertEqual(crate['exit_status'], 0, name)
            package = f"{name}@{crate['version']}"
            paths = (list(build_dirs[package].rglob('*.a')) if package in build_dirs
                     else [rust_archives[name.replace('-', '_')]])
            self.assertCountEqual([path.name for path in paths],
                                  [archive['name'] for archive in crate['archives']])
            receipts = {archive['name']: archive for archive in crate['archives']}
            for path in paths:
                with path.open('rb') as stream:
                    self.assertEqual(hashlib.file_digest(stream, 'sha256').hexdigest(),
                                     receipts[path.name]['sha256'], path.name)
            archives[name] = paths

        rlibs = sorted((sysroot / 'lib/rustlib' / target / 'lib').glob('*.rlib'))
        self.assertTrue(rlibs, 'missing hosted Rust sysroot')
        providers = symbols([*rlibs, rust_archives['zlib_rs'],
                             *(path for paths in archives.values() for path in paths)],
                            '--defined-only')
        for crate, paths in archives.items():
            with self.subTest(crate=crate):
                missing = symbols(paths, '--undefined-only') - providers - exports
                self.assertEqual(sorted(missing), [])

    def test_openssl_breenix_archives(self):
        run = Path((CACHE / 'g2.path').read_text().strip())
        messages = Path(os.environ.get(
            'BREENIX_X1_CARGO_MESSAGES', run / 'bootstrap-x86-inventory.cargo.jsonl'))
        outputs = []
        for line in messages.read_text().splitlines():
            if not line.startswith('{'):
                continue
            message = json.loads(line)
            if message['reason'] == 'build-script-executed' and re.search(
                    r'[#/]openssl-sys(?:@|#)', message['package_id']):
                output = Path(message['out_dir'])
                if 'x86_64-breenix-hosted' in output.parts:
                    outputs.append(output)
        self.assertEqual(len(outputs), 1, 'missing successful hosted openssl-sys build')

        readelf = os.environ.get('LLVM_READELF', 'llvm-readelf')
        for name in ('libssl.a', 'libcrypto.a'):
            with self.subTest(archive=name):
                archive = outputs[0] / 'openssl-build/install/lib' / name
                headers = subprocess.check_output([readelf, '-h', str(archive)], text=True)
                machines = re.findall(r'^\s*Machine:\s*(.+)$', headers, re.MULTILINE)
                classes = re.findall(r'^\s*Class:\s*(.+)$', headers, re.MULTILINE)
                types = re.findall(r'^\s*Type:\s*(\S+)', headers, re.MULTILINE)
                self.assertTrue(machines, 'archive must contain ELF object members')
                self.assertEqual(set(machines), {'Advanced Micro Devices X86-64'})
                self.assertEqual(classes, ['ELF64'] * len(machines))
                self.assertEqual(types, ['REL'] * len(machines))

    def test_x1_c_dependency_inventory(self):
        directory = ROOT / 'toolchain/rust-host'
        inventory = directory / 'x1.json'
        self.assertTrue(inventory.is_file(), 'missing hosted Cargo archive inventory x1.json')
        record = json.loads(inventory.read_text())
        licenses = json.loads((directory / 'x1-licenses.json').read_text())
        self.assertEqual(record['cargo_commit'], PIN)
        self.assertEqual(record['target'], 'x86_64-breenix-hosted')
        self.assertEqual(record['features'], licenses['features'])
        expected = {(p['name'], p['version']) for p in licenses['packages']
                    if p['name'].endswith('-sys')}
        observed = [(p['name'], p['version']) for p in record['crates']]
        self.assertCountEqual(observed, expected)

        libc = record['libc']
        self.assertRegex(libc['sha256'], r'^[0-9a-f]{64}$')
        exports = set(libc['exports'])
        self.assertIn('__errno_location', exports)
        self.assertIn('getentropy', exports)
        classes = {'breenix-side', 'dependency-change'}
        for crate in record['crates']:
            with self.subTest(crate=crate['name']):
                status = crate['exit_status']
                self.assertIs(type(status), int)
                error = crate['first_error']
                if status == 0:
                    self.assertIsNone(error)
                else:
                    self.assertIsInstance(error, str)
                    self.assertTrue(error.strip())

                undef = set()
                archives = crate['archives']
                if not archives:
                    self.assertTrue(crate['archive_absence'].strip())
                for archive in archives:
                    self.assertRegex(archive['sha256'], r'^[0-9a-f]{64}$')
                    self.assertTrue(archive['name'].endswith(('.a', '.rlib')))
                    self.assertEqual(len(archive['undefined']), len(set(archive['undefined'])))
                    undef.update(archive['undefined'])
                if crate['name'] == 'libgit2-sys':
                    self.assertEqual(status, 0)
                    self.assertIn('libgit2.a', {a['name'] for a in archives})

                gaps = crate['gaps']
                self.assertEqual(set(gaps), undef - exports)
                headers = crate['missing_headers']
                if error:
                    for header in re.findall(r"'([^']+\.h)' file not found", error):
                        self.assertIn(header, headers)
                for name, category in (gaps | headers).items():
                    self.assertTrue(name.strip())
                    self.assertIn(category, classes, name)

    def test_x1_licenses_recorded(self):
        cargo = CACHE / 'cargo'
        commit = subprocess.check_output(
            ['git', '-C', str(cargo), 'rev-parse', 'HEAD'], text=True).strip()
        self.assertEqual(commit, PIN)
        env = dict(os.environ, CARGO_HOME=str(CACHE / 'cargo-home'))
        metadata = json.loads(subprocess.check_output(
            ['cargo', 'metadata', '--locked', '--offline', '--format-version', '1',
             '--manifest-path', str(cargo / 'Cargo.toml'), '--filter-platform',
             'x86_64-breenix-hosted', '--features', 'all-static'], env=env, text=True))
        tree = subprocess.check_output(
            ['cargo', 'tree', '--manifest-path', str(cargo / 'Cargo.toml'),
             '-p', 'cargo', '-e', 'normal,build', '--target',
             'x86_64-breenix-hosted', '--locked', '--offline', '--prefix', 'none',
             '--features', 'all-static', '--format', '{p}'], env=env, text=True)
        closure = set()
        for line in tree.splitlines():
            match = re.match(r'^([^ ]+) v([^ ]+)', line)
            self.assertIsNotNone(match, line)
            closure.add(match.groups())
        record = json.loads((ROOT / 'toolchain/rust-host/x1-licenses.json').read_text())
        self.assertEqual(record['cargo_commit'], PIN)
        self.assertEqual(record['features'], ['all-static'])
        packages = {(entry['name'], entry['version']): entry for entry in record['packages']}
        self.assertEqual(len(packages), len(record['packages']))
        self.assertEqual(set(packages), closure)
        workspace = {p['id']: (p['name'], p['version']) for p in metadata['packages']
                     if p['id'] in metadata['workspace_members']}
        excluded = {(e['name'], e['version']) for e in record['excluded']}
        self.assertEqual(excluded, set(workspace.values()) - closure)
        self.assertEqual(len(excluded), len(record['excluded']))
        for entry in record['excluded']:
            self.assertTrue(entry['reason'])
        for package in metadata['packages']:
            if (package['name'], package['version']) not in closure:
                continue
            entry = packages[package['name'], package['version']]
            self.assertTrue(entry['spdx'], package['name'])
            self.assertTrue(entry['source'], package['name'])
            self.assertIn(entry['disposition'], ('permissive', 'opaque only'))
            self.assertEqual(entry['metadata_license'], package['license'])
        libraries = {entry['name']: entry for entry in record['vendored_c_libraries']}
        self.assertEqual(len(libraries), len(record['vendored_c_libraries']))
        self.assertEqual(set(libraries), {'zlib', 'curl', 'nghttp2', 'OpenSSL',
                                         'libgit2', 'libssh2', 'SQLite'})
        for library in libraries.values():
            for key in ('version', 'spdx', 'source', 'vendored_by', 'disposition'):
                self.assertTrue(library[key], (library['name'], key))


if __name__ == '__main__':
    unittest.main()
