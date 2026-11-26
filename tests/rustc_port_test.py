import datetime
import hashlib
import json
import mmap
import os
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
import unittest
from unittest import mock
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RECORDS = ROOT / 'toolchain/rust-host/x0'
TREE = Path.home() / 'fun/code/rust.worktrees/x0-666252fd'
BASE = '706f244db'
PIN = '666252fd61839b9e9b1bc9e777a0c84c5cf3dc6d'
RUSTIX_PATCH = RECORDS.parent / 'k5/patches/rustix-1.0.7-breenix.patch'
GETRANDOM_PATCH = RECORDS.parent / 'k5/patches/getrandom-0.2.16-breenix.patch'
LIBLOADING_PATCH = RECORDS.parent / 'k5/patches/libloading-0.8.8-breenix.patch'
NIX_PATCH = RECORDS.parent / 'k5/patches/nix-0.30.1-breenix.patch'
ERRNO_PATCH = RECORDS.parent / 'k5/patches/errno-0.3.12-breenix.patch'


sys.path.insert(0, str(ROOT / "scripts/rust-host"))
from rustc_port import guest_environment


def git(*args, tree=TREE):
    return subprocess.run(['git', '-C', str(tree), *args], text=True,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True).stdout.strip()


class RustcPort(unittest.TestCase):
    def test_twin_spawn_environment_is_guest_set(self):
        with mock.patch.dict(os.environ, {"RUSTFLAGS": "ambient", "CARGO_HOME": "ambient"}):
            self.assertEqual(guest_environment(Path("/fixture/targets")),
                             {"RUST_TARGET_PATH": "/fixture/targets"})

    def test_guest_target_loads_twin_sysroot(self):
        cache = Path.home() / 'Library/Caches/breenix-e25/k5/twin'
        mac_rustc = Path(os.environ.get('BREENIX_K5_MAC_RUSTC', cache / 'mac/bin/rustc'))
        sysroot = Path(os.environ.get('BREENIX_K5_SYSROOT', cache / 'sysroot'))
        suite = (ROOT / 'userspace/programs/src/suite_rustc.rs').read_text()
        match = re.search(r'^const TARGET: &str = "([^"]+)";$', suite, re.M)
        self.assertIsNotNone(match, 'guest target argument')
        with tempfile.TemporaryDirectory(prefix='breenix-k5-target-') as directory:
            fixture = Path(directory)
            targets = fixture / 'targets'
            targets.mkdir()
            shutil.copyfile(ROOT / 'x86_64-breenix.json', targets / 'x86_64-breenix.json')
            corpus = fixture / 'corpus'
            shutil.copytree(RECORDS.parent / 'k5/corpus', corpus)
            host = json.loads((corpus / 'expect.json').read_text())['hosted_host']
            shutil.copyfile(ROOT / f'{host}.json', targets / f'{host}.json')
            # Translate the guest's explicit spawn environment to the host fixture.
            self.assertIn('let envp = [target_path.as_ptr().cast(), null()];', suite)
            self.assertIn('CString::new(format!("RUST_TARGET_PATH={FIXTURE}/targets"))', suite)
            environment = guest_environment(targets)
            host_lookup = subprocess.run(
                [mac_rustc, '--target', host, '--print', 'cfg'], cwd=corpus, env=environment,
                capture_output=True, text=True, timeout=15)
            self.assertEqual(host_lookup.returncode, 0, host_lookup.stderr)
            output = fixture / 'uses-std.o'
            result = subprocess.run(
                [mac_rustc, '--edition=2021', '--sysroot', sysroot, '-Dwarnings',
                 '--emit=obj', '--target', match[1], 'uses-std.rs', '-o', output],
                cwd=corpus, env=environment,
                capture_output=True, text=True, timeout=15)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertGreater(output.stat().st_size, 0)

    def test_rustc_is_dynamic_breenix_elf(self):
        stage = ROOT / 'userspace/programs/target/rustc/x86_64'
        record = json.loads((RECORDS.parent / 'k5/x86_64.json').read_text())
        llvm = Path(os.environ.get('BREENIX_X2_LLVM', '/opt/homebrew/opt/llvm@20/bin'))
        rustc = stage / 'rustc'
        self.assertTrue(rustc.is_file(), 'build the hosted twin with build.sh --rustc')
        drivers = list(stage.glob('librustc_driver-*.so'))
        self.assertEqual(len(drivers), 1, drivers)
        driver, = drivers
        cache = Path(os.environ.get('BREENIX_RUSTC_PORT_BUILDS', ROOT / 'build/rustc-port'))
        libc = cache / 'hosted/lib/libc.so'

        def readelf(elf, *args):
            return subprocess.check_output([llvm / 'llvm-readelf', *args, elf],
                                           text=True, timeout=10)

        definitions = set()
        strong = {}
        weak = {}
        for elf, kind, needed in ((rustc, 'EXEC', [driver.name, libc.name]),
                                  (driver, 'DYN', [libc.name]),
                                  (libc, 'DYN', [])):
            headers = readelf(elf, '-W', '-h', '-l', '-d')
            self.assertRegex(headers, r'Class:\s+ELF64')
            self.assertRegex(headers, r'Data:.*little endian')
            self.assertRegex(headers, r'Machine:\s+Advanced Micro Devices X86-64')
            self.assertRegex(headers, rf'Type:\s+{kind}\b')
            self.assertEqual(re.findall(r'\(NEEDED\).*\[(.+)\]', headers), needed, elf)
            interp = re.findall(r'Requesting program interpreter: (.+)\]', headers)
            self.assertEqual(interp, ['/lib/ld-breenix.so'] if elf == rustc else [], elf)
            if elf == rustc:
                self.assertEqual(re.findall(r'\(RUNPATH\).*\[(.+)\]', headers),
                                 ['$ORIGIN/../lib'])
                self.assertNotIn('(RPATH)', headers)
            else:
                self.assertEqual(re.findall(r'\(SONAME\).*\[(.+)\]', headers), [elf.name])

            strong[elf] = set()
            weak[elf] = set()
            for line in readelf(elf, '-W', '--dyn-syms').splitlines():
                fields = line.split()
                if len(fields) < 8 or not fields[0].removesuffix(':').isdigit():
                    continue
                _, value, _, _, binding, visibility, index, name, *_ = fields
                if index == 'UND':
                    if binding == 'WEAK':
                        self.assertEqual(int(value, 16), 0, name)
                        weak[elf].add(name)
                    else:
                        strong[elf].add(name)
                elif binding in ('GLOBAL', 'WEAK') and visibility in ('DEFAULT', 'PROTECTED'):
                    definitions.add(name)
        self.assertEqual({elf.name: sorted(names - definitions) for elf, names in strong.items()
                          if names - definitions}, {})
        self.assertEqual({elf.name: sorted(names & definitions) for elf, names in weak.items()
                          if names & definitions}, {})

        with libc.open('rb') as artifact:
            self.assertEqual(hashlib.file_digest(artifact, 'sha256').hexdigest(),
                             record['hosted_link']['libc_sha256'])
        for elf, key in ((rustc, 'hosted_rustc_sha256'), (driver, 'hosted_driver_sha256')):
            with elf.open('rb') as artifact:
                self.assertEqual(hashlib.file_digest(artifact, 'sha256').hexdigest(), record[key])
            link_map = stage / f'{elf.name}.map'
            self.assertTrue(link_map.is_file(), link_map)
            with link_map.open('rb') as artifact:
                self.assertEqual(hashlib.file_digest(artifact, 'sha256').hexdigest(),
                                 record['hosted_link']['maps_sha256'][link_map.name])
            with link_map.open('rb') as stream, mmap.mmap(stream.fileno(), 0,
                                                         access=mmap.ACCESS_READ) as contents:
                self.assertNotRegex(contents, rb'(?:^|[/\s])lib(?:c|unwind)\.a[( :]')
                self.assertEqual(contents.find(b'breenix-crt.o') >= 0, elf == rustc)
                self.assertEqual(contents.find(b'libbunwind.a(') >= 0, elf == driver)

    def test_twin_rlibs_match_twin_compiler(self):
        cache = Path.home() / 'Library/Caches/breenix-e25/k5/twin'
        mac_rustc = Path(os.environ.get('BREENIX_K5_MAC_RUSTC', cache / 'mac/bin/rustc'))
        sysroot = Path(os.environ.get('BREENIX_K5_SYSROOT', cache / 'sysroot'))
        verbose = subprocess.check_output([mac_rustc, '-vV'], text=True, timeout=5)
        banner, *fields = verbose.splitlines()
        version = dict(line.split(': ', 1) for line in fields)
        targets = ('x86_64-breenix', 'x86_64-breenix-hosted')
        digests = {}
        failures = []
        for target in targets:
            archives = sorted((sysroot / 'lib/rustlib' / target / 'lib').glob('*.rlib'))
            for crate in ('core', 'alloc', 'std'):
                self.assertEqual(sum(path.name.startswith(f'lib{crate}-') for path in archives),
                                 1, target)
            for archive in archives:
                result = subprocess.run([mac_rustc, '-Zls=root', archive],
                                        capture_output=True, text=True, timeout=5, check=True)
                if result.stderr or f'triple {target}' not in result.stdout.splitlines():
                    failures.append(f'{target}/{archive.name}: {result.stderr or result.stdout}'.strip())
                with archive.open('rb') as artifact:
                    digests[archive.relative_to(sysroot).as_posix()] = hashlib.file_digest(
                        artifact, 'sha256').hexdigest()
        self.assertFalse(failures, '\n'.join(failures))

        commit = version['commit-hash']
        self.assertRegex(commit, r'^[0-9a-f]{40}$')
        match = re.fullmatch(rf"rustc {re.escape(version['release'])} "
                             rf"\(([0-9a-f]{{7,40}}) {re.escape(version['commit-date'])}\)", banner)
        self.assertIsNotNone(match, banner)
        self.assertTrue(commit.startswith(match[1]))
        hosted = Path(os.environ.get('BREENIX_K5_HOSTED', cache / 'hosted'))
        driver, = (hosted / 'lib').glob('librustc_driver-*.so')
        llvm = Path(os.environ.get('BREENIX_X2_LLVM', '/opt/homebrew/opt/llvm@20/bin'))
        headers = subprocess.check_output(
            [llvm / 'llvm-readelf', '--file-header', '--dynamic-table', hosted / 'bin/rustc'],
            text=True, timeout=5)
        self.assertRegex(headers, r'Machine:\s+Advanced Micro Devices X86-64')
        self.assertIn(driver.name, re.findall(r'\(NEEDED\).*\[(.+)\]', headers))
        # The hosted ELF cannot run on the Mac. Check the linked driver's
        # embedded identity here and reserve its live -vV for the guest case.
        with driver.open('rb') as binary, mmap.mmap(binary.fileno(), 0, access=mmap.ACCESS_READ) as data:
            for value in (version['release'], commit, version['commit-date'], targets[1]):
                self.assertNotEqual(data.find(value.encode()), -1, f'hosted rustc identity field: {value}')

        record = json.loads((RECORDS.parent / 'k5/x86_64.json').read_text())
        pin = tomllib.loads((ROOT / 'rust-fork.toml').read_text())['hosted']['commit']
        self.assertEqual(commit, pin)
        self.assertEqual(record['sysroot'], {
            'rustc_version': banner, 'commit_hash': commit, 'files_sha256': digests,
        })

    def test_rustix_patch_is_breenix_only(self):
        patch = RUSTIX_PATCH.read_text()
        records = re.split(r'(?m)^diff --git ', patch)[1:]
        self.assertTrue(records)
        for record in records:
            header = record.splitlines()[0]
            match = re.fullmatch(r'a/(src/[\w/]+\.rs) b/\1', header)
            self.assertIsNotNone(match, header)
            path = match[1]
            self.assertNotIn('tests', Path(path).parts)
            self.assertIn(f'\n--- a/{path}\n+++ b/{path}\n', record)
            hunks = re.split(r'(?m)^@@ .* @@[^\n]*\n', record)[1:]
            self.assertTrue(hunks, path)
            for hunk in hunks:
                self.assertFalse(any(line.startswith('-') for line in hunk.splitlines()), path)
                added = '\n'.join(line[1:] for line in hunk.splitlines() if line.startswith('+'))
                self.assertIsNotNone(re.fullmatch(
                    r'#\[cfg\(target_os = "breenix"\)\]\n(?:'
                    r'type \w+ = [\w:]+;|'
                    r'bitflags! \{\n(?:    [^\n]+\n)+\}'
                    r'\n#\[cfg_attr\(target_os = "breenix", cfg\(any\(\)\)\)\])', added), path)

    def test_rustix_patch_preserves_registry_copy(self):
        self.assert_patch_preserves_registry_copy(RUSTIX_PATCH, 'rustix-1.0.7')

    def test_getrandom_02_patch_is_breenix_only(self):
        patch = GETRANDOM_PATCH.read_text()
        headers = re.findall(r'(?m)^diff --git (.*)$', patch)
        self.assertEqual(headers, ['a/src/lib.rs b/src/lib.rs',
                                   'a/src/util_libc.rs b/src/util_libc.rs'])
        self.assertFalse(re.search(r'(?m)^-(?!-- )', patch))
        self.assertEqual(re.findall(r'(?m)^\+(?!\+\+ )(.*)$', patch), [
            '    } else if #[cfg(target_os = "breenix")] {',
            '        mod util_libc;',
            '        #[path = "getentropy.rs"] mod imp;',
            '    } else if #[cfg(target_os = "breenix")] {',
            '        use libc::__errno_location as errno_location;',
        ])
        self.assert_patch_preserves_registry_copy(GETRANDOM_PATCH, 'getrandom-0.2.16')

    def test_libloading_patch_matches_breenix_dlfcn(self):
        patch = LIBLOADING_PATCH.read_text()
        source = 'src/os/unix/consts.rs'
        self.assertEqual(re.findall(r'(?m)^diff --git (.*)$', patch), [f'a/{source} b/{source}'])
        self.assertIn(f'\n--- a/{source}\n+++ b/{source}\n', patch)
        self.assertFalse(re.search(r'(?m)^-(?!-- )', patch))
        header = (ROOT / 'libs/libbreenix-libc/include/dlfcn.h').read_text()
        values = dict(re.findall(r'(?m)^#define (RTLD_\w+) (\S+)$', header))
        hunks = re.split(r'(?m)^@@ .* @@[^\n]*\n', patch)[1:]
        self.assertEqual(len(hunks), 4)
        for name, hunk in zip(('RTLD_LAZY', 'RTLD_NOW', 'RTLD_GLOBAL', 'RTLD_LOCAL'), hunks):
            self.assertEqual(hunk.splitlines(), [
                '+        } else if #[cfg(target_os = "breenix")] {',
                f'+            pub(super) const {name}: c_int = {values[name]};',
            ])
        self.assert_patch_preserves_registry_copy(LIBLOADING_PATCH, 'libloading-0.8.8')

    def test_nix_patch_is_breenix_only(self):
        patch = NIX_PATCH.read_text()
        self.assertEqual(re.findall(r'(?m)^diff --git (.*)$', patch),
                         ['a/src/errno.rs b/src/errno.rs',
                          'a/src/sys/signal.rs b/src/sys/signal.rs',
                          'a/src/unistd.rs b/src/unistd.rs'])
        for path in ('src/errno.rs', 'src/sys/signal.rs', 'src/unistd.rs'):
            self.assertIn(f'\n--- a/{path}\n+++ b/{path}\n', patch)
        unistd = patch.split('diff --git a/src/unistd.rs b/src/unistd.rs\n')[1]
        self.assertEqual(re.findall(r'(?m)^\+    target_os = "breenix",$', unistd),
                         ['+    target_os = "breenix",'] * 2)
        for hunk in re.split(r'(?m)^@@ .* @@[^\n]*\n', patch)[1:]:
            removed = re.findall(r'(?m)^-(?!-- )(.*)$', hunk)
            added = re.findall(r'(?m)^\+(?!\+\+ )(.*)$', hunk)
            for line in added.copy():
                if line.strip() in ('target_os = "breenix",',
                                    '#[cfg_attr(target_os = "breenix", derive(Clone, Debug))]'):
                    added.remove(line)
            for line in added:
                self.assertIn('target_os = "breenix"', line)
            normalized = [line.replace(', target_os = "breenix"', '')
                          .replace('#[cfg_attr(not(target_os = "breenix"), derive(',
                                   '#[derive(')
                          .replace('PartialEq))]', 'PartialEq)]') for line in added]
            self.assertEqual(removed, normalized)
        self.assert_patch_preserves_registry_copy(NIX_PATCH, 'nix-0.30.1')

    def test_errno_patch_is_breenix_only(self):
        patch = ERRNO_PATCH.read_text()
        self.assertEqual(re.findall(r'(?m)^diff --git (.*)$', patch),
                         ['a/src/unix.rs b/src/unix.rs'])
        self.assertIn('\n--- a/src/unix.rs\n+++ b/src/unix.rs\n', patch)
        self.assertFalse(re.search(r'(?m)^-(?!-- )', patch))
        self.assertEqual(re.findall(r'(?m)^\+(?!\+\+ )(.*)$', patch), [
            '    #[cfg_attr(target_os = "breenix", link_name = "__errno_location")]',
        ])

    def test_errno_patch_preserves_registry_copy(self):
        self.assert_patch_preserves_registry_copy(ERRNO_PATCH, 'errno-0.3.12')

    def test_x2_driver_uses_breenix_errno_location(self):
        stage = Path(os.environ.get('BREENIX_X2_STAGE', Path.home() /
                                    'Library/Caches/breenix-e25/kd/x2/stage1'))
        llvm = Path(os.environ.get('BREENIX_X2_LLVM', '/opt/homebrew/opt/llvm@20/bin'))
        driver, = (stage / 'lib').glob('librustc_driver-*.so')

        def symbols(elf):
            output = subprocess.check_output(
                [llvm / 'llvm-nm', '-D', '--format=posix', elf], text=True, timeout=30)
            return {name: kind for name, kind, *_ in map(str.split, output.splitlines())
                    if name in ('errno_location', '__errno_location')}

        imports = symbols(driver)
        exports = symbols(stage / 'lib/libc.so')
        self.assertNotIn('errno_location', imports)
        self.assertNotIn('errno_location', exports)
        self.assertEqual(imports.get('__errno_location'), 'U')
        self.assertEqual(exports.get('__errno_location'), 'T')

    def assert_patch_preserves_registry_copy(self, patch, crate):
        self.assertTrue(patch.is_file(), f'{crate} port patch is missing')
        cargo = Path(os.environ.get('BREENIX_X2_CARGO_HOME', Path.home() /
                                    'Library/Caches/breenix-e25/k5/x2b/cargo'))
        sources = list(cargo.glob(f'registry/src/*/{crate}'))
        self.assertEqual(len(sources), 1, sources)
        source = sources[0].resolve()

        def snapshot(tree):
            return {path.relative_to(tree): hashlib.sha256(path.read_bytes()).hexdigest()
                    for path in tree.rglob('*') if path.is_file()}

        before = snapshot(source)
        self.assertTrue(before)
        with tempfile.TemporaryDirectory(prefix='breenix-x2-patch-') as directory:
            local = Path(directory) / source.name
            shutil.copytree(source, local)
            subprocess.run(['git', 'apply', '--no-index', '--unidiff-zero', patch], cwd=local,
                           check=True, timeout=15)
            self.assertNotEqual(snapshot(local), before)
        self.assertEqual(snapshot(source), before)

    def test_x2_static_route_answered_with_citations(self):
        record = json.loads((RECORDS.parent / 'x2.json').read_text())
        route = record['static_route']
        self.assertIn(route['answer'], ('documented', 'none'))
        self.assertTrue(route['citations'])
        pin = tomllib.loads((ROOT / 'rust-fork.toml').read_text())['hosted']['commit']
        for citation in route['citations']:
            self.assertEqual(set(citation), {'file', 'line', 'quote'})
            self.assertTrue(citation['file'] == 'bootstrap.example.toml' or
                            citation['file'].startswith('src/doc/rustc-dev-guide/') or
                            citation['file'].startswith('src/doc/rustc/'))
            self.assertIs(type(citation['line']), int)
            self.assertGreater(citation['line'], 0)
            self.assertTrue(citation['quote'].strip())
            contents = git('show', f"{pin}:{citation['file']}", tree=ROOT / 'rust-fork')
            lines = contents.splitlines()
            self.assertLessEqual(citation['line'], len(lines))
            self.assertEqual(lines[citation['line'] - 1], citation['quote'])

    def test_x2_one_process_state(self):
        stage = Path(os.environ.get('BREENIX_X2_STAGE', Path.home() /
                                    'Library/Caches/breenix-e25/kd/x2/stage1'))
        llvm = Path(os.environ.get('BREENIX_X2_LLVM', '/opt/homebrew/opt/llvm@20/bin'))
        drivers = sorted((stage / 'lib').glob('librustc_driver-*.so'))
        self.assertEqual(len(drivers), 1, drivers)
        executable = stage / 'bin/rustc'
        driver, = drivers
        libc = stage / 'lib/libc.so'

        def inspect(tool, path, *args):
            return subprocess.check_output([llvm / tool, *args, path], text=True, timeout=30)

        def definitions(path, *args):
            # POSIX format puts the symbol name before the type, value and size.
            # In demangled output the name itself may contain spaces.
            return [tuple(line.rsplit(maxsplit=3)[:2]) for line in
                    inspect('llvm-nm', path, '--defined-only', '--format=posix', *args).splitlines()]

        paths = (executable, driver, libc)
        # Inspect the known-bad executable and driver even when no libc.so exists.
        symbols = {path: definitions(path) for path in paths if path.is_file()}

        def owners(pattern):
            return [path.name for path, names in symbols.items() for name, _ in names
                    if re.fullmatch(pattern, name)]

        self.assertEqual(owners('_start'), ['rustc'])
        self.assertIn(libc, symbols, 'X2 is missing its shared libc')
        for name in ('environ', 'stdin', 'stdout', 'stderr', 'optarg', 'optind',
                     'opterr', 'optopt', '__cxa_atexit', 'getauxval'):
            self.assertEqual(owners(name), ['libc.so'], name)
        for name in ('AUXV', 'PROCESS', 'RUNTIME_READY', 'EARLY_ERRNO', 'TLS_IMAGE',
                     'STACK_CACHE', 'AT_EXIT'):
            self.assertEqual(owners(r'_ZN1c.*' + str(len(name)) + name +
                                    r'17h[0-9a-f]+E(?:\..*)?'), ['libc.so'], name)

        for path, expected in ((executable, {driver.name, libc.name}),
                               (driver, {libc.name}), (libc, set())):
            dynamic = inspect('llvm-readelf', path, '--file-header', '--dynamic-table')
            self.assertEqual(set(re.findall(r'\(NEEDED\).*\[(.+)\]', dynamic)), expected, path)
            if path == libc:
                self.assertRegex(dynamic, r'Type:\s+DYN\b')
                self.assertEqual(re.findall(r'\(SONAME\).*\[(.+)\]', dynamic), ['libc.so'])

        for path in (executable, driver):
            link_map = (stage / 'maps' / (path.name + '.map')).read_text()
            self.assertNotRegex(link_map, r'\blibc\.a\(')
            self.assertNotRegex(link_map, r'\blibc\.a:')
            self.assertFalse([name for name, _ in symbols[path] if name.startswith('_ZN1c')])
            if path == driver:
                self.assertNotRegex(link_map, r'\bbreenix-crt\.o\b')

        runtime_data = {}
        for path in paths:
            for name, kind in definitions(path, '--demangle'):
                if kind.upper() not in {'B', 'D', 'G', 'S', 'V'}:
                    continue
                name = re.sub(r'(?:\.llvm\.[0-9]+|::h[0-9a-f]{16})$', '', name)
                if (name.startswith(('std::', 'alloc::', 'bunwind::', 'c::stdlib::',
                                     'std::__', '__cxxabiv1::', '__rust', '__unw',
                                     '_Unwind_', '__cxa_'))):
                    runtime_data.setdefault(name, set()).add(path.name)
        duplicates = {name: sorted(files) for name, files in runtime_data.items() if len(files) > 1}
        self.assertEqual(duplicates, {})

    def test_x2_driver_linked_inventory(self):
        path = RECORDS.parent / 'x2.json'
        self.assertTrue(path.is_file(), 'X2 rustc driver link result has not been recorded')
        record = json.loads(path.read_text())
        errno_port = record['ruling_108']['patch']
        self.assertEqual(errno_port['sha256'], hashlib.sha256(ERRNO_PATCH.read_bytes()).hexdigest())
        self.assertEqual(errno_port['text'], ERRNO_PATCH.read_text())
        self.assertTrue(record['prediction'].strip())
        self.assertLess(datetime.datetime.fromisoformat(record['prediction_time']),
                        datetime.datetime.fromisoformat(record['time']))
        self.assertEqual(record['target'], 'x86_64-breenix-hosted')
        command = record['command']
        self.assertEqual(command[:2], ['taskpolicy', '-b'])
        self.assertIn('--config', command)
        self.assertIn('compiler/rustc', command)
        self.assertEqual(record['result'], 'linked')
        self.assertEqual(record['exit_status'], 0)

        pin = tomllib.loads((ROOT / 'rust-fork.toml').read_text())['hosted']['commit']
        self.assertEqual(record['rust_commit'], pin)
        fork = ROOT / 'rust-fork'
        self.assertEqual(record['compiler_tree'], git('rev-parse', f'{pin}:compiler', tree=fork))
        self.assertEqual(record['compiler_tree'], git('rev-parse', f'{PIN}:compiler'))
        self.assertEqual(record['llvm_commit'],
                         git('ls-tree', pin, 'src/llvm-project', tree=fork).split()[2])
        self.assertEqual(git('status', '--porcelain', '--ignore-submodules=none',
                             '--', 'compiler', 'src/llvm-project'), '')
        for field in ('pinned_status_before', 'pinned_status_after'):
            self.assertEqual(record[field], {'compiler': '', 'llvm_project': ''})
        self.assertRegex(record['log_sha256'], r'^[0-9a-f]{64}$')

        fields = {'dynamic_relocations', 'tls_models', 'undefined_symbols', 'dt_needed', 'pt_interp'}
        inventory = record['inventory']
        self.assertEqual(set(inventory), fields)
        artifacts = record['artifacts']
        self.assertEqual(set(artifacts), {'librustc_driver', 'rustc', 'libc'})
        driver = artifacts['librustc_driver']
        executable = artifacts['rustc']
        libc = artifacts['libc']
        self.assertRegex(driver['name'], r'^librustc_driver(?:-[0-9a-f]+)?\.so$')
        self.assertNotIn('errno_location', driver['inventory']['undefined_symbols'])
        self.assertIn('__errno_location', driver['inventory']['undefined_symbols'])
        self.assertEqual(executable['name'], 'rustc')
        self.assertEqual(libc['name'], 'libc.so')
        self.assertEqual(libc['elf_type'], 'ET_DYN')
        self.assertEqual(libc['dt_soname'], ['libc.so'])
        for artifact in artifacts.values():
            self.assertRegex(artifact['sha256'], r'^[0-9a-f]{64}$')
            self.assertEqual(set(artifact['inventory']), fields)
            for names in artifact['inventory'].values():
                self.assertIsInstance(names, list)
                self.assertTrue(all(isinstance(name, str) and name.strip() for name in names))
                self.assertEqual(names, sorted(set(names)))
        for field, names in inventory.items():
            self.assertEqual(names, sorted({name for artifact in artifacts.values()
                                            for name in artifact['inventory'][field]}))
        if record['static_route']['answer'] == 'none':
            self.assertEqual(set(executable['inventory']['dt_needed']),
                             {driver['name'], libc['name']})
            self.assertEqual(driver['inventory']['dt_needed'], [libc['name']])
            self.assertEqual(driver['inventory']['pt_interp'], [])
            self.assertEqual(libc['inventory']['dt_needed'], [])
            self.assertEqual(libc['inventory']['pt_interp'], [])
            self.assertEqual(len(executable['inventory']['pt_interp']), 1)
            self.assertTrue(executable['inventory']['pt_interp'][0].startswith('/'))
            self.assertTrue(inventory['dynamic_relocations'])

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
