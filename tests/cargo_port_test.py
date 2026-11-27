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
MACHINES = {'x86_64': 'Advanced Micro Devices X86-64', 'aarch64': 'AArch64'}


def cargo_stage(arch):
    stage = Path(os.environ.get('BREENIX_K7_STAGE', CACHE / 'stage1'))
    return stage / 'aarch64' if arch == 'aarch64' else stage


class CargoPort(unittest.TestCase):
    def test_cargo_is_breenix_elf_with_empty_undefined_set(self):
        for arch in MACHINES:
            with self.subTest(arch=arch):
                self._check_cargo_elf(arch)

    def _check_cargo_elf(self, arch):
        stage = cargo_stage(arch)
        machine = MACHINES[arch]
        cargo = stage / 'bin/cargo'
        libc = stage / 'lib/libc.so'
        self.assertTrue(cargo.is_file(), 'missing hosted Cargo ELF')
        readelf = os.environ.get('LLVM_READELF', 'llvm-readelf')

        def inspect(path, *args):
            return subprocess.check_output([readelf, *args, path], text=True, timeout=30)

        elf = inspect(cargo, '-h', '-l', '-d')
        self.assertRegex(elf, r'Class:\s+ELF64\b')
        self.assertRegex(elf, r'Type:\s+EXEC\b')
        self.assertRegex(elf, rf'Machine:\s+{machine}\b')
        self.assertEqual(re.findall(r'\[Requesting program interpreter: (.+?)\]', elf),
                         ['/lib/ld-breenix.so'])
        self.assertEqual(re.findall(r'\(NEEDED\).*\[(.+?)\]', elf), ['libc.so'])
        library = inspect(libc, '-h', '-d')
        self.assertRegex(library, r'Type:\s+DYN\b')
        self.assertRegex(library, rf'Machine:\s+{machine}\b')
        self.assertEqual(re.findall(r'\(SONAME\).*\[(.+?)\]', library), ['libc.so'])
        self.assertNotRegex(library, r'\(NEEDED\)')

        defined, undefined = set(), set()
        for path in (cargo, libc):
            for line in inspect(path, '--dyn-syms', '--wide').splitlines():
                fields = line.split()
                if len(fields) < 8 or not re.fullmatch(r'\d+:', fields[0]):
                    continue
                binding, visibility, index, name = fields[4:8]
                if index == 'UND' and binding in ('GLOBAL', 'UNIQUE'):
                    undefined.add(name)
                if ((arch == 'x86_64' or path == libc)
                        and index != 'UND' and binding in ('GLOBAL', 'WEAK', 'UNIQUE')
                        and visibility in ('DEFAULT', 'PROTECTED')):
                    # A default version also satisfies an unversioned lookup.
                    defined.add(name.replace('@@', '@'))
                    if '@@' in name:
                        defined.add(name.split('@@', 1)[0])
        self.assertEqual(sorted(undefined - defined), [])

        link_map = (stage / 'maps/cargo.map').read_text()
        self.assertTrue(link_map.strip(), 'missing Cargo linker map contents')
        self.assertNotRegex(
            link_map,
            r'\b(?:libc(?:-pic)?|libbreenix[_-]libc)(?:-[0-9a-f]+)?\.(?:a|rlib)[(:]')
        self.assertNotRegex(link_map, r'\.a[(:][^\s)]*crt[^\s)]*\.o\b')

        record = json.loads((ROOT / f'toolchain/rust-host/k7/{arch}.json').read_text())
        if arch == 'aarch64':
            pin = json.loads((ROOT / 'toolchain/rust-host/hosted-libc-aarch64.json').read_text())
            self.assertEqual(record['libc_sha256'], pin['sha256']['libc.so'])
            self.assertRegex(link_map, r'\blibbunwind\.a\(bunwind-[^\s)]+\.o\)')
            self.assertNotRegex(link_map, r'\blibunwind\.(?:a|so)(?:[(:.]|\s|$)')
            for member in re.findall(r'\blibunwind-[0-9a-f]+\.rlib\(([^)]+)\)', link_map):
                self.assertRegex(member, r'^unwind-.*\.rcgu\.o$')
        for path, key in ((cargo, 'cargo_sha256'), (libc, 'libc_sha256'),
                          (stage / 'maps/cargo.map', 'map_sha256')):
            with path.open('rb') as stream:
                self.assertEqual(hashlib.file_digest(stream, 'sha256').hexdigest(),
                                 record[key], path.name)

    def test_cargo_one_process_state(self):
        for arch in MACHINES:
            with self.subTest(arch=arch):
                self._check_cargo_process_state(arch)

    def _check_cargo_process_state(self, arch):
        stage = cargo_stage(arch)
        cargo, libc = stage / 'bin/cargo', stage / 'lib/libc.so'
        self.assertTrue(cargo.is_file(), 'missing hosted Cargo ELF')
        nm = os.environ.get('LLVM_NM', 'llvm-nm')
        symbols = {}
        for path in (cargo, libc):
            output = subprocess.check_output(
                [nm, '--defined-only', '--demangle', '--format=posix', path],
                text=True, timeout=30)
            symbols[path.name] = [tuple(line.rsplit(maxsplit=3)[:2])
                                  for line in output.splitlines()]

        def owners(pattern):
            return [file for file, definitions in symbols.items()
                    for name, _ in definitions if re.fullmatch(pattern, name)]

        self.assertEqual(owners('_start'), ['cargo'])
        for name in ('environ', 'stdin', 'stdout', 'stderr', 'optarg', 'optind',
                     'opterr', 'optopt', '__cxa_atexit', 'getauxval'):
            self.assertEqual(owners(name), ['libc.so'], name)
        for name in ('AUXV', 'PROCESS', 'RUNTIME_READY', 'EARLY_ERRNO', 'TLS_IMAGE',
                     'STACK_CACHE', 'AT_EXIT'):
            self.assertEqual(owners(r'c::.*::' + name + r'::h[0-9a-f]+(?: \(.*\))?'),
                             ['libc.so'], name)

        runtime = ('std::', 'alloc::', 'bunwind::', 'c::', '__cxxabiv1::',
                   '__rust', '__unw', '_Unwind_', '__cxa_')
        data = {}
        for file, definitions in symbols.items():
            for name, kind in definitions:
                if kind.upper() in {'B', 'D', 'G', 'R', 'S', 'V'} and name.startswith(runtime):
                    name = re.sub(r'::h[0-9a-f]{16}(?: \(.*\))?$', '', name)
                    data.setdefault(name, set()).add(file)
        self.assertEqual(sorted(name for name, files in data.items() if len(files) > 1), [])

    def test_arm_cargo_dynamic_relocations_are_supported(self):
        cargo = cargo_stage('aarch64') / 'bin/cargo'
        self.assertTrue(cargo.is_file(), 'missing hosted ARM Cargo ELF')
        readobj = os.environ.get('LLVM_READOBJ', 'llvm-readobj')
        output = subprocess.check_output([readobj, '--dyn-relocations', cargo],
                                         text=True, timeout=30)
        actual = set(re.findall(r'^\s*0x[0-9a-fA-F]+\s+(\S+)', output, re.MULTILINE))
        self.assertTrue(actual, 'missing ARM Cargo dynamic relocations')
        # The AArch64 handlers in libs/ld-breenix/src/object.rs::relocate_impl.
        supported = {'R_AARCH64_ABS64', 'R_AARCH64_GLOB_DAT', 'R_AARCH64_JUMP_SLOT',
                     'R_AARCH64_RELATIVE', 'R_AARCH64_TLSDESC'}
        self.assertEqual(sorted(actual - supported), [], f'full inventory: {sorted(actual)}')

    def test_x1_c_dependencies_resolve_in_libc(self):
        for arch in ('x86_64', 'aarch64'):
            with self.subTest(arch=arch):
                self._check_x1_c_dependencies_resolve_in_libc(arch)

    def _check_x1_c_dependencies_resolve_in_libc(self, arch):
        directory = ROOT / 'toolchain/rust-host'
        kd = ROOT / 'userspace/programs/target/kd'
        target = f'{arch}-breenix-hosted'
        if arch == 'x86_64':
            run = Path((CACHE / 'g2.path').read_text().strip())
            messages = Path(os.environ.get('BREENIX_X1_CARGO_MESSAGES_X86_64',
                            os.environ.get('BREENIX_X1_CARGO_MESSAGES',
                                           run / 'bootstrap-x86-inventory.cargo.jsonl')))
            sysroot = Path(os.environ.get('BREENIX_X1_SYSROOT', run / 'stage1'))
            libc = Path(os.environ.get('BREENIX_X1_LIBC', kd / 'libc.so'))
            inventory = directory / 'x1.json'
            machine = 'Advanced Micro Devices X86-64'
        else:
            messages = Path(os.environ['BREENIX_X1_CARGO_MESSAGES_AARCH64'])
            sysroot = Path(os.environ['BREENIX_X1_SYSROOT_AARCH64'])
            libc = Path(os.environ.get('BREENIX_X1_LIBC_AARCH64', kd / 'aarch64/libc.so'))
            inventory = directory / 'x1-aarch64.json'
            machine = 'AArch64'
        record = json.loads(inventory.read_text())
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
        self.assertRegex(header, rf'Machine:\s+{machine}\b')
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
        readelf = os.environ.get('LLVM_READELF', 'llvm-readelf')
        for arch, machine in (('x86_64', 'Advanced Micro Devices X86-64'),
                              ('aarch64', 'AArch64')):
            with self.subTest(arch=arch):
                messages = Path(os.environ.get(
                    f'BREENIX_X1_CARGO_MESSAGES_{arch.upper()}',
                    os.environ.get('BREENIX_X1_CARGO_MESSAGES',
                                   run / 'bootstrap-x86-inventory.cargo.jsonl')))
                outputs = []
                for line in messages.read_text().splitlines():
                    if not line.startswith('{'):
                        continue
                    message = json.loads(line)
                    if message['reason'] == 'build-script-executed' and re.search(
                            r'[#/]openssl-sys(?:@|#)', message['package_id']):
                        output = Path(message['out_dir'])
                        if f'{arch}-breenix-hosted' in output.parts:
                            outputs.append(output)
                self.assertEqual(len(outputs), 1, 'missing successful hosted openssl-sys build')

                for name in ('libssl.a', 'libcrypto.a'):
                    with self.subTest(archive=name):
                        archive = outputs[0] / 'openssl-build/install/lib' / name
                        headers = subprocess.check_output([readelf, '-h', str(archive)], text=True)
                        machines = re.findall(r'^\s*Machine:\s*(.+)$', headers, re.MULTILINE)
                        classes = re.findall(r'^\s*Class:\s*(.+)$', headers, re.MULTILINE)
                        types = re.findall(r'^\s*Type:\s*(\S+)', headers, re.MULTILINE)
                        self.assertTrue(machines, 'archive must contain ELF object members')
                        self.assertEqual(set(machines), {machine})
                        self.assertEqual(classes, ['ELF64'] * len(machines))
                        self.assertEqual(types, ['REL'] * len(machines))

    def test_x1_c_dependency_inventory(self):
        for arch in ('x86_64', 'aarch64'):
            with self.subTest(arch=arch):
                self._check_x1_c_dependency_inventory(arch)

    def _check_x1_c_dependency_inventory(self, arch):
        directory = ROOT / 'toolchain/rust-host'
        inventory = directory / ('x1.json' if arch == 'x86_64' else 'x1-aarch64.json')
        self.assertTrue(inventory.is_file(), 'missing hosted Cargo archive inventory x1.json')
        record = json.loads(inventory.read_text())
        licenses = (json.loads((directory / 'x1-licenses.json').read_text())
                    if arch == 'x86_64' else record['licenses'])
        self.assertEqual(record['cargo_commit'], PIN)
        self.assertEqual(record['target'], f'{arch}-breenix-hosted')
        self.assertEqual(record['features'], licenses['features'])
        expected = {(p['name'], p['version']) for p in licenses['packages']
                    if p['name'].endswith('-sys')}
        observed = [(p['name'], p['version']) for p in record['crates']]
        self.assertCountEqual(observed, expected)

        libc = record['libc']
        self.assertRegex(libc['sha256'], r'^[0-9a-f]{64}$')
        if arch == 'aarch64':
            pin = json.loads((directory / 'hosted-libc-aarch64.json').read_text())
            self.assertEqual(libc['sha256'], pin['sha256']['libc.so'])
            self.assertIsInstance(record['remaining_breenix_symbols'], list)
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
        for arch in ('x86_64', 'aarch64'):
            with self.subTest(arch=arch):
                self._check_x1_licenses_recorded(arch)

    def _check_x1_licenses_recorded(self, arch):
        cargo = CACHE / 'cargo'
        commit = subprocess.check_output(
            ['git', '-C', str(cargo), 'rev-parse', 'HEAD'], text=True).strip()
        self.assertEqual(commit, PIN)
        env = dict(os.environ, CARGO_HOME=str(CACHE / 'cargo-home'))
        metadata = json.loads(subprocess.check_output(
            ['cargo', 'metadata', '--locked', '--offline', '--format-version', '1',
             '--manifest-path', str(cargo / 'Cargo.toml'), '--filter-platform',
             f'{arch}-breenix-hosted', '--features', 'all-static'], env=env, text=True))
        tree = subprocess.check_output(
            ['cargo', 'tree', '--manifest-path', str(cargo / 'Cargo.toml'),
             '-p', 'cargo', '-e', 'normal,build', '--target',
             f'{arch}-breenix-hosted', '--locked', '--offline', '--prefix', 'none',
             '--features', 'all-static', '--format', '{p}'], env=env, text=True)
        closure = set()
        for line in tree.splitlines():
            match = re.match(r'^([^ ]+) v([^ ]+)', line)
            self.assertIsNotNone(match, line)
            closure.add(match.groups())
        directory = ROOT / 'toolchain/rust-host'
        if arch == 'x86_64':
            record = json.loads((directory / 'x1-licenses.json').read_text())
        else:
            inventory = directory / 'x1-aarch64.json'
            self.assertTrue(inventory.is_file(), 'missing ARM Cargo inventory')
            record = json.loads(inventory.read_text())['licenses']
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
