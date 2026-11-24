"""K3 runtime archive and hosted link checks."""
import json
import os
import re
import sys
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/rust-host'))
from x0_runtime import llvm_tool, symbols

HEADER_TREE = Path('libs/libbreenix-libc/include')
HEADER_BASE = '5f7bff4e8b48178b0f4b84e57a567a2f8e4d3171'


class CxxRuntimeTest(unittest.TestCase):
    def test_ostream_compiles_and_links_both_arches(self):
        lld = llvm_tool(ROOT, 'rust-lld')
        nm = llvm_tool(ROOT, 'llvm-nm')
        inputs = Path(os.environ.get('BREENIX_CXX_RUNTIME_ROOT', ROOT / 'build/cxx-runtime'))
        builds = Path(os.environ.get('BREENIX_CXX_RUNTIME_BUILDS', ROOT / 'build/cxx-runtime'))
        for arch in ('x86_64', 'aarch64'):
            with self.subTest(arch=arch), tempfile.TemporaryDirectory(prefix='breenix-k3-ostream-') as temp:
                obj = Path(temp) / 'ostream.o'
                elf = Path(temp) / 'ostream.elf'
                subprocess.run(
                    [ROOT / 'scripts/workload-cxx', '--arch', arch,
                     '--runtime-build', builds / arch / 'build', '--',
                     '-std=c++23', '-Werror', '-x', 'c++', '-', '-o', obj],
                    input=('#include <ostream>\n'
                           'static_assert(sizeof(std::ostream) > 0);\n'
                           'int main() {\n'
                           '    std::ostream stream(nullptr);\n'
                           '    return stream.bad() ? 0 : 1;\n'
                           '}\n'),
                    text=True, check=True, timeout=10,
                )
                lib = inputs / arch / 'lib'
                providers = [lib / name for name in
                             ('libc++.a', 'libc++abi.a', 'libc.a', 'compiler_builtins.rlib')]
                subprocess.run(
                    ['taskpolicy', '-b', lld, '-flavor', 'gnu', '-static', '--gc-sections',
                     '--eh-frame-hdr', '--fatal-warnings', '-T', ROOT / 'userspace/programs/linker-hosted.ld',
                     '-o', elf, obj, '--start-group', *providers, '--end-group'],
                    check=True, timeout=10,
                )
                self.assertEqual(subprocess.check_output([nm, '--undefined-only', elf], text=True), '')

    def test_archives_build_both_arches(self):
        import cxx_runtime

        llvm = cxx_runtime.default_llvm()
        pin = cxx_runtime.verify_llvm(ROOT, llvm)
        runs = Path(os.environ.get('BREENIX_CXX_RUNTIME_BUILDS', ROOT / 'build/cxx-runtime'))
        for arch in ('x86_64', 'aarch64'):
            with self.subTest(arch=arch):
                run = runs / arch
                record = json.loads((run / 'record.json').read_text())
                self.assertEqual(record['arch'], arch)
                self.assertEqual(record['llvm_project'], pin)
                for stage in ('configure', 'build'):
                    result = record[stage]
                    self.assertIsNotNone(result, f'{stage} did not run')
                    self.assertEqual(result['exit_status'], 0, result['first_error'])
                    self.assertIsNone(result['first_error'])
                    log = run / f'{stage}.log'
                    self.assertEqual(cxx_runtime.digest(log), result['log_sha256'])
                    errors = [line for line in log.read_text().splitlines()
                              if cxx_runtime.FIRST_ERROR.search(line)]
                    self.assertEqual(errors, [])
                for name, component in (('libc++.a', 'libcxx'), ('libc++abi.a', 'libcxxabi')):
                    archive = run / 'build/lib' / name
                    self.assertTrue(archive.is_file(), f'missing {archive}')
                    self.assertEqual(archive.read_bytes()[:8], b'!<arch>\n')
                    self.assertEqual(cxx_runtime.digest(archive), record['archives'][name]['sha256'])
                    notice = archive.with_name(name + '.LICENSE.TXT')
                    self.assertEqual(notice.read_bytes(), (llvm / component / 'LICENSE.TXT').read_bytes())
                    self.assertEqual(cxx_runtime.digest(notice), record['archives'][name]['notice_sha256'])
                self.assertFalse(list((run / 'build').rglob('*.so')))
        self.assertEqual(cxx_runtime.verify_llvm(ROOT, llvm), pin)

    def test_header_changes_keep_c_objects_identical(self):
        paths = subprocess.check_output(
            ['git', 'ls-files', '-z', '--', 'toolchain/corpus/*.c', 'userspace/*.c'],
            cwd=ROOT, text=True,
        ).split('\0')
        # Constraint inputs have their own diagnostics and cannot emit objects.
        sources = [Path(path) for path in paths if path and '/constraints/' not in path
                   and path != 'toolchain/corpus/c-core/diagnostic.c']
        clang = os.environ.get('BREENIX_TOOLCHAIN_CLANG', '/opt/homebrew/opt/llvm/bin/clang')
        resource = subprocess.check_output([clang, '-print-resource-dir'], text=True).strip()
        env = dict(os.environ)
        for name in ('CPATH', 'C_INCLUDE_PATH'):
            env.pop(name, None)

        failures = []
        with tempfile.TemporaryDirectory(prefix='breenix-k3-headers-') as temp:
            scratch = Path(temp)
            tar = scratch / 'base.tar'
            subprocess.run(['git', 'archive', '-o', tar, HEADER_BASE, HEADER_TREE],
                           cwd=ROOT, check=True)
            subprocess.run(['tar', '-xf', tar, '-C', scratch], check=True)
            for arch, arch_flag in (('x86_64', '-mno-red-zone'),
                                   ('aarch64', '-mno-outline-atomics')):
                for source in sources:
                    objects = []
                    diagnostics = []
                    flags = []
                    if source == Path('toolchain/corpus/c11/atomics.c'):
                        flags.append('-D_POSIX_C_SOURCE=200809L')
                    if source == Path('toolchain/corpus/c11/profile-macros.c'):
                        # This header-free fixture checks bcc's advertised C11 profile.
                        flags.extend(['-D__STDC_NO_VLA__=1', '-D__STDC_NO_COMPLEX__=1',
                                      '-D__STDC_NO_THREADS__=1', '-U__STDC_IEC_559_COMPLEX__'])
                    for label, headers in (('base', scratch / HEADER_TREE),
                                           ('candidate', ROOT / HEADER_TREE)):
                        obj = scratch / f'{label}.o'
                        obj.unlink(missing_ok=True)
                        support = (ROOT / 'toolchain/include' if source.parts[0] == 'toolchain'
                                   else Path(resource) / 'include')
                        result = subprocess.run(
                            ['taskpolicy', '-b', clang, '--no-default-config',
                             f'--target={arch}-unknown-linux-gnu', arch_flag,
                             '-Ulinux', '-U__linux', '-U__linux__', '-U__gnu_linux__', '-D__breenix__=1',
                             '-std=c17', '-O2', '-fno-builtin', '-frounding-math', *flags,
                             '-fno-stack-protector', '-fno-pic', '-fno-pie',
                             '-nostdinc', '-isystem', headers, '-isystem', support,
                             f'-ffile-prefix-map={headers}=/breenix-include',
                             '-c', source, '-o', obj],
                            cwd=ROOT, env=env, capture_output=True, text=True, timeout=10,
                        )
                        if result.returncode:
                            failures.append(f'{arch} {source} ({label}): {result.stderr.strip()}')
                        else:
                            objects.append(obj.read_bytes())
                        diagnostics.append(result.stderr.replace(str(headers), '<headers>'))
                        if re.search(r'^<headers>/.*: warning:', diagnostics[-1], re.M):
                            failures.append(f'{arch} {source} ({label}): {diagnostics[-1].strip()}')
                    if len(objects) == 2 and objects[0] != objects[1]:
                        failures.append(f'{arch} {source}: object bytes differ')
                    if diagnostics[0] != diagnostics[1]:
                        failures.append(f'{arch} {source}: diagnostics differ\n'
                                        f'base: {diagnostics[0]}candidate: {diagnostics[1]}')
        self.assertTrue(sources)
        self.assertEqual('\n'.join(failures), '')

    def test_runtime_diagnostics_match_record(self):
        import cxx_runtime

        llvm = cxx_runtime.default_llvm().resolve()
        runs = Path(os.environ.get('BREENIX_CXX_RUNTIME_BUILDS', ROOT / 'build/cxx-runtime'))
        for arch in ('x86_64', 'aarch64'):
            with self.subTest(arch=arch):
                expected = json.loads((ROOT / f'toolchain/rust-host/k3/{arch}.json').read_text())
                run = (runs / arch).resolve()
                observed = json.loads((run / 'record.json').read_text())
                self.assertEqual(observed['clang++'], expected['clang++'])
                for stage in ('configure', 'build'):
                    warnings = cxx_runtime.diagnostics_from_log(run / f'{stage}.log', run, llvm)
                    for warning in warnings:
                        self.assertTrue(cxx_runtime.is_upstream_warning(warning), warning)
                    self.assertEqual(warnings, expected[stage]['warnings'], f'{arch} {stage}')
                    self.assertEqual(warnings, observed[stage]['warnings'], f'{arch} {stage}')

    def test_configure_follows_ruling_86(self):
        import json
        import cxx_runtime

        licenses = json.loads((ROOT / 'toolchain/rust-host/x0/licenses.json').read_text())
        self.assertIn('libc++', licenses)
        self.assertIn('libc++abi', licenses)
        for arch in ('x86_64', 'aarch64'):
            command = cxx_runtime.configure_command(ROOT, Path('/llvm'), Path('/build'), arch)
            options = dict(arg[2:].split('=', 1) for arg in command if arg.startswith('-D'))
            for key, value in {
                'LLVM_ENABLE_RUNTIMES': 'libcxx;libcxxabi',
                'LIBCXX_CXX_ABI': 'libcxxabi',
                'LIBCXX_ENABLE_EXCEPTIONS': 'OFF',
                'LIBCXXABI_ENABLE_EXCEPTIONS': 'OFF',
                'LIBCXXABI_USE_LLVM_UNWINDER': 'OFF',
                'LIBCXX_ENABLE_LOCALIZATION': 'ON',  # E25 ruling 93 requires streams.
                'LIBCXX_ENABLE_THREADS': 'ON',
                'LIBCXX_HAS_PTHREAD_API': 'ON',
                'LIBCXXABI_HAS_PTHREAD_API': 'ON',
                'LIBCXX_ENABLE_SHARED': 'OFF',
                'LIBCXXABI_ENABLE_SHARED': 'OFF',
                'LIBCXX_ENABLE_STATIC': 'ON',
                'LIBCXXABI_ENABLE_STATIC': 'ON',
            }.items():
                self.assertEqual(options[key], value, key)
            self.assertNotIn('LIBCXX_ENABLE_RTTI', options)
            self.assertNotIn('LIBCXXABI_ENABLE_RTTI', options)
            self.assertIn(str(ROOT / 'libs/libbreenix-libc/include'), options['CMAKE_CXX_FLAGS'])
        cxx_runtime.verify_llvm(ROOT, cxx_runtime.default_llvm())

    def test_platform_module_static_only(self):
        import cxx_runtime

        llvm = cxx_runtime.default_llvm()
        cxx_runtime.verify_llvm(ROOT, llvm)
        for arch in ('x86_64', 'aarch64'):
            with self.subTest(arch=arch), tempfile.TemporaryDirectory() as temp:
                build = Path(temp) / 'build'
                command = cxx_runtime.configure_command(ROOT, llvm, build, arch)
                self.assertIn('-DCMAKE_SYSTEM_NAME=Breenix', command)
                self.assertIn(f'-DCMAKE_MODULE_PATH={ROOT / "toolchain/cmake"}', command)
                subprocess.run(command, check=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                               text=True, timeout=55)
                ninja = (build / 'build.ninja').read_text().splitlines()
                for archive in ('libc++.a', 'libc++abi.a'):
                    rules = [line for line in ninja if line.startswith(f'build lib/{archive}:')]
                    self.assertEqual(len(rules), 1, rules)
                self.assertFalse(list(build.rglob('*.so')))
        cxx_runtime.verify_llvm(ROOT, llvm)

    def test_cxx_runtime_symbols_resolve(self):
        from cxx_runtime import LLVM_BIN

        nm = llvm_tool(ROOT, 'llvm-nm')
        readelf = os.environ.get('LLVM_READELF', str(LLVM_BIN / 'llvm-readelf'))
        lld = llvm_tool(ROOT, 'rust-lld')
        inputs = Path(os.environ.get('BREENIX_CXX_RUNTIME_ROOT', ROOT / 'build/cxx-runtime'))
        builds = Path(os.environ.get('BREENIX_CXX_RUNTIME_BUILDS', ROOT / 'build/cxx-runtime'))
        section_name = '__lcxx_override'
        start = '__start___lcxx_override'
        stop = '__stop___lcxx_override'
        for arch in ('x86_64', 'aarch64'):
            with self.subTest(arch=arch), tempfile.TemporaryDirectory(prefix='breenix-k3-link-') as temp:
                lib = inputs / arch / 'lib'
                archives = [lib / 'libc++.a', lib / 'libc++abi.a']
                providers = archives + [lib / 'libc.a', lib / 'compiler_builtins.rlib']
                for path in providers:
                    self.assertTrue(path.is_file(), f'missing required archive: {path}')
                undefined = set().union(*(symbols(nm, path, '--undefined-only') for path in archives))
                defined = set().union(*(symbols(nm, path, '--defined-only') for path in providers))
                forbidden = {name for name in undefined if name.startswith('_Unwind_')
                             or name in {'__gxx_personality_v0', '__cxa_throw'}}
                self.assertEqual(forbidden, set(), f'{arch}: exception or unwinder references')
                self.assertEqual(undefined - defined, {start, stop}, f'{arch}: unresolved archive symbols')

                # Reserve storage through libc++ and its new/delete ABI so the
                # hosted link reaches libc++abi's section-bound references.
                obj = Path(temp) / 'probe.o'
                elf = Path(temp) / 'probe.elf'
                subprocess.run(
                    [ROOT / 'scripts/workload-cxx', '--arch', arch,
                     '--runtime-build', builds / arch / 'build', '--',
                     '-std=c++23', '-Werror', '-x', 'c++', '-', '-o', obj],
                    input=('#include <string>\n'
                           'int main() {\n'
                           '    std::string text;\n'
                           '    text.reserve(128);\n'
                           '    return text.capacity() < 128;\n'
                           '}\n'),
                    text=True, check=True, timeout=10,
                )
                subprocess.run(
                    ['taskpolicy', '-b', lld, '-flavor', 'gnu', '-static', '--gc-sections',
                     '--eh-frame-hdr', '--fatal-warnings', '-T', ROOT / 'userspace/programs/linker-hosted.ld',
                     '-o', elf, obj, '--start-group', *providers, '--end-group'],
                    check=True, timeout=10,
                )
                self.assertEqual(subprocess.check_output([nm, '--undefined-only', elf], text=True), '')

                # GNU ld manual §3.6.4.5 places synthesized bounds at the
                # output section's first byte and one past its last byte.
                [report] = json.loads(subprocess.check_output(
                    [readelf, '--elf-output-style=JSON', '--sections', '--symbols', elf],
                ))
                [section] = [row['Section'] for row in report['Sections']
                             if row['Section']['Name']['Name'] == section_name]
                self.assertGreater(section['Size'], 0)
                bounds = {row['Symbol']['Name']['Name']: row['Symbol'] for row in report['Symbols']
                          if row['Symbol']['Name']['Name'] in {start, stop}}
                self.assertEqual(bounds.keys(), {start, stop})
                for name, value in ((start, section['Address']),
                                    (stop, section['Address'] + section['Size'])):
                    self.assertEqual(bounds[name]['Section']['Value'], section['Index'], name)
                    self.assertEqual(bounds[name]['Value'], value, name)


if __name__ == '__main__':
    unittest.main()
