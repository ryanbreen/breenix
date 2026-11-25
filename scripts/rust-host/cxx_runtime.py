#!/usr/bin/env python3
"""Configure and build the pinned K3 C++ runtimes with the Breenix locale adapter."""
import argparse
import json
import os
import re
import shlex
import shutil
import subprocess
import tomllib
import time
from datetime import datetime, timezone
from collections import Counter
from pathlib import Path

from x0_runtime import BACKGROUND, FIRST_ERROR, build, digest, output

ROOT = Path(__file__).resolve().parents[2]
ARCHES = ('x86_64', 'aarch64')
CLANGXX = '/usr/bin/clang++'
LLVM_BIN = Path('/opt/homebrew/opt/llvm/bin')
POSIX_FLAG = '-D_POSIX_C_SOURCE=200809L'  # POSIX.1-2024 section 2.2.1.
LOCALE_PATCH = ROOT / 'toolchain/rust-host/k3/patches/libcxx-breenix-locale.patch'
LOCALE_HEADER = Path('__locale_dir/locale_base_api/breenix.h')
CMAKE_WARNING = re.compile(r'^CMake (?:Deprecation )?Warning\b')
COMPILER_WARNING = re.compile(r'^.*\bwarning:')


def default_llvm():
    return Path(os.environ.get('BREENIX_LLVM_PROJECT', str(ROOT / 'rust-fork/src/llvm-project')))


def verify_llvm(root, llvm):
    fork = tomllib.loads((root / 'rust-fork.toml').read_text())['commit']
    pin = output('git', '-C', root / 'rust-fork', 'ls-tree', fork, 'src/llvm-project').split()[2]
    if output('git', '-C', llvm, 'rev-parse', 'HEAD') != pin or output('git', '-C', llvm, 'status', '--porcelain'):
        raise ValueError('LLVM workload must be clean and match the pinned gitlink')
    return pin


def prepare_runtime_source(llvm, scratch):
    source = scratch / 'source'
    source.mkdir()
    for entry in llvm.iterdir():
        if entry.name in ('libcxx', 'runtimes'):
            shutil.copytree(entry, source / entry.name, symlinks=True)
        elif not entry.name.startswith('.'):
            (source / entry.name).symlink_to(entry)
    subprocess.run(['git', 'apply', '--no-index', LOCALE_PATCH], cwd=source, check=True)
    return source


def configure_command(root, llvm, directory, arch):
    if arch not in ARCHES:
        raise ValueError(f'unsupported architecture: {arch}')
    flags = shlex.join(('-nostdlibinc', '-isystem', str(root / 'libs/libbreenix-libc/include'),
                        POSIX_FLAG, '-D__breenix__=1'))
    triple = f'{arch}-unknown-none'
    return (*BACKGROUND, 'cmake', '-G', 'Ninja', '-S', str(llvm / 'runtimes'), '-B', str(directory),
            '-DCMAKE_SYSTEM_NAME=Breenix', f'-DCMAKE_MODULE_PATH={root / "toolchain/cmake"}',
            f'-DCMAKE_SYSTEM_PROCESSOR={arch}',
            '-DCMAKE_BUILD_TYPE=Release', '-DCMAKE_TRY_COMPILE_TARGET_TYPE=STATIC_LIBRARY',
            '-DCMAKE_C_COMPILER=/usr/bin/clang', f'-DCMAKE_CXX_COMPILER={CLANGXX}',
            f'-DCMAKE_AR={LLVM_BIN / "llvm-ar"}', f'-DCMAKE_RANLIB={LLVM_BIN / "llvm-ranlib"}',
            '-DCMAKE_ASM_COMPILER=/usr/bin/clang', f'-DCMAKE_C_COMPILER_TARGET={triple}',
            f'-DCMAKE_CXX_COMPILER_TARGET={triple}', f'-DCMAKE_ASM_COMPILER_TARGET={triple}',
            f'-DCMAKE_C_FLAGS={flags}', f'-DCMAKE_CXX_FLAGS={flags}',
            '-DLLVM_ENABLE_RUNTIMES=libcxx;libcxxabi', '-DLLVM_INCLUDE_TESTS=OFF',
            '-DLIBCXX_CXX_ABI=libcxxabi', '-DLIBCXX_ENABLE_LOCALIZATION=ON',
            '-DLIBCXX_ENABLE_FILESYSTEM=OFF',
            '-DLIBCXX_ENABLE_THREADS=ON', '-DTHREADS_PREFER_PTHREAD_FLAG=ON',
            '-DLIBCXX_HAS_PTHREAD_API=ON', '-DLIBCXXABI_HAS_PTHREAD_API=ON',
            '-DLIBCXX_ENABLE_EXCEPTIONS=OFF', '-DLIBCXXABI_ENABLE_EXCEPTIONS=OFF',
            '-DLIBCXXABI_USE_LLVM_UNWINDER=OFF', '-DLIBCXX_ENABLE_SHARED=OFF',
            '-DLIBCXXABI_ENABLE_SHARED=OFF', '-DLIBCXX_ENABLE_STATIC=ON',
            '-DLIBCXXABI_ENABLE_STATIC=ON', '-DLIBCXX_ENABLE_STATIC_ABI_LIBRARY=OFF')


def normalize(text, scratch, llvm):
    for path, label in ((llvm, '<llvm-project>'), (scratch, '<scratch>'),
                        (ROOT, '<repo>'), (Path.home(), '<home>')):
        text = text.replace(str(path), label)
    return text


def diagnostics_from_log(log, scratch, llvm):
    lines = log.read_text(errors='replace').splitlines()
    warnings = Counter()
    index = 0
    while index < len(lines):
        line = lines[index]
        index += 1
        if CMAKE_WARNING.match(line):
            block = [line]
            while index < len(lines) and not lines[index].startswith(('CMake ', '-- ')):
                block.append(lines[index])
                index += 1
            warnings[normalize('\n'.join(block).rstrip(), scratch, llvm)] += 1
        elif COMPILER_WARNING.match(line):
            warnings[normalize(line, scratch, llvm)] += 1
    return dict(sorted(warnings.items()))


def is_upstream_warning(warning):
    location = warning.splitlines()[0]
    if str(LOCALE_HEADER) in location:
        return False
    if CMAKE_WARNING.match(location):
        return '<repo>' not in location
    return location.startswith(('<llvm-project>/libcxx/', '<llvm-project>/libcxxabi/',
                                '<scratch>/source/libcxx/', '<scratch>/source/libcxxabi/',
                                '<scratch>/build/include/c++/v1/'))


def use_explicit_link_inputs(archive):
    # The workload emits ELF directives for separate pthread and rt libraries.
    # Both APIs come from the explicit Breenix libc.a link input.
    dump = subprocess.run([LLVM_BIN / 'llvm-readelf', '--string-dump=.deplibs', archive],
                          check=True, capture_output=True, text=True).stdout
    names = sorted(set(re.findall(r'^\[\s*\d+\] (.*)$', dump, re.MULTILINE)))
    if set(names) - {'pthread', 'rt'}:
        raise ValueError(f'{archive.name}: unexpected implicit link inputs: {names}')
    if names:
        subprocess.run([*BACKGROUND, LLVM_BIN / 'llvm-objcopy', '--remove-section=.deplibs',
                        archive], check=True)
    return names


def run_stage(name, command, scratch, llvm, *, timeout=55, allowlist=None):
    log = scratch / f'{name}.log'
    started = datetime.now(timezone.utc)
    clock = time.monotonic()
    status = build(command, log, env=dict(os.environ, CARGO_BUILD_JOBS='4'), timeout=timeout)
    elapsed = time.monotonic() - clock
    ended = datetime.now(timezone.utc)
    lines = log.read_text(errors='replace').splitlines()
    warnings = diagnostics_from_log(log, scratch, llvm)
    pending = Counter(warnings)
    diagnostics = []
    for line in lines:
        normalized = normalize(line, scratch, llvm)
        if FIRST_ERROR.search(line):
            diagnostics.append(normalized)
        else:
            warning = next((entry for entry in pending
                            if pending[entry] and entry.splitlines()[0] == normalized), None)
            if warning is not None:
                diagnostics.append(warning)
                pending[warning] -= 1
    remaining = Counter(allowlist or {})
    error = None
    for diagnostic in diagnostics:
        if remaining[diagnostic] > 0:
            remaining[diagnostic] -= 1
        elif error is None:
            error = diagnostic
    if status == 124:
        last_step = next((line for line in reversed(lines) if re.match(r'^\[\d+/\d+\]', line)),
                         'no ninja step recorded')
        error = f'Process exceeded {timeout} seconds after {elapsed:.2f}s; last step: {last_step}'
    elif status and not error:
        error = f'Process exited {status}'
    return {'started': started.isoformat(), 'ended': ended.isoformat(), 'elapsed_seconds': elapsed,
            'command': [normalize(str(arg), scratch, llvm) for arg in command],
            'exit_status': status, 'first_error': error, 'diagnostics': dict(Counter(diagnostics)),
            'warnings': warnings, 'log_sha256': digest(log)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('arch', choices=ARCHES)
    parser.add_argument('--llvm-project', type=Path, default=default_llvm())
    parser.add_argument('--scratch', type=Path, required=True)
    parser.add_argument('--record', type=Path, required=True)
    args = parser.parse_args()
    llvm = args.llvm_project.resolve()
    pin = verify_llvm(ROOT, llvm)
    scratch = args.scratch.resolve()
    scratch.mkdir(parents=True, exist_ok=False)
    source = prepare_runtime_source(llvm, scratch)
    directory = scratch / 'build'
    clangxx = {'path': CLANGXX, 'version': output(CLANGXX, '--version').splitlines()[0]}
    expected = json.loads((ROOT / f'toolchain/rust-host/k3/{args.arch}.json').read_text())
    configure = run_stage('configure', configure_command(ROOT, source, directory, args.arch), scratch, llvm,
                          allowlist=expected['configure']['warnings'])
    built = None
    archives = {}
    if configure['exit_status'] == 0:
        # The pinned generated-header list does not know about this local overlay.
        header = directory / 'include/c++/v1' / LOCALE_HEADER
        header.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source / 'libcxx/include' / LOCALE_HEADER, header)
        built = run_stage('build', (*BACKGROUND, 'cmake', '--build', str(directory), '--parallel', '4',
                                    '--target', 'cxx_static', 'cxxabi_static'), scratch, llvm,
                          timeout=1800, allowlist=expected['build']['warnings'])
        if built['exit_status'] == 0:
            for name, component in (('libc++.a', 'libcxx'), ('libc++abi.a', 'libcxxabi')):
                archive, = directory.rglob(name)
                input_sha256 = digest(archive)
                redirected = use_explicit_link_inputs(archive)
                notices = archive.parent / (name + '.LICENSE.TXT')
                shutil.copyfile(llvm / component / 'LICENSE.TXT', notices)
                archives[name] = {'sha256': digest(archive), 'notice_sha256': digest(notices),
                                  'input_sha256': input_sha256, 'provided_by_libc': redirected}
    verify_llvm(ROOT, llvm)
    record = {'claim-lint:ok': 'claim-lint:ok: compiler target triples, see toolchain/rust-host/k3/README.md',
              'arch': args.arch, 'llvm_project': pin, 'clang++': clangxx, 'configure': configure,
              'build': built, 'archives': archives,
              'locale_patch': {'sha256': digest(LOCALE_PATCH), 'text': LOCALE_PATCH.read_text()},
              'generic_collision_targets': ['cxxabi_shared', 'cxxabi_static']}
    args.record.parent.mkdir(parents=True, exist_ok=True)
    args.record.write_text(json.dumps(record, indent=2) + '\n')
    print(json.dumps(record, indent=2))
    return 0 if all(
        stage and stage['exit_status'] == 0 and stage['first_error'] is None
        and all(is_upstream_warning(warning) for warning in stage['warnings'])
        for stage in (configure, built)
    ) else 1


if __name__ == '__main__':
    raise SystemExit(main())
